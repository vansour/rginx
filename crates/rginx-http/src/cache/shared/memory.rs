#![allow(dead_code)]

use std::ffi::CString;
use std::io;
use std::mem::{MaybeUninit, size_of};
use std::os::fd::RawFd;
use std::ptr::NonNull;
use std::time::{SystemTime, UNIX_EPOCH};

const SHM_MAGIC: u64 = 0x4d48_5358_4e49_4752;
const SHM_ABI_VERSION: u32 = 1;
const DEFAULT_HASH_BUCKET_COUNT: u64 = 1_024;
const DEFAULT_OPERATION_RING_CAPACITY: u64 = 4_096;
const SHM_NAME_PREFIX: &str = "/rginx-cache-";

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct SharedMemoryHeader {
    pub(super) magic: u64,
    pub(super) abi_version: u32,
    pub(super) header_len: u32,
    pub(super) zone_name_hash: u64,
    pub(super) capacity_bytes: u64,
    pub(super) generation: u64,
    pub(super) store_epoch: u64,
    pub(super) operation_seq: u64,
    pub(super) entry_count: u64,
    pub(super) current_size_bytes: u64,
    pub(super) hash_bucket_count: u64,
    pub(super) operation_ring_capacity: u64,
    pub(super) allocator_free_head: u64,
    pub(super) flags: u64,
}

#[derive(Clone, Debug)]
pub(super) struct SharedMemorySegmentConfig {
    pub(super) name: String,
    pub(super) zone_name_hash: u64,
    pub(super) capacity_bytes: usize,
    pub(super) hash_bucket_count: u64,
    pub(super) operation_ring_capacity: u64,
    pub(super) flags: u64,
}

impl SharedMemorySegmentConfig {
    pub(super) fn for_zone(zone_name: &str, capacity_bytes: usize) -> Self {
        let zone_name_hash = stable_hash(zone_name.as_bytes());
        Self::new(format!("{SHM_NAME_PREFIX}{zone_name_hash:016x}"), zone_name_hash, capacity_bytes)
    }

    pub(super) fn for_identity(identity: &str, capacity_bytes: usize) -> Self {
        let zone_name_hash = stable_hash(identity.as_bytes());
        Self::new(format!("{SHM_NAME_PREFIX}{zone_name_hash:016x}"), zone_name_hash, capacity_bytes)
    }

    pub(super) fn new(name: impl Into<String>, zone_name_hash: u64, capacity_bytes: usize) -> Self {
        Self {
            name: name.into(),
            zone_name_hash,
            capacity_bytes,
            hash_bucket_count: DEFAULT_HASH_BUCKET_COUNT,
            operation_ring_capacity: DEFAULT_OPERATION_RING_CAPACITY,
            flags: 0,
        }
    }

    pub(super) fn with_hash_bucket_count(mut self, hash_bucket_count: u64) -> Self {
        self.hash_bucket_count = hash_bucket_count;
        self
    }

    pub(super) fn with_operation_ring_capacity(mut self, operation_ring_capacity: u64) -> Self {
        self.operation_ring_capacity = operation_ring_capacity;
        self
    }

    fn shm_name(&self) -> io::Result<CString> {
        if !self.name.starts_with('/') {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "shared memory name must start with `/`",
            ));
        }
        CString::new(self.name.as_str()).map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("shared memory name contains NUL byte: {error}"),
            )
        })
    }

    fn validate_capacity(&self) -> io::Result<()> {
        if self.capacity_bytes < size_of::<SharedMemoryHeader>() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "shared memory capacity must be at least {} bytes",
                    size_of::<SharedMemoryHeader>()
                ),
            ));
        }
        let _ = i64::try_from(self.capacity_bytes).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidInput, "shared memory capacity does not fit off_t")
        })?;
        Ok(())
    }

    fn initial_header(&self) -> io::Result<SharedMemoryHeader> {
        Ok(SharedMemoryHeader {
            magic: SHM_MAGIC,
            abi_version: SHM_ABI_VERSION,
            header_len: u32::try_from(size_of::<SharedMemoryHeader>()).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidInput, "shared memory header is too large")
            })?,
            zone_name_hash: self.zone_name_hash,
            capacity_bytes: u64::try_from(self.capacity_bytes).map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "shared memory capacity does not fit u64",
                )
            })?,
            generation: 1,
            store_epoch: current_store_epoch(),
            operation_seq: 0,
            entry_count: 0,
            current_size_bytes: 0,
            hash_bucket_count: self.hash_bucket_count,
            operation_ring_capacity: self.operation_ring_capacity,
            allocator_free_head: 0,
            flags: self.flags,
        })
    }
}

