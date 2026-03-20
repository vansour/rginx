use rginx_core::{ConfigSnapshot, Result};

pub fn reload(_next: &ConfigSnapshot) -> Result<()> {
    Ok(())
}
