use crate::{ConfigSnapshot, Result};

pub trait LifecycleHook: Send + Sync {
    fn on_reload(&self, _next: &ConfigSnapshot) -> Result<()> {
        Ok(())
    }
}