#[derive(Debug)]
pub(super) struct SharedMemorySegment {
    name: CString,
    fd: RawFd,
    ptr: NonNull<u8>,
    capacity_bytes: usize,
}

unsafe impl Send for SharedMemorySegment {}
unsafe impl Sync for SharedMemorySegment {}

impl SharedMemorySegment {
    pub(super) fn create_or_reset(config: &SharedMemorySegmentConfig) -> io::Result<Self> {
        config.validate_capacity()?;
        let name = config.shm_name()?;
        let fd = shm_open(&name, libc::O_CREAT | libc::O_RDWR, 0o600)?;
        if let Err(error) = ftruncate(fd, config.capacity_bytes) {
            let _ = close_fd(fd);
            return Err(error);
        }
        let segment = match Self::map(name, fd, config.capacity_bytes) {
            Ok(segment) => segment,
            Err(error) => {
                let _ = close_fd(fd);
                return Err(error);
            }
        };
        segment.zero();
        segment.write_header(config.initial_header()?);
        Ok(segment)
    }

    pub(super) fn attach(config: &SharedMemorySegmentConfig) -> io::Result<Self> {
        config.validate_capacity()?;
        let name = config.shm_name()?;
        let fd = shm_open(&name, libc::O_RDWR, 0o600)?;
        let actual_capacity = match fd_size(fd) {
            Ok(size) => size,
            Err(error) => {
                let _ = close_fd(fd);
                return Err(error);
            }
        };
        if actual_capacity != config.capacity_bytes {
            let _ = close_fd(fd);
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "shared memory capacity mismatch: expected {}, found {}",
                    config.capacity_bytes, actual_capacity
                ),
            ));
        }
        let segment = match Self::map(name, fd, config.capacity_bytes) {
            Ok(segment) => segment,
            Err(error) => {
                let _ = close_fd(fd);
                return Err(error);
            }
        };
        if let Err(error) = segment.validate_header(config) {
            drop(segment);
            return Err(error);
        }
        Ok(segment)
    }

    pub(super) fn unlink(config: &SharedMemorySegmentConfig) -> io::Result<()> {
        let name = config.shm_name()?;
        let result = unsafe { libc::shm_unlink(name.as_ptr()) };
        if result == 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        if error.kind() == io::ErrorKind::NotFound { Ok(()) } else { Err(error) }
    }

    pub(super) fn header(&self) -> SharedMemoryHeader {
        unsafe { self.header_ptr().read() }
    }

    pub(super) fn payload_capacity(&self) -> usize {
        self.capacity_bytes.saturating_sub(size_of::<SharedMemoryHeader>())
    }

    pub(super) fn write_payload(&self, offset: usize, bytes: &[u8]) -> io::Result<()> {
        self.validate_payload_range(offset, bytes.len())?;
        unsafe {
            std::ptr::copy_nonoverlapping(
                bytes.as_ptr(),
                self.payload_ptr().add(offset),
                bytes.len(),
            );
        }
        Ok(())
    }

    pub(super) fn read_payload(&self, offset: usize, len: usize) -> io::Result<Vec<u8>> {
        self.validate_payload_range(offset, len)?;
        let mut bytes = vec![0; len];
        unsafe {
            std::ptr::copy_nonoverlapping(self.payload_ptr().add(offset), bytes.as_mut_ptr(), len);
        }
        Ok(bytes)
    }

    fn map(name: CString, fd: RawFd, capacity_bytes: usize) -> io::Result<Self> {
        let mapped = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                capacity_bytes,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd,
                0,
            )
        };
        if mapped == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        let ptr = NonNull::new(mapped.cast::<u8>()).ok_or_else(|| {
            io::Error::other("mmap returned a null pointer for shared memory segment")
        })?;
        Ok(Self { name, fd, ptr, capacity_bytes })
    }

    fn zero(&self) {
        unsafe {
            std::ptr::write_bytes(self.ptr.as_ptr(), 0, self.capacity_bytes);
        }
    }

    pub(super) fn write_header(&self, header: SharedMemoryHeader) {
        unsafe {
            self.header_ptr().write(header);
        }
    }

    fn validate_header(&self, config: &SharedMemorySegmentConfig) -> io::Result<()> {
        let header = self.header();
        if header.magic != SHM_MAGIC {
            return Err(invalid_header("magic", SHM_MAGIC, header.magic));
        }
        if header.abi_version != SHM_ABI_VERSION {
            return Err(invalid_header(
                "abi_version",
                u64::from(SHM_ABI_VERSION),
                u64::from(header.abi_version),
            ));
        }
        let expected_header_len = u32::try_from(size_of::<SharedMemoryHeader>()).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidInput, "shared memory header is too large")
        })?;
        if header.header_len != expected_header_len {
            return Err(invalid_header(
                "header_len",
                u64::from(expected_header_len),
                u64::from(header.header_len),
            ));
        }
        if header.zone_name_hash != config.zone_name_hash {
            return Err(invalid_header(
                "zone_name_hash",
                config.zone_name_hash,
                header.zone_name_hash,
            ));
        }
        let expected_capacity = u64::try_from(config.capacity_bytes).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidInput, "shared memory capacity does not fit u64")
        })?;
        if header.capacity_bytes != expected_capacity {
            return Err(invalid_header("capacity_bytes", expected_capacity, header.capacity_bytes));
        }
        if header.hash_bucket_count != config.hash_bucket_count {
            return Err(invalid_header(
                "hash_bucket_count",
                config.hash_bucket_count,
                header.hash_bucket_count,
            ));
        }
        if header.operation_ring_capacity != config.operation_ring_capacity {
            return Err(invalid_header(
                "operation_ring_capacity",
                config.operation_ring_capacity,
                header.operation_ring_capacity,
            ));
        }
        if header.flags != config.flags {
            return Err(invalid_header("flags", config.flags, header.flags));
        }
        Ok(())
    }

    fn validate_payload_range(&self, offset: usize, len: usize) -> io::Result<()> {
        let end = offset.checked_add(len).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "shared memory payload range overflowed")
        })?;
        if end > self.payload_capacity() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "shared memory payload range exceeds capacity: end {}, capacity {}",
                    end,
                    self.payload_capacity()
                ),
            ));
        }
        Ok(())
    }

    fn header_ptr(&self) -> *mut SharedMemoryHeader {
        self.ptr.as_ptr().cast::<SharedMemoryHeader>()
    }

    fn payload_ptr(&self) -> *mut u8 {
        unsafe { self.ptr.as_ptr().add(size_of::<SharedMemoryHeader>()) }
    }
}

