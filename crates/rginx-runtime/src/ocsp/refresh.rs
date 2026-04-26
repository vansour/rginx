use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::{Request, Uri};

use super::{OCSP_FETCH_TIMEOUT, OcspClient};

pub(super) async fn fetch_ocsp_response(
    client: &OcspClient,
    responder_urls: &[String],
    request_body: Vec<u8>,
) -> Result<Vec<u8>, String> {
    let mut errors = Vec::new();
    for responder_url in responder_urls {
        match fetch_ocsp_response_from_url(client, responder_url, request_body.clone()).await {
            Ok(response_body) => return Ok(response_body),
            Err(error) => errors.push(format!("{responder_url}: {error}")),
        }
    }

    Err(if errors.is_empty() {
        "no OCSP responder URLs were available".to_string()
    } else {
        errors.join("; ")
    })
}

pub(super) async fn fetch_ocsp_response_from_url(
    client: &OcspClient,
    responder_url: &str,
    request_body: Vec<u8>,
) -> Result<Vec<u8>, String> {
    let uri = responder_url
        .parse::<Uri>()
        .map_err(|error| format!("invalid OCSP responder URI: {error}"))?;
    let request = Request::post(uri)
        .header("content-type", "application/ocsp-request")
        .header("accept", "application/ocsp-response")
        .body(Full::new(Bytes::from(request_body)))
        .map_err(|error| format!("failed to build OCSP request: {error}"))?;

    let response = tokio::time::timeout(OCSP_FETCH_TIMEOUT, client.request(request))
        .await
        .map_err(|_| format!("timed out after {}s", OCSP_FETCH_TIMEOUT.as_secs()))?
        .map_err(|error| format!("request failed: {error}"))?;
    if !response.status().is_success() {
        return Err(format!("responder returned HTTP {}", response.status()));
    }

    let mut body = response.into_body();
    let mut payload = Vec::new();
    while let Some(frame) = body
        .frame()
        .await
        .transpose()
        .map_err(|error| format!("failed to read OCSP response body: {error}"))?
    {
        let Some(chunk) = frame.data_ref() else {
            continue;
        };
        if payload.len().saturating_add(chunk.len()) > rginx_http::MAX_OCSP_RESPONSE_BYTES {
            return Err(format!(
                "OCSP response exceeded {} bytes",
                rginx_http::MAX_OCSP_RESPONSE_BYTES
            ));
        }
        payload.extend_from_slice(chunk);
    }
    if payload.is_empty() {
        return Err("responder returned an empty OCSP response body".to_string());
    }

    Ok(payload)
}
