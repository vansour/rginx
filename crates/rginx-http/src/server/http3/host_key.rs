use super::*;

const HTTP3_HOST_KEY_BYTES: usize = 64;

pub(super) fn load_or_create_http3_host_key(
    http3: Option<&rginx_core::ListenerHttp3>,
) -> Result<Option<Vec<u8>>> {
    let Some(path) = http3.and_then(|http3| http3.host_key_path.as_deref()) else {
        return Ok(None);
    };

    Ok(Some(load_or_create_host_key_material(path)?))
}

fn load_or_create_host_key_material(path: &Path) -> Result<Vec<u8>> {
    match std::fs::read(path) {
        Ok(bytes) => validate_host_key_material(path, bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            create_host_key_material(path)
        }
        Err(error) => Err(Error::Io(error)),
    }
}

fn validate_host_key_material(path: &Path, bytes: Vec<u8>) -> Result<Vec<u8>> {
    if bytes.len() != HTTP3_HOST_KEY_BYTES {
        return Err(Error::Config(format!(
            "http3 host_key_path `{}` must contain exactly {} bytes",
            path.display(),
            HTTP3_HOST_KEY_BYTES
        )));
    }

    Ok(bytes)
}

fn create_host_key_material(path: &Path) -> Result<Vec<u8>> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }

    let mut bytes = vec![0u8; HTTP3_HOST_KEY_BYTES];
    rand::SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| Error::Server("failed to generate http3 host key material".to_string()))?;

    use std::io::Write as _;

    let mut file = match std::fs::OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            return validate_host_key_material(path, std::fs::read(path).map_err(Error::Io)?);
        }
        Err(error) => return Err(Error::Io(error)),
    };
    file.write_all(&bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    file.flush().map_err(Error::Io)?;
    file.sync_all().map_err(Error::Io)?;

    Ok(bytes)
}

pub(super) fn derive_labeled_key_material(host_key_material: &[u8], label: &[u8]) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(label);
    digest.update(host_key_material);
    digest.finalize().into()
}

pub(super) fn derive_hashed_connection_id_key(host_key_material: &[u8]) -> u64 {
    let digest = derive_labeled_key_material(host_key_material, b"rginx-http3-cid-key");
    u64::from_be_bytes(digest[..8].try_into().expect("sha256 digest should contain 8 bytes"))
}
