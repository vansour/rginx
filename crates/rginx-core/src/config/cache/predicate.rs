use super::*;

impl CachePredicate {
    pub fn matches_request(&self, request: &CachePredicateRequestContext<'_>) -> bool {
        match self {
            Self::Any(conditions) => {
                conditions.iter().any(|condition| condition.matches_request(request))
            }
            Self::All(conditions) => {
                conditions.iter().all(|condition| condition.matches_request(request))
            }
            Self::Not(condition) => !condition.matches_request(request),
            Self::Method(method) => method == request.method,
            Self::HeaderExists(name) => request.headers.contains_key(name),
            Self::HeaderEquals { name, value } => request
                .headers
                .get_all(name)
                .iter()
                .filter_map(|header| header.to_str().ok())
                .any(|header| header == value),
            Self::QueryExists(name) => query_pairs(request.uri).any(|(key, _)| key == name),
            Self::QueryEquals { name, value } => {
                query_pairs(request.uri).any(|(key, candidate)| key == name && candidate == value)
            }
            Self::CookieExists(name) => cookie_pairs(request.headers).any(|(key, _)| key == name),
            Self::CookieEquals { name, value } => cookie_pairs(request.headers)
                .any(|(key, candidate)| key == name && candidate == value),
            Self::Status(_) => false,
        }
    }

    pub fn matches_response(
        &self,
        request: &CachePredicateRequestContext<'_>,
        status: StatusCode,
    ) -> bool {
        match self {
            Self::Any(conditions) => {
                conditions.iter().any(|condition| condition.matches_response(request, status))
            }
            Self::All(conditions) => {
                conditions.iter().all(|condition| condition.matches_response(request, status))
            }
            Self::Not(condition) => !condition.matches_response(request, status),
            Self::Status(statuses) => statuses.contains(&status),
            _ => self.matches_request(request),
        }
    }
}

pub(super) fn query_pairs(uri: &str) -> impl Iterator<Item = (&str, &str)> {
    uri.split_once('?')
        .map(|(_, query)| query.split('&'))
        .into_iter()
        .flatten()
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
            (name, value)
        })
}

pub(super) fn cookie_pairs(headers: &HeaderMap) -> impl Iterator<Item = (&str, &str)> {
    headers
        .get_all(http::header::COOKIE)
        .iter()
        .filter_map(|header| header.to_str().ok())
        .flat_map(|header| header.split(';'))
        .map(str::trim)
        .filter(|cookie| !cookie.is_empty())
        .map(|cookie| {
            let (name, value) = cookie.split_once('=').unwrap_or((cookie, ""));
            (name.trim(), value.trim())
        })
}
