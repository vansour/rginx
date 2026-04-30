use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use instant_acme::AccountCredentials;
use rginx_core::{AcmeSettings, Error, ManagedCertificateSpec, Result};
use serde::{Deserialize, Serialize};

const CERT_FILE_MODE: u32 = 0o644;
const PRIVATE_KEY_FILE_MODE: u32 = 0o600;

static TEMP_FILE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Serialize, Deserialize)]
pub(crate) struct PersistedAccountCredentials {
    pub(crate) directory_url: String,
    pub(crate) credentials: AccountCredentials,
}

pub(crate) fn load_account_credentials(
    settings: &AcmeSettings,
) -> Result<Option<PersistedAccountCredentials>> {
    let path = account_credentials_path(settings);
    match fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map(Some).map_err(|error| {
            Error::Server(format!(
                "failed to parse persisted ACME account credentials `{}`: {error}",
                path.display()
            ))
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(Error::Io(error)),
    }
}

pub(crate) fn store_account_credentials(
    settings: &AcmeSettings,
    persisted: &PersistedAccountCredentials,
) -> Result<()> {
    let path = account_credentials_path(settings);
    let bytes = serde_json::to_vec_pretty(persisted)
        .map_err(|error| Error::Server(format!("failed to serialize ACME account: {error}")))?;
    atomic_write(&path, &bytes, PRIVATE_KEY_FILE_MODE)
}

pub(crate) fn write_certificate_pair(
    spec: &ManagedCertificateSpec,
    certificate_chain_pem: &str,
    private_key_pem: &str,
) -> Result<()> {
    atomic_write(&spec.cert_path, certificate_chain_pem.as_bytes(), CERT_FILE_MODE)?;
    atomic_write(&spec.key_path, private_key_pem.as_bytes(), PRIVATE_KEY_FILE_MODE)
}

fn account_credentials_path(settings: &AcmeSettings) -> PathBuf {
    settings.state_dir.join("account.json")
}

fn atomic_write(path: &Path, contents: &[u8], mode: u32) -> Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;

    let temp_path = temporary_path(path);
    let write_result = (|| -> Result<()> {
        let mut file = create_temp_file(&temp_path, mode)?;
        file.write_all(contents)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temp_path, path)?;
        sync_directory(parent)?;
        Ok(())
    })();

    if write_result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }

    write_result
}

fn create_temp_file(path: &Path, mode: u32) -> Result<File> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;

        return OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(mode)
            .open(path)
            .map_err(Error::from);
    }

    #[allow(unreachable_code)]
    OpenOptions::new().create_new(true).write(true).open(path).map_err(Error::from)
}

fn temporary_path(path: &Path) -> PathBuf {
    let timestamp_nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let sequence = TEMP_FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let file_name = path.file_name().and_then(|value| value.to_str()).unwrap_or("acme-material");
    path.with_file_name(format!("{file_name}.tmp-{timestamp_nanos}-{sequence}"))
}

fn sync_directory(path: &Path) -> Result<()> {
    File::open(path)?.sync_all()?;
    Ok(())
}
