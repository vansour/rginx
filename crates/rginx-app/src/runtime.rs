use anyhow::anyhow;

pub(crate) fn build_runtime(
    worker_threads: Option<usize>,
) -> anyhow::Result<tokio::runtime::Runtime> {
    let mut builder = tokio::runtime::Builder::new_multi_thread();
    builder.enable_all();
    if let Some(worker_threads) = worker_threads {
        builder.worker_threads(worker_threads);
    }
    builder.build().map_err(|error| anyhow!("failed to build tokio runtime: {error}"))
}
