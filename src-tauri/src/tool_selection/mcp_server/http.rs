#![allow(clippy::result_large_err)]
//! HTTP-boundary helpers for the published MCP endpoint: bearer authentication, media-type
//! checks, bounded body reads and the small response constructors. Split from the JSON-RPC
//! dispatch so the router only owns the method contract.

use axum::body::Body;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::Value;

use super::{ServerInner, BODY_MAX_BYTES, BODY_READ_TIMEOUT};

/// Refuses a browser Origin, requires the exact loopback Host and authenticates the bearer token.
/// Failure is a bare HTTP status; no session or ledger access happens before this succeeds.
pub fn authenticate(inner: &ServerInner, headers: &HeaderMap) -> Result<(), Response> {
    // A browser-supplied Origin is always refused: this endpoint is not CORS-enabled.
    if headers.contains_key(header::ORIGIN) {
        return Err(empty(StatusCode::FORBIDDEN));
    }
    // Only the exact loopback listener host is accepted.
    match header_str(headers, "host") {
        Some(host) if host == inner.host => {}
        _ => return Err(empty(StatusCode::FORBIDDEN)),
    }
    let Some(value) = header_str(headers, "authorization") else {
        return Err(empty(StatusCode::UNAUTHORIZED));
    };
    // The auth scheme is case-insensitive (RFC 7235); the token itself stays case-sensitive.
    let Some((scheme, token)) = value.split_once(' ') else {
        return Err(empty(StatusCode::UNAUTHORIZED));
    };
    if !scheme.eq_ignore_ascii_case("Bearer") {
        return Err(empty(StatusCode::UNAUTHORIZED));
    }
    if !constant_time_eq(token.as_bytes(), inner.token.as_bytes()) {
        return Err(empty(StatusCode::UNAUTHORIZED));
    }
    Ok(())
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    // Hash both sides to a fixed size first: the variable-length comparison would otherwise
    // return early on a length mismatch and leak the length of the server token.
    use sha2::{Digest, Sha256};
    let left = Sha256::digest(left);
    let right = Sha256::digest(right);
    let mut difference = 0u8;
    for (left, right) in left.iter().zip(right.iter()) {
        difference |= left ^ right;
    }
    difference == 0
}

pub fn header_str<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|value| value.to_str().ok())
}

pub fn content_type_is_json(headers: &HeaderMap) -> bool {
    let Some(value) = header_str(headers, "content-type") else {
        return false;
    };
    let media = value.split(';').next().unwrap_or("").trim();
    media.eq_ignore_ascii_case("application/json")
}

pub fn accept_is_supported(headers: &HeaderMap) -> bool {
    let Some(value) = header_str(headers, "accept") else {
        return false;
    };
    accepts_media(value, "application/json") && accepts_media(value, "text/event-stream")
}

fn accepts_media(value: &str, media: &str) -> bool {
    value.split(',').any(|part| {
        let token = part.split(';').next().unwrap_or("").trim();
        token.eq_ignore_ascii_case(media)
            || token == "*/*"
            || (media.starts_with("application/") && token.eq_ignore_ascii_case("application/*"))
    })
}

pub async fn read_body(body: Body) -> Result<axum::body::Bytes, Response> {
    match tokio::time::timeout(
        BODY_READ_TIMEOUT,
        axum::body::to_bytes(body, BODY_MAX_BYTES),
    )
    .await
    {
        Ok(Ok(bytes)) => Ok(bytes),
        Ok(Err(_)) => Err(empty(StatusCode::PAYLOAD_TOO_LARGE)),
        Err(_) => Err(empty(StatusCode::REQUEST_TIMEOUT)),
    }
}

pub fn empty(status: StatusCode) -> Response {
    status.into_response()
}

pub fn json_response(status: StatusCode, body: &Value) -> Response {
    (
        status,
        [(header::CONTENT_TYPE, "application/json")],
        body.to_string(),
    )
        .into_response()
}
