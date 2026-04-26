use super::ListenerWorkerGroup;

pub(crate) fn initiate_shutdown_for_groups<'a>(
    groups: impl IntoIterator<Item = &'a ListenerWorkerGroup>,
) {
    for group in groups {
        group.initiate_shutdown();
    }
}

pub(crate) fn abort_listener_worker_groups<'a>(
    groups: impl IntoIterator<Item = &'a ListenerWorkerGroup>,
) {
    for group in groups {
        group.abort();
    }
}
