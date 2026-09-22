use tauri::http::{header, Request, Response, StatusCode};
use url::Url;

use super::{
    contracts::{CSP, PERMISSIONS_POLICY, PREVIEW_SCHEME},
    policy,
    tokens::PreviewRuntime,
};

pub(crate) fn respond(runtime: &PreviewRuntime, request: Request<Vec<u8>>) -> Response<Vec<u8>> {
    if request.method() != tauri::http::Method::GET && request.method() != tauri::http::Method::HEAD
    {
        runtime.record_denial("method-not-allowed");
        return deny(StatusCode::METHOD_NOT_ALLOWED);
    }
    let uri = request.uri().to_string();
    let Some(token) = token_from_uri(&uri) else {
        runtime.record_denial("path-invalid");
        return deny(StatusCode::NOT_FOUND);
    };
    match runtime.lookup_active(&token) {
        Ok(entry) => {
            let Some(revision) = super::catalog::lookup(&entry.artifact_id, &entry.revision_id)
            else {
                runtime.record_denial("path-invalid");
                return deny(StatusCode::NOT_FOUND);
            };
            if revision.scope != entry.scope || revision.digest() != entry.digest {
                runtime.record_denial("path-invalid");
                return deny(StatusCode::NOT_FOUND);
            }
            if let Err(_reason) =
                super::service::validate_revision_bytes(revision.payload, &entry.digest)
            {
                runtime.record_denial("path-invalid");
                return deny(StatusCode::NOT_FOUND);
            }
            html_response(
                revision.payload.to_vec(),
                request.method() == tauri::http::Method::HEAD,
            )
        }
        Err(reason) => {
            runtime.record_denial(policy::sanitize_reason(reason));
            deny(StatusCode::NOT_FOUND)
        }
    }
}

pub(crate) fn token_from_uri(uri: &str) -> Option<String> {
    Url::parse(uri).ok().and_then(|url| token_from_url(&url))
}

pub(crate) fn token_from_url(url: &Url) -> Option<String> {
    if url.query().is_some() {
        return None;
    }
    let path = url.path().trim_end_matches('/');
    let host = url.host_str().unwrap_or_default();
    match url.scheme() {
        PREVIEW_SCHEME => match host {
            "preview" => file_token(path),
            "localhost" => file_token(path.strip_prefix("/preview").unwrap_or("")),
            _ => None,
        },
        "http" if host == format!("{PREVIEW_SCHEME}.localhost") => {
            file_token(path.strip_prefix("/preview")?)
        }
        _ => None,
    }
}

fn file_token(path: &str) -> Option<String> {
    let path = path.trim_start_matches('/');
    if path.contains("..") || path.contains('\\') {
        return None;
    }
    let (token, file) = path.split_once('/')?;
    if file != "index.html" {
        return None;
    }
    if token.len() != 64 || !token.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    Some(token.to_ascii_lowercase())
}

fn html_response(body: Vec<u8>, head: bool) -> Response<Vec<u8>> {
    let mut builder = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .header(header::CACHE_CONTROL, "no-store")
        .header("X-Content-Type-Options", "nosniff")
        .header("Referrer-Policy", "no-referrer")
        .header("Content-Security-Policy", CSP)
        .header("Permissions-Policy", PERMISSIONS_POLICY);
    if head {
        builder = builder.header(header::CONTENT_LENGTH, body.len());
        return builder
            .body(Vec::new())
            .unwrap_or_else(|_| deny(StatusCode::INTERNAL_SERVER_ERROR));
    }
    builder
        .body(body)
        .unwrap_or_else(|_| deny(StatusCode::INTERNAL_SERVER_ERROR))
}

fn deny(status: StatusCode) -> Response<Vec<u8>> {
    Response::builder()
        .status(status)
        .header(header::CACHE_CONTROL, "no-store")
        .header("X-Content-Type-Options", "nosniff")
        .body(Vec::new())
        .unwrap_or_else(|_| Response::new(Vec::new()))
}
