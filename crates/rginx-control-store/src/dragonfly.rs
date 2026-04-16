#[derive(Debug, Clone)]
pub struct DragonflyKeyspace {
    prefix: String,
}

impl DragonflyKeyspace {
    pub fn new(prefix: impl Into<String>) -> Self {
        Self { prefix: prefix.into() }
    }

    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    pub fn session_key(&self, session_id: &str) -> String {
        format!("{}:session:{session_id}", self.prefix)
    }

    pub fn deployment_queue_key(&self, cluster_id: &str) -> String {
        format!("{}:deployment:queue:{cluster_id}", self.prefix)
    }

    pub fn deployment_lock_key(&self, cluster_id: &str) -> String {
        format!("{}:deployment:lock:{cluster_id}", self.prefix)
    }

    pub fn node_presence_key(&self, node_id: &str) -> String {
        format!("{}:node:presence:{node_id}", self.prefix)
    }

    pub fn event_stream_key(&self, stream_name: &str) -> String {
        format!("{}:event:stream:{stream_name}", self.prefix)
    }

    pub fn sse_fanout_key(&self, channel: &str) -> String {
        format!("{}:sse:fanout:{channel}", self.prefix)
    }
}

#[cfg(test)]
mod tests {
    use super::DragonflyKeyspace;

    #[test]
    fn keyspace_is_prefixed_consistently() {
        let keyspace = DragonflyKeyspace::new("rginx:control");

        assert_eq!(keyspace.session_key("sess_01"), "rginx:control:session:sess_01");
        assert_eq!(
            keyspace.deployment_queue_key("cluster-mainland"),
            "rginx:control:deployment:queue:cluster-mainland"
        );
        assert_eq!(
            keyspace.node_presence_key("edge-sha-01"),
            "rginx:control:node:presence:edge-sha-01"
        );
    }
}
