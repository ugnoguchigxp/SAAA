//! Loopback session checks. The cookie is not a provider token.
use axum::{
    http::{HeaderMap, Method, StatusCode},
    Json,
};
use serde_json::{json, Value};
use std::net::SocketAddr;

#[derive(Clone)]
pub struct LabAuth {
    pub token: String,
    pub origin: String,
    pub host: String,
}

pub fn authorize(
    auth: &LabAuth,
    method: &Method,
    headers: &HeaderMap,
    peer: Option<SocketAddr>,
) -> Result<(), (StatusCode, Json<Value>)> {
    if peer.is_none_or(|peer| !peer.ip().is_loopback()) {
        return Err(denied("forbidden", "この操作は許可されていません。"));
    }
    let host = headers.get("host").and_then(|value| value.to_str().ok());
    if host != Some(auth.host.as_str()) {
        return Err(denied("forbidden", "この操作は許可されていません。"));
    }
    if !cookie(headers, "saaa_lab_session").is_some_and(|value| same_token(value, &auth.token)) {
        return Err(denied(
            "unauthenticated",
            "試用セッションを確認できません。再接続してください。",
        ));
    }
    let origin = headers.get("origin").and_then(|value| value.to_str().ok());
    let site = headers
        .get("sec-fetch-site")
        .and_then(|value| value.to_str().ok());
    if site == Some("cross-site") {
        return Err(denied("forbidden", "この操作は許可されていません。"));
    }
    let changing = *method != Method::GET && *method != Method::HEAD;
    let same_origin = origin == Some(auth.origin.as_str())
        || (!changing && origin.is_none() && site == Some("same-origin"));
    if (changing && origin != Some(auth.origin.as_str())) || !same_origin {
        return Err(denied("forbidden", "この操作は許可されていません。"));
    }
    Ok(())
}

fn cookie<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    let raw = headers.get("cookie")?.to_str().ok()?;
    raw.split(';').find_map(|part| {
        let (key, value) = part.trim().split_once('=')?;
        (key == name).then_some(value)
    })
}

fn same_token(actual: &str, expected: &str) -> bool {
    let actual = actual.as_bytes();
    let expected = expected.as_bytes();
    if actual.len() != expected.len() {
        return false;
    }
    let mut diff = 0u8;
    for (left, right) in actual.iter().zip(expected.iter()) {
        diff |= left ^ right;
    }
    diff == 0
}

fn denied(code: &'static str, message: &str) -> (StatusCode, Json<Value>) {
    let status = match code {
        "unauthenticated" => StatusCode::UNAUTHORIZED,
        _ => StatusCode::FORBIDDEN,
    };
    (
        status,
        Json(json!({"error": {"code": code, "message": message}})),
    )
}
