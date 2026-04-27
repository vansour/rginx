#[derive(Default)]
pub(super) struct RouteTransportCheckDetails {
    pub(super) request_buffering_auto_routes: usize,
    pub(super) request_buffering_on_routes: usize,
    pub(super) request_buffering_off_routes: usize,
    pub(super) response_buffering_auto_routes: usize,
    pub(super) response_buffering_on_routes: usize,
    pub(super) response_buffering_off_routes: usize,
    pub(super) compression_auto_routes: usize,
    pub(super) compression_off_routes: usize,
    pub(super) compression_force_routes: usize,
    pub(super) custom_compression_min_bytes_routes: usize,
    pub(super) custom_compression_content_types_routes: usize,
    pub(super) streaming_response_idle_timeout_routes: usize,
    pub(super) cache_enabled_routes: usize,
}

pub(super) fn route_transport_check_details(
    config: &rginx_config::ConfigSnapshot,
) -> RouteTransportCheckDetails {
    let mut details = RouteTransportCheckDetails::default();

    for route in all_routes(config) {
        match route.request_buffering {
            rginx_core::RouteBufferingPolicy::Auto => details.request_buffering_auto_routes += 1,
            rginx_core::RouteBufferingPolicy::On => details.request_buffering_on_routes += 1,
            rginx_core::RouteBufferingPolicy::Off => details.request_buffering_off_routes += 1,
        }

        match route.response_buffering {
            rginx_core::RouteBufferingPolicy::Auto => details.response_buffering_auto_routes += 1,
            rginx_core::RouteBufferingPolicy::On => details.response_buffering_on_routes += 1,
            rginx_core::RouteBufferingPolicy::Off => details.response_buffering_off_routes += 1,
        }

        match route.compression {
            rginx_core::RouteCompressionPolicy::Auto => details.compression_auto_routes += 1,
            rginx_core::RouteCompressionPolicy::Off => details.compression_off_routes += 1,
            rginx_core::RouteCompressionPolicy::Force => details.compression_force_routes += 1,
        }

        if route.compression_min_bytes.is_some() {
            details.custom_compression_min_bytes_routes += 1;
        }
        if !route.compression_content_types.is_empty() {
            details.custom_compression_content_types_routes += 1;
        }
        if route.streaming_response_idle_timeout.is_some() {
            details.streaming_response_idle_timeout_routes += 1;
        }
        if route.cache.is_some() {
            details.cache_enabled_routes += 1;
        }
    }

    details
}

fn all_routes(
    config: &rginx_config::ConfigSnapshot,
) -> impl Iterator<Item = &rginx_core::Route> + '_ {
    std::iter::once(&config.default_vhost)
        .chain(config.vhosts.iter())
        .flat_map(|vhost| vhost.routes.iter())
}
