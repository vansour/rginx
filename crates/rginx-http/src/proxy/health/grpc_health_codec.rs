use bytes::{Bytes, BytesMut};
use http::{HeaderMap, HeaderName, Response, StatusCode};
use http_body_util::BodyExt;

use crate::handler::BoxError;

const GRPC_HEALTH_SERVING_STATUS_SERVING: u64 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GrpcHealthServingStatus {
    Unknown,
    Serving,
    NotServing,
    ServiceUnknown,
    Other(u64),
}

impl GrpcHealthServingStatus {
    fn from_u64(value: u64) -> Self {
        match value {
            0 => Self::Unknown,
            GRPC_HEALTH_SERVING_STATUS_SERVING => Self::Serving,
            2 => Self::NotServing,
            3 => Self::ServiceUnknown,
            other => Self::Other(other),
        }
    }

    pub(crate) fn is_serving(self) -> bool {
        matches!(self, Self::Serving)
    }
}

#[derive(Debug)]
pub(crate) enum GrpcHealthProbeResult {
    Serving,
    NotServing {
        http_status: StatusCode,
        grpc_status: Option<String>,
        serving_status: Option<GrpcHealthServingStatus>,
    },
}

pub(crate) async fn evaluate_grpc_health_probe_response<B>(
    response: Response<B>,
) -> Result<GrpcHealthProbeResult, BoxError>
where
    B: hyper::body::Body<Data = Bytes> + Unpin,
    B::Error: Into<BoxError>,
{
    let (parts, body) = response.into_parts();
    let response_headers = parts.headers;
    let http_status = parts.status;
    let (body, trailers) = collect_response_body_and_trailers(body).await?;
    let grpc_status = grpc_trailer_value(&response_headers, trailers.as_ref(), "grpc-status");
    let serving_status = decode_grpc_health_check_response(body.as_ref())?;

    Ok(
        if http_status.is_success()
            && grpc_status.as_deref() == Some("0")
            && serving_status.is_some_and(GrpcHealthServingStatus::is_serving)
        {
            GrpcHealthProbeResult::Serving
        } else {
            GrpcHealthProbeResult::NotServing { http_status, grpc_status, serving_status }
        },
    )
}

async fn collect_response_body_and_trailers<B>(
    mut body: B,
) -> Result<(Bytes, Option<HeaderMap>), BoxError>
where
    B: hyper::body::Body<Data = Bytes> + Unpin,
    B::Error: Into<BoxError>,
{
    let mut collected = BytesMut::new();
    let mut trailers = None;

    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(Into::<BoxError>::into)?;
        let frame = match frame.into_data() {
            Ok(data) => {
                collected.extend_from_slice(&data);
                continue;
            }
            Err(frame) => frame,
        };

        let frame_trailers = match frame.into_trailers() {
            Ok(trailers) => trailers,
            Err(_) => continue,
        };
        if let Some(existing) = trailers.as_mut() {
            super::super::append_header_map(existing, &frame_trailers);
        } else {
            trailers = Some(frame_trailers);
        }
    }

    Ok((collected.freeze(), trailers))
}

fn grpc_trailer_value(
    headers: &HeaderMap,
    trailers: Option<&HeaderMap>,
    name: &str,
) -> Option<String> {
    let name = HeaderName::from_bytes(name.as_bytes()).ok()?;
    trailers
        .and_then(|trailers| trailers.get(&name))
        .or_else(|| headers.get(&name))
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
}

pub(crate) fn encode_grpc_health_check_request(service: &str) -> Bytes {
    let mut payload = BytesMut::new();
    if !service.is_empty() {
        payload.extend_from_slice(&[0x0a]);
        append_protobuf_varint(&mut payload, service.len() as u64);
        payload.extend_from_slice(service.as_bytes());
    }

    let mut frame = BytesMut::with_capacity(5 + payload.len());
    frame.extend_from_slice(&[0]);
    frame.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    frame.extend_from_slice(&payload);
    frame.freeze()
}