impl Drop for SharedMemorySegment {
    fn drop(&mut self) {
        let _ = unsafe { libc::munmap(self.ptr.as_ptr().cast(), self.capacity_bytes) };
        let _ = close_fd(self.fd);
    }
}

fn shm_open(name: &CString, flags: libc::c_int, mode: libc::mode_t) -> io::Result<RawFd> {
    let fd = unsafe { libc::shm_open(name.as_ptr(), flags, mode) };
    if fd < 0 { Err(io::Error::last_os_error()) } else { Ok(fd) }
}

fn ftruncate(fd: RawFd, capacity_bytes: usize) -> io::Result<()> {
    let len = i64::try_from(capacity_bytes).map_err(|_| {
        io::Error::new(io::ErrorKind::InvalidInput, "shared memory capacity does not fit off_t")
    })? as libc::off_t;
    if unsafe { libc::ftruncate(fd, len) } == 0 { Ok(()) } else { Err(io::Error::last_os_error()) }
}

fn fd_size(fd: RawFd) -> io::Result<usize> {
    let mut stat = MaybeUninit::<libc::stat>::uninit();
    if unsafe { libc::fstat(fd, stat.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let stat = unsafe { stat.assume_init() };
    usize::try_from(stat.st_size)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "negative shared memory size"))
}

fn close_fd(fd: RawFd) -> io::Result<()> {
    if unsafe { libc::close(fd) } == 0 { Ok(()) } else { Err(io::Error::last_os_error()) }
}

fn invalid_header(field: &str, expected: u64, actual: u64) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("shared memory header {field} mismatch: expected {expected}, found {actual}"),
    )
}

