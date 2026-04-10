use super::*;

mod body;
mod codec;

pub(super) use body::{
    GrpcWebRequestBody, GrpcWebResponseBody, GrpcWebTextDecodeBody, GrpcWebTextEncodeBody,
};
pub(super) use codec::extract_grpc_initial_trailers;
#[cfg(test)]
pub(super) use codec::{
    decode_grpc_web_text_chunk, decode_grpc_web_text_final, encode_grpc_web_text_chunk,
    encode_grpc_web_trailers, flush_grpc_web_text_chunk,
};

#[derive(Debug, Clone)]
pub(crate) struct GrpcWebMode {
    pub downstream_content_type: HeaderValue,
    pub upstream_content_type: HeaderValue,
    pub encoding: GrpcWebEncoding,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GrpcWebEncoding {
    Binary,
    Text,
}

impl GrpcWebMode {
    pub(super) fn is_text(&self) -> bool {
        self.encoding == GrpcWebEncoding::Text
    }
}
