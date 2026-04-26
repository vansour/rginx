use std::borrow::Cow;

use rginx_core::{Route, RouteBufferingPolicy, RouteCompressionPolicy};

const MIN_COMPRESSIBLE_RESPONSE_BYTES: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ResponseCompressionOptions<'a> {
    pub(crate) response_buffering: RouteBufferingPolicy,
    pub(crate) compression: RouteCompressionPolicy,
    pub(crate) compression_min_bytes: Option<usize>,
    pub(crate) compression_content_types: Cow<'a, [String]>,
}

impl Default for ResponseCompressionOptions<'_> {
    fn default() -> Self {
        Self {
            response_buffering: RouteBufferingPolicy::Auto,
            compression: RouteCompressionPolicy::Auto,
            compression_min_bytes: None,
            compression_content_types: Cow::Borrowed(&[]),
        }
    }
}

impl<'a> ResponseCompressionOptions<'a> {
    pub(crate) fn for_route(route: &'a Route) -> Self {
        Self {
            response_buffering: route.response_buffering,
            compression: route.compression,
            compression_min_bytes: route.compression_min_bytes,
            compression_content_types: Cow::Borrowed(route.compression_content_types.as_slice()),
        }
    }

    pub(crate) fn min_bytes(&self) -> usize {
        match self.compression {
            RouteCompressionPolicy::Force => self.compression_min_bytes.unwrap_or(1),
            RouteCompressionPolicy::Auto | RouteCompressionPolicy::Off => {
                self.compression_min_bytes.unwrap_or(MIN_COMPRESSIBLE_RESPONSE_BYTES)
            }
        }
    }

    pub(crate) fn response_compression_disabled(&self) -> bool {
        self.compression == RouteCompressionPolicy::Off
            || self.response_buffering == RouteBufferingPolicy::Off
    }
}
