use super::super::*;

pub(super) struct TrafficCounterRefs {
    pub(super) listener: Option<Arc<ListenerTrafficCounters>>,
    pub(super) vhost: Option<Arc<RequestTrafficCounters>>,
    pub(super) route: Option<Arc<RouteTrafficCounters>>,
}
