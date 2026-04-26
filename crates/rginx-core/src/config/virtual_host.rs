use super::{Route, ServerNameMatch, VirtualHostTls, best_matching_server_name_pattern};

#[derive(Debug, Clone)]
pub struct VirtualHost {
    pub id: String,
    pub server_names: Vec<String>,
    pub routes: Vec<Route>,
    pub tls: Option<VirtualHostTls>,
}

impl VirtualHost {
    pub fn matches_host(&self, host: &str) -> bool {
        self.server_names.is_empty() || self.best_server_name_match(host).is_some()
    }

    pub fn best_server_name_match(&self, host: &str) -> Option<ServerNameMatch> {
        best_matching_server_name_pattern(self.server_names.iter().map(String::as_str), host)
            .map(|(_, matched)| matched)
    }
}
