use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use bytes::{Bytes, BytesMut};
use http::{HeaderMap, HeaderName, HeaderValue};

use crate::handler::BoxError;

use super::observability::GrpcObservability;

#[derive(Debug, Default)]
pub(in crate::handler) struct GrpcWebObservabilityParser {
    is_text: bool,
    encoded_carryover: BytesMut,
    buffer: BytesMut,
    saw_trailers: bool,
    disabled: bool,
}

impl GrpcWebObservabilityParser {
    pub(in crate::handler) fn for_protocol(protocol: &str) -> Option<Self> {
        match protocol {
            "grpc-web" => Some(Self { is_text: false, ..Self::default() }),
            "grpc-web-text" => Some(Self { is_text: true, ..Self::default() }),
            _ => None,
        }
    }

    pub(in crate::handler) fn observe_chunk(&mut self, data: &[u8], grpc: &mut GrpcObservability) {
        if self.disabled {
            return;
        }

        let result = if self.is_text {
            decode_grpc_web_text_observability_chunk(&mut self.encoded_carryover, data).and_then(
                |decoded| {
                    if let Some(decoded) = decoded {
                        self.observe_binary_chunk(&decoded, false, grpc)
                    } else {
                        Ok(())
                    }
                },
            )
        } else {
            self.observe_binary_chunk(data, false, grpc)
        };

        if let Err(error) = result {
            self.disabled = true;
            tracing::debug!(
                protocol = %grpc.protocol,
                service = %grpc.service,
                method = %grpc.method,
                %error,
                "failed to parse grpc-web response trailers for observability"
            );
        }
    }

    pub(in crate::handler) fn finish(&mut self, grpc: &mut GrpcObservability) {
        if self.disabled {
            return;
        }

        let result = if self.is_text {
            decode_grpc_web_text_observability_final(&mut self.encoded_carryover).and_then(
                |decoded| {
                    if let Some(decoded) = decoded {
                        self.observe_binary_chunk(&decoded, true, grpc)?;
                    } else {
                        self.observe_binary_chunk(&[], true, grpc)?;
                    }
                    Ok(())
                },
            )
        } else {
            self.observe_binary_chunk(&[], true, grpc)
        };

        if let Err(error) = result {
            self.disabled = true;
            tracing::debug!(
                protocol = %grpc.protocol,
                service = %grpc.service,
                method = %grpc.method,
                %error,
                "failed to finish grpc-web response trailer parsing for observability"
            );
        }
    }

    fn observe_binary_chunk(
        &mut self,
        data: &[u8],
        inner_finished: bool,
        grpc: &mut GrpcObservability,
    ) -> Result<(), BoxError> {
        if !data.is_empty() {
            self.buffer.extend_from_slice(data);
        }

        loop {
            let Some(frame) = decode_grpc_web_observability_frame(
                &mut self.buffer,
                inner_finished,
                self.saw_trailers,
            )?
            else {
                return Ok(());
            };

            match frame {
                ParsedGrpcWebObservabilityFrame::Data => {}
                ParsedGrpcWebObservabilityFrame::Trailers(trailers) => {
                    self.saw_trailers = true;
                    grpc.update_from_headers(&trailers);
                }
            }
        }
    }
}

enum ParsedGrpcWebObservabilityFrame {
    Data,
    Trailers(HeaderMap),
}

fn decode_grpc_web_observability_frame(
    buffer: &mut BytesMut,
    inner_finished: bool,
    saw_grpc_web_trailers: bool,
) -> Result<Option<ParsedGrpcWebObservabilityFrame>, BoxError> {
    if buffer.is_empty() {
        return Ok(None);
    }

    if saw_grpc_web_trailers {
        return Err(invalid_grpc_observability("grpc-web response trailer frame must be terminal"));
    }

    if buffer.len() < 5 {
        if inner_finished {
            return Err(invalid_grpc_observability("incomplete grpc-web response frame header"));
        }
        return Ok(None);
    }

    let flags = buffer[0];
    let len = u32::from_be_bytes([buffer[1], buffer[2], buffer[3], buffer[4]]) as usize;
    let frame_len = 5usize.saturating_add(len);
    if buffer.len() < frame_len {
        if inner_finished {
            return Err(invalid_grpc_observability("incomplete grpc-web response frame payload"));
        }
        return Ok(None);
    }

    let frame = buffer.split_to(frame_len).freeze();
    if (flags & 0x80) != 0 {
        Ok(Some(ParsedGrpcWebObservabilityFrame::Trailers(
            decode_grpc_web_trailer_block_for_observability(&frame[5..])?,
        )))
    } else {
        Ok(Some(ParsedGrpcWebObservabilityFrame::Data))
    }
}

fn decode_grpc_web_trailer_block_for_observability(payload: &[u8]) -> Result<HeaderMap, BoxError> {
    let mut trailers = HeaderMap::new();

    for line in payload.split(|byte| *byte == b'\n') {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if line.is_empty() {
            continue;
        }

        let Some(separator) = line.iter().position(|byte| *byte == b':') else {
            return Err(invalid_grpc_observability("grpc-web trailer line should contain ':'"));
        };
        let (name, value) = line.split_at(separator);
        let value = &value[1..];
        let name = HeaderName::from_bytes(name).map_err(|_| {
            invalid_grpc_observability("grpc-web trailer name should be a valid HTTP header")
        })?;
        let value = std::str::from_utf8(value)
            .map_err(|_| {
                invalid_grpc_observability("grpc-web trailer value should be valid utf-8")
            })?
            .trim();
        let value = HeaderValue::from_str(value).map_err(|_| {
            invalid_grpc_observability("grpc-web trailer value should be a valid HTTP header")
        })?;
        trailers.append(name, value);
    }

    Ok(trailers)
}

fn decode_grpc_web_text_observability_chunk(
    carryover: &mut BytesMut,
    data: &[u8],
) -> Result<Option<Bytes>, BoxError> {
    for byte in data {
        if !byte.is_ascii_whitespace() {
            carryover.extend_from_slice(&[*byte]);
        }
    }

    let complete_len = carryover.len() / 4 * 4;
    if complete_len == 0 {
        return Ok(None);
    }

    let encoded = carryover.split_to(complete_len);
    let mut decoded = Vec::new();
    for quantum in encoded.chunks_exact(4) {
        let chunk = STANDARD.decode(quantum).map_err(|error| {
            invalid_grpc_observability(&format!(
                "invalid grpc-web-text base64 chunk for observability: {error}"
            ))
        })?;
        decoded.extend_from_slice(&chunk);
    }

    Ok((!decoded.is_empty()).then(|| Bytes::from(decoded)))
}

pub(in crate::handler) fn decode_grpc_web_text_observability_final(
    carryover: &mut BytesMut,
) -> Result<Option<Bytes>, BoxError> {
    if carryover.is_empty() {
        return Ok(None);
    }

    if !carryover.len().is_multiple_of(4) {
        return Err(invalid_grpc_observability("incomplete grpc-web-text base64 response body"));
    }

    let encoded = carryover.split();
    let mut decoded = Vec::new();
    for quantum in encoded.chunks_exact(4) {
        let chunk = STANDARD.decode(quantum).map_err(|error| {
            invalid_grpc_observability(&format!(
                "invalid grpc-web-text base64 chunk for observability: {error}"
            ))
        })?;
        decoded.extend_from_slice(&chunk);
    }

    Ok((!decoded.is_empty()).then(|| Bytes::from(decoded)))
}

fn invalid_grpc_observability(message: &str) -> BoxError {
    std::io::Error::new(std::io::ErrorKind::InvalidData, message).into()
}
