use rginx_core::{Error, Result};

use crate::model::RuntimeConfig;

pub(super) fn validate_runtime(runtime: &RuntimeConfig) -> Result<()> {
    if runtime.shutdown_timeout_secs == 0 {
        return Err(Error::Config(
            "runtime.shutdown_timeout_secs must be greater than 0".to_string(),
        ));
    }

    if runtime.worker_threads.is_some_and(|count| count == 0) {
        return Err(Error::Config("runtime.worker_threads must be greater than 0".to_string()));
    }

    if runtime.accept_workers.is_some_and(|count| count == 0) {
        return Err(Error::Config("runtime.accept_workers must be greater than 0".to_string()));
    }

    Ok(())
}