pub(crate) fn decode_grpc_health_check_response(
    body: &[u8],
) -> Result<Option<GrpcHealthServingStatus>, BoxError> {
    if body.is_empty() {
        return Ok(None);
    }

    let payload = decode_grpc_frame_payload(body)?;
    Ok(Some(decode_grpc_health_check_response_payload(payload)?))
}

fn decode_grpc_frame_payload(body: &[u8]) -> Result<&[u8], BoxError> {
    if body.len() < 5 {
        return Err(invalid_grpc_health_probe("incomplete gRPC health response frame header"));
    }

    let compressed = body[0];
    if compressed != 0 {
        return Err(invalid_grpc_health_probe(
            "compressed gRPC health responses are not supported",
        ));
    }

    let len = u32::from_be_bytes([body[1], body[2], body[3], body[4]]) as usize;
    let expected_len = 5 + len;
    if body.len() != expected_len {
        return Err(invalid_grpc_health_probe(format!(
            "gRPC health response frame length mismatch: expected {expected_len} bytes, got {}",
            body.len()
        )));
    }

    Ok(&body[5..])
}

fn decode_grpc_health_check_response_payload(
    payload: &[u8],
) -> Result<GrpcHealthServingStatus, BoxError> {
    let mut index = 0usize;
    let mut serving_status = GrpcHealthServingStatus::Unknown;

    while index < payload.len() {
        let tag = decode_protobuf_varint(payload, &mut index)?;
        let field_number = tag >> 3;
        let wire_type = (tag & 0x07) as u8;

        match (field_number, wire_type) {
            (1, 0) => {
                serving_status =
                    GrpcHealthServingStatus::from_u64(decode_protobuf_varint(payload, &mut index)?);
            }
            _ => skip_protobuf_field(payload, &mut index, wire_type)?,
        }
    }

    Ok(serving_status)
}

fn append_protobuf_varint(buffer: &mut BytesMut, mut value: u64) {
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        buffer.extend_from_slice(&[byte]);
        if value == 0 {
            break;
        }
    }
}

fn decode_protobuf_varint(payload: &[u8], index: &mut usize) -> Result<u64, BoxError> {
    let mut value = 0u64;
    let mut shift = 0u32;

    while *index < payload.len() {
        let byte = payload[*index];
        *index += 1;
        value |= u64::from(byte & 0x7f) << shift;

        if byte & 0x80 == 0 {
            return Ok(value);
        }

        shift += 7;
        if shift >= 64 {
            return Err(invalid_grpc_health_probe("protobuf varint is too large"));
        }
    }

    Err(invalid_grpc_health_probe("unexpected EOF while decoding protobuf varint"))
}

fn skip_protobuf_field(payload: &[u8], index: &mut usize, wire_type: u8) -> Result<(), BoxError> {
    match wire_type {
        0 => {
            let _ = decode_protobuf_varint(payload, index)?;
        }
        1 => {
            let end = index.saturating_add(8);
            if end > payload.len() {
                return Err(invalid_grpc_health_probe(
                    "unexpected EOF while skipping fixed64 protobuf field",
                ));
            }
            *index = end;
        }
        2 => {
            let len = usize::try_from(decode_protobuf_varint(payload, index)?).map_err(|_| {
                invalid_grpc_health_probe("length-delimited protobuf field exceeds platform limits")
            })?;
            let end = index.saturating_add(len);
            if end > payload.len() {
                return Err(invalid_grpc_health_probe(
                    "unexpected EOF while skipping length-delimited protobuf field",
                ));
            }
            *index = end;
        }
        5 => {
            let end = index.saturating_add(4);
            if end > payload.len() {
                return Err(invalid_grpc_health_probe(
                    "unexpected EOF while skipping fixed32 protobuf field",
                ));
            }
            *index = end;
        }
        _ => {
            return Err(invalid_grpc_health_probe(format!(
                "unsupported protobuf wire type `{wire_type}` in gRPC health response"
            )));
        }
    }

    Ok(())
}

fn invalid_grpc_health_probe(message: impl Into<String>) -> BoxError {
    std::io::Error::new(std::io::ErrorKind::InvalidData, message.into()).into()
}
