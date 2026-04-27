use regex::{Regex, RegexBuilder};
use thiserror::Error;

#[derive(Debug, Clone)]
pub struct RouteRegexMatcher {
    pattern: String,
    case_insensitive: bool,
    regex: Regex,
}

impl RouteRegexMatcher {
    pub const SIZE_LIMIT_BYTES: usize = 1 << 20;

    pub fn new(pattern: String, case_insensitive: bool) -> Result<Self, RouteRegexError> {
        let regex = RegexBuilder::new(&pattern)
            .case_insensitive(case_insensitive)
            .size_limit(Self::SIZE_LIMIT_BYTES)
            .build()
            .map_err(|source| RouteRegexError::InvalidPattern {
                pattern: pattern.clone(),
                source,
            })?;

        Ok(Self { pattern, case_insensitive, regex })
    }

    pub fn pattern(&self) -> &str {
        &self.pattern
    }

    pub fn case_insensitive(&self) -> bool {
        self.case_insensitive
    }

    pub fn matches(&self, path: &str) -> bool {
        self.regex.is_match(path)
    }
}

impl PartialEq for RouteRegexMatcher {
    fn eq(&self, other: &Self) -> bool {
        self.pattern == other.pattern && self.case_insensitive == other.case_insensitive
    }
}

impl Eq for RouteRegexMatcher {}

#[derive(Debug, Error)]
pub enum RouteRegexError {
    #[error("route regex pattern `{pattern}` is invalid: {source}")]
    InvalidPattern {
        pattern: String,
        #[source]
        source: regex::Error,
    },
}