fn current_store_epoch() -> u64 {
    let epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .min(u128::from(u64::MAX)) as u64;
    epoch.max(1)
}

pub(super) fn stable_hash(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_memory_segment_create_attach_and_payload_round_trip() {
        let config = test_config("round-trip", 4_096);
        let _ = SharedMemorySegment::unlink(&config);
        let segment = SharedMemorySegment::create_or_reset(&config)
            .expect("shared memory segment should be created");
        let header = segment.header();
        assert_eq!(header.magic, SHM_MAGIC);
        assert_eq!(header.abi_version, SHM_ABI_VERSION);
        assert_eq!(header.zone_name_hash, config.zone_name_hash);
        assert_eq!(header.capacity_bytes, config.capacity_bytes as u64);
        assert_eq!(segment.payload_capacity(), 4_096 - size_of::<SharedMemoryHeader>());

        segment.write_payload(7, b"phase2").expect("payload write should succeed");
        drop(segment);

        let attached =
            SharedMemorySegment::attach(&config).expect("shared memory segment should attach");
        assert_eq!(
            attached.read_payload(7, 6).expect("payload read should succeed"),
            b"phase2".to_vec()
        );
        drop(attached);
        SharedMemorySegment::unlink(&config).expect("shared memory segment should unlink");
    }

    #[test]
    fn shared_memory_attach_rejects_zone_hash_mismatch() {
        let config = test_config("zone-mismatch", 4_096);
        let _ = SharedMemorySegment::unlink(&config);
        let segment = SharedMemorySegment::create_or_reset(&config)
            .expect("shared memory segment should be created");

        let mut mismatched = config.clone();
        mismatched.zone_name_hash ^= 1;
        let error = SharedMemorySegment::attach(&mismatched)
            .expect_err("zone hash mismatch should reject attach");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);

        drop(segment);
        SharedMemorySegment::unlink(&config).expect("shared memory segment should unlink");
    }

    #[test]
    fn shared_memory_attach_rejects_capacity_mismatch() {
        let config = test_config("capacity-mismatch", 4_096);
        let _ = SharedMemorySegment::unlink(&config);
        let segment = SharedMemorySegment::create_or_reset(&config)
            .expect("shared memory segment should be created");

        let mut mismatched = config.clone();
        mismatched.capacity_bytes = 8_192;
        let error = SharedMemorySegment::attach(&mismatched)
            .expect_err("capacity mismatch should reject attach");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);

        drop(segment);
        SharedMemorySegment::unlink(&config).expect("shared memory segment should unlink");
    }

    #[test]
    fn shared_memory_attach_rejects_abi_mismatch_and_create_resets() {
        let config = test_config("abi-reset", 4_096);
        let _ = SharedMemorySegment::unlink(&config);
        let segment = SharedMemorySegment::create_or_reset(&config)
            .expect("shared memory segment should be created");

        let mut header = segment.header();
        header.abi_version = SHM_ABI_VERSION + 1;
        segment.write_header(header);
        let error =
            SharedMemorySegment::attach(&config).expect_err("ABI mismatch should reject attach");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        drop(segment);

        let reset = SharedMemorySegment::create_or_reset(&config)
            .expect("shared memory segment should be reset");
        assert_eq!(reset.header().abi_version, SHM_ABI_VERSION);
        drop(reset);
        SharedMemorySegment::unlink(&config).expect("shared memory segment should unlink");
    }

    #[test]
    fn shared_memory_create_rejects_too_small_capacity() {
        let config =
            test_config("small-capacity", size_of::<SharedMemoryHeader>().saturating_sub(1));
        let error = SharedMemorySegment::create_or_reset(&config)
            .expect_err("too small capacity should reject create");
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        let _ = SharedMemorySegment::unlink(&config);
    }

    fn test_config(label: &str, capacity_bytes: usize) -> SharedMemorySegmentConfig {
        let unique = format!("{}-{}-{label}", std::process::id(), current_store_epoch());
        let zone_name_hash = stable_hash(unique.as_bytes());
        SharedMemorySegmentConfig::new(
            format!("{SHM_NAME_PREFIX}test-{zone_name_hash:016x}"),
            zone_name_hash,
            capacity_bytes,
        )
    }
}
