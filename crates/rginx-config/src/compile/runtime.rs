use std::time::Duration;

use rginx_core::{Error, Result, RuntimeSettings};

use crate::model::RuntimeConfig;

pub(super) fn compile_runtime_settings(runtime: RuntimeConfig) -> Result<RuntimeSettings> {
    let RuntimeConfig { shutdown_timeout_secs, worker_threads, accept_workers } = runtime;

    Ok(RuntimeSettings {
        shutdown_timeout: Duration::from_secs(shutdown_timeout_secs),
        worker_threads: compile_runtime_worker_threads(worker_threads)?,
        accept_workers: compile_runtime_accept_workers(accept_workers)?,
    })
}

fn compile_runtime_worker_threads(worker_threads: Option<u64>) -> Result<Option<usize>> {
    worker_threads
        .map(|value| {
            usize::try_from(value).map_err(|_| {
                Error::Config(format!("runtime worker_threads `{value}` exceeds platform limits"))
            })
        })
        .transpose()
}

fn compile_runtime_accept_workers(accept_workers: Option<u64>) -> Result<usize> {
    accept_workers
        .map(|value| {
            usize::try_from(value).map_err(|_| {
                Error::Config(format!("runtime accept_workers `{value}` exceeds platform limits"))
            })
        })
        .transpose()
        .map(|value| value.unwrap_or(1))
}
