//! HTTP v1. After the response status is committed, failures stay in the NDJSON body.
use std::{
    net::SocketAddr,
    sync::{atomic::AtomicUsize, Arc},
};

use axum::{
    body::Body,
    extract::{ConnectInfo, DefaultBodyLimit, Path, Request, State},
    http::{header, HeaderMap, Method, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use futures_util::stream;
use saaa_larm_session::media::MediaKind;
use saaa_media::{
    ExplicitSecrets, FixedAvailability, GenerateInput, HistoryQuery, LiveBackend, MediaHostError,
    MediaService, RunHandle, RunTerminal, SqlStore, SystemClock,
};
use saaa_provider_routing::LarmReachability;
use serde_json::{json, Value};

use crate::{
    auth::{authorize, LabAuth},
    fixture::FixtureBackend,
    owner::OwnerGuard,
    store::{load_snapshot, LabOpen},
};

const BODY_LIMIT: usize = 64 * 1024;
const LINE_LIMIT: usize = 1024 * 1024;

#[derive(Clone)]
pub struct LabHttp {
    pub service: Arc<MediaService>,
    pub auth: LabAuth,
    pub posts: Arc<AtomicUsize>,
}

pub struct RunningLab {
    pub http: LabHttp,
    owner: OwnerGuard,
}

impl RunningLab {
    pub fn into_parts(self) -> (LabHttp, OwnerGuard) {
        (self.http, self.owner)
    }
}

pub fn open_service(open: LabOpen, host: String) -> Result<RunningLab, String> {
    let LabOpen {
        connection,
        config,
        owner,
    } = open;
    let posts = Arc::new(AtomicUsize::new(0));
    let store = Arc::new(SqlStore::new(
        saaa_media::MutexDb::new(connection),
        Arc::new(load_snapshot),
    ));
    let secrets = Arc::new(
        ExplicitSecrets::new(config.larm_token.clone(), Vec::new())
            .map_err(|error| error.message)?,
    );
    let backend: Arc<dyn saaa_media::MediaBackend> = match config.provider.as_str() {
        "larm" => {
            if config.larm_token.as_deref().unwrap_or("").is_empty() {
                return Err("feature-lab larm provider needs an explicit token".into());
            }
            Arc::new(LiveBackend::new(store.clone(), secrets))
        }
        "fixture" => Arc::new(FixtureBackend::counting(posts.clone())),
        _ => return Err("feature-lab provider must be fixture or larm".into()),
    };
    let service = Arc::new(MediaService::new(
        store,
        Arc::new(FixedAvailability(LarmReachability::Reachable)),
        Arc::new(SystemClock),
        backend,
    ));
    service
        .reconcile_interrupted()
        .map_err(|error| error.message)?;
    Ok(RunningLab {
        http: LabHttp {
            service,
            auth: LabAuth {
                token: config.session_token,
                origin: config.allowed_origin,
                host,
            },
            posts,
        },
        owner,
    })
}

pub fn router(lab: LabHttp) -> Router {
    Router::new()
        .route("/api/v1/media/runs", post(submit).get(history))
        .route("/api/v1/media/runs/{run_id}/cancel", post(cancel))
        .route("/api/v1/media/runs/{run_id}/reconcile", post(reconcile))
        .route(
            "/api/v1/media/runs/{run_id}/artifacts/{index}",
            get(artifact),
        )
        .layer(DefaultBodyLimit::disable())
        .with_state(lab)
}

pub fn listen_host(port: u16) -> String {
    format!("127.0.0.1:{port}")
}

pub fn authorized_host(origin: &str) -> Result<String, String> {
    let rest = origin
        .strip_prefix("http://")
        .or_else(|| origin.strip_prefix("https://"))
        .ok_or_else(|| "feature-lab origin must include a scheme".to_string())?;
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    if !loopback_host(host) {
        return Err("feature-lab origin must be loopback".into());
    }
    Ok(host.to_string())
}

fn loopback_host(host: &str) -> bool {
    if host == "127.0.0.1" || host == "[::1]" {
        return true;
    }
    let port = host
        .strip_prefix("127.0.0.1:")
        .or_else(|| host.strip_prefix("[::1]:"));
    port.is_some_and(|port| !port.is_empty() && port.parse::<u16>().is_ok())
}

async fn submit(State(lab): State<LabHttp>, request: Request) -> Response {
    if let Some(error) = gate(
        &lab,
        request.method(),
        request.headers(),
        request.extensions(),
    ) {
        return error;
    }
    let headers = request.headers().clone();
    if content_length_too_large(&headers) {
        return payload_too_large();
    }
    if !json_content(&headers) {
        return error_response(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "unsupported_media",
            "JSONで送信してください。",
        );
    }
    let Ok(bytes) = read_limited(request).await else {
        return payload_too_large();
    };
    let input: GenerateInput = match serde_json::from_slice(&bytes) {
        Ok(input) => input,
        Err(_) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "invalid_input",
                "生成要求を確認してください。",
            );
        }
    };
    if input.kind != MediaKind::Image {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_input",
            "試用では画像生成だけを受け付けます。",
        );
    }
    match lab.service.submit(input).await {
        Ok(handle) => stream_response(handle),
        Err(error) => host_error(error),
    }
}

async fn history(State(lab): State<LabHttp>, request: Request) -> Response {
    if let Some(error) = gate(
        &lab,
        request.method(),
        request.headers(),
        request.extensions(),
    ) {
        return error;
    }
    let raw = request.uri().query().unwrap_or("").to_string();
    if read_limited(request).await.is_err() {
        return payload_too_large();
    }
    let run_id = match query_run_id(&raw) {
        Ok(run_id) => run_id,
        Err(()) => return invalid_query(),
    };
    let history_query = match run_id {
        Some(run_id) => HistoryQuery::ByRunId(run_id),
        None => HistoryQuery::Latest { limit: 20 },
    };
    match lab.service.history(&history_query) {
        Ok(rows) => Json(rows).into_response(),
        Err(error) => host_error(error),
    }
}

async fn cancel(
    State(lab): State<LabHttp>,
    Path(run_id): Path<String>,
    request: Request,
) -> Response {
    if let Some(error) = gate(
        &lab,
        request.method(),
        request.headers(),
        request.extensions(),
    ) {
        return error;
    }
    if read_limited(request).await.is_err() {
        return payload_too_large();
    }
    match lab.service.cancel(&run_id).await {
        Ok(outcome) => {
            let status = match outcome.status {
                saaa_media::CancelStatus::Accepted => "accepted",
                saaa_media::CancelStatus::AlreadyTerminal => "alreadyTerminal",
            };
            (
                StatusCode::ACCEPTED,
                Json(json!({"runId": run_id, "status": status})),
            )
                .into_response()
        }
        Err(error) => host_error(error),
    }
}

async fn reconcile(
    State(lab): State<LabHttp>,
    Path(run_id): Path<String>,
    request: Request,
) -> Response {
    if let Some(error) = gate(
        &lab,
        request.method(),
        request.headers(),
        request.extensions(),
    ) {
        return error;
    }
    if read_limited(request).await.is_err() {
        return payload_too_large();
    }
    match lab.service.reconcile(&run_id).await {
        Ok(handle) => stream_response(handle),
        Err(error) => host_error(error),
    }
}

async fn artifact(
    State(lab): State<LabHttp>,
    Path((run_id, index)): Path<(String, String)>,
    request: Request,
) -> Response {
    if let Some(error) = gate(
        &lab,
        request.method(),
        request.headers(),
        request.extensions(),
    ) {
        return error;
    }
    if read_limited(request).await.is_err() {
        return payload_too_large();
    }
    let Ok(index) = index.parse::<usize>() else {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_input",
            "成果物の番号を確認してください。",
        );
    };
    match lab.service.artifact(&run_id, index).await {
        Ok(artifact) => Response::builder()
            .header(header::CONTENT_TYPE, artifact.mime_type)
            .body(Body::from(artifact.bytes))
            .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response()),
        Err(error) => host_error(error),
    }
}

fn gate(
    lab: &LabHttp,
    method: &Method,
    headers: &HeaderMap,
    extensions: &axum::http::Extensions,
) -> Option<Response> {
    let peer = extensions
        .get::<ConnectInfo<SocketAddr>>()
        .map(|info| info.0);
    authorize(&lab.auth, method, headers, peer)
        .err()
        .map(IntoResponse::into_response)
}

fn stream_response(handle: RunHandle) -> Response {
    let stream = stream::unfold(
        (handle, 1u64, false),
        |(mut handle, seq, done)| async move {
            if done {
                return None;
            }
            if handle.terminal().is_some() {
                return Some((
                    line_ok(terminal_line(&handle, seq)),
                    (handle, seq + 1, true),
                ));
            }
            if let Some(progress) = handle.next_progress().await {
                if handle.terminal().is_some() {
                    return Some((
                        line_ok(terminal_line(&handle, seq)),
                        (handle, seq + 1, true),
                    ));
                }
                let line = json_line(
                    handle.run_id(),
                    seq,
                    json!({
                        "version": 1,
                        "runId": handle.run_id(),
                        "seq": seq,
                        "type": "progress",
                        "progress": progress
                    }),
                );
                return Some((line_ok(line), (handle, seq + 1, false)));
            }
            Some((
                line_ok(terminal_line(&handle, seq)),
                (handle, seq + 1, true),
            ))
        },
    );
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/x-ndjson")
        .body(Body::from_stream(stream))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

fn terminal_line(handle: &RunHandle, seq: u64) -> String {
    let value = match handle.terminal() {
        Some(RunTerminal::Output(output)) => json!({
            "version": 1,
            "runId": handle.run_id(),
            "seq": seq,
            "type": "terminal",
            "output": output
        }),
        Some(RunTerminal::HostFailure(error)) => json!({
            "version": 1,
            "runId": handle.run_id(),
            "seq": seq,
            "type": "error",
            "error": {"code": error.code, "message": error.message}
        }),
        None => json!({
            "version": 1,
            "runId": handle.run_id(),
            "seq": seq,
            "type": "error",
            "error": {"code": "stream_interrupted", "message": "生成結果の待受が切れました。"}
        }),
    };
    json_line(handle.run_id(), seq, value)
}

fn line_ok(text: String) -> Result<String, std::convert::Infallible> {
    Ok(text)
}

fn json_line(run_id: &str, seq: u64, value: Value) -> String {
    let mut text = value.to_string();
    if text.len() > LINE_LIMIT {
        text = json!({
            "version": 1,
            "runId": run_id,
            "seq": seq,
            "type": "error",
            "error": {"code": "payload_too_large", "message": "応答が大きすぎます。"}
        })
        .to_string();
    }
    text.push('\n');
    text
}

async fn read_limited(request: Request) -> Result<axum::body::Bytes, ()> {
    if content_length_too_large(request.headers()) {
        return Err(());
    }
    axum::body::to_bytes(request.into_body(), BODY_LIMIT)
        .await
        .map_err(|_| ())
}

fn content_length_too_large(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .is_some_and(|length| length > BODY_LIMIT as u64)
}

fn payload_too_large() -> Response {
    error_response(
        StatusCode::PAYLOAD_TOO_LARGE,
        "payload_too_large",
        "生成要求が大きすぎます。",
    )
}

fn query_run_id(raw: &str) -> Result<Option<String>, ()> {
    let mut run_id = None;
    for pair in raw.split('&').filter(|pair| !pair.is_empty()) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        let key = percent_decode(key)?;
        if key != "runId" || run_id.is_some() {
            return Err(());
        }
        run_id = Some(percent_decode(value)?);
    }
    Ok(run_id)
}

fn invalid_query() -> Response {
    error_response(
        StatusCode::BAD_REQUEST,
        "invalid_input",
        "照会条件を確認してください。",
    )
}

fn percent_decode(value: &str) -> Result<String, ()> {
    let mut bytes = Vec::new();
    let input = value.as_bytes();
    let mut index = 0;
    while index < input.len() {
        if input[index] == b'%' && index + 2 < input.len() {
            let hex = std::str::from_utf8(&input[index + 1..index + 3]).map_err(|_| ())?;
            bytes.push(u8::from_str_radix(hex, 16).map_err(|_| ())?);
            index += 3;
        } else if input[index] == b'+' {
            bytes.push(b' ');
            index += 1;
        } else {
            bytes.push(input[index]);
            index += 1;
        }
    }
    String::from_utf8(bytes).map_err(|_| ())
}

fn host_error(error: MediaHostError) -> Response {
    error_response(
        StatusCode::from_u16(error.http_status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        error.code,
        &error.message,
    )
}

fn error_response(status: StatusCode, code: &str, message: &str) -> Response {
    (
        status,
        Json(json!({"error": {"code": code, "message": message}})),
    )
        .into_response()
}

fn json_content(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("application/json"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    fn lab() -> LabHttp {
        let directory =
            std::env::temp_dir().join(format!("saaa-feature-lab-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        let config = crate::store::LabConfig {
            database_path: directory.join("lab.sqlite").display().to_string(),
            allowed_origin: "http://127.0.0.1:1422".into(),
            session_token: "0123456789abcdef0123456789abcdef".into(),
            provider: "fixture".into(),
            larm_endpoint: "http://127.0.0.1:9/".into(),
            larm_token: None,
            replace_route: false,
        };
        let open = crate::store::open_lab(config).unwrap();
        open_service(open, "127.0.0.1:9".into()).unwrap().http
    }

    fn request(lab: &LabHttp, method: &str, uri: &str, body: Option<&str>) -> Request<Body> {
        let mut builder = Request::builder()
            .method(method)
            .uri(uri)
            .header("host", "127.0.0.1:9")
            .header("origin", "http://127.0.0.1:1422")
            .header("cookie", format!("saaa_lab_session={}", lab.auth.token));
        if body.is_some() {
            builder = builder.header("content-type", "application/json");
        }
        let mut request = builder
            .body(Body::from(body.unwrap_or("").to_string()))
            .unwrap();
        request
            .extensions_mut()
            .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 1))));
        request
    }

    #[tokio::test]
    async fn missing_cookie_is_unauthorized_and_does_not_reach_generation() {
        let lab = lab();
        let mut request = request(&lab, "GET", "/api/v1/media/runs", None);
        request.headers_mut().remove("cookie");
        let response = router(lab).oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn fixture_image_stream_posts_once() {
        let lab = lab();
        let posts = lab.posts.clone();
        let request = request(
            &lab,
            "POST",
            "/api/v1/media/runs",
            Some(
                r#"{"runId":"00000000-0000-4000-8000-000000000010","kind":"image","prompt":"円"}"#,
            ),
        );
        let response = router(lab).oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = http_body_util::BodyExt::collect(response.into_body())
            .await
            .unwrap()
            .to_bytes();
        let text = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(text.contains("\"type\":\"terminal\""));
        assert_eq!(posts.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn reconcile_of_a_finished_image_does_not_post_again() {
        let lab = lab();
        let posts = lab.posts.clone();
        let app = router(lab.clone());
        let generated = app
            .clone()
            .oneshot(request(
                &lab,
                "POST",
                "/api/v1/media/runs",
                Some(
                    r#"{"runId":"00000000-0000-4000-8000-000000000011","kind":"image","prompt":"円"}"#,
                ),
            ))
            .await
            .unwrap();
        let _ = http_body_util::BodyExt::collect(generated.into_body())
            .await
            .unwrap();
        let reconciled = app
            .oneshot(request(
                &lab,
                "POST",
                "/api/v1/media/runs/00000000-0000-4000-8000-000000000011/reconcile",
                None,
            ))
            .await
            .unwrap();
        assert_eq!(reconciled.status(), StatusCode::OK);
        let bytes = http_body_util::BodyExt::collect(reconciled.into_body())
            .await
            .unwrap()
            .to_bytes();
        assert!(String::from_utf8(bytes.to_vec())
            .unwrap()
            .contains("\"type\":\"terminal\""));
        assert_eq!(posts.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn missing_peer_and_invalid_run_id_are_rejected() {
        let lab = lab();
        let app = router(lab.clone());
        let mut missing_peer = request(&lab, "GET", "/api/v1/media/runs", None);
        missing_peer
            .extensions_mut()
            .remove::<ConnectInfo<SocketAddr>>();
        let response = app.clone().oneshot(missing_peer).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let response = app
            .oneshot(request(
                &lab,
                "GET",
                "/api/v1/media/runs?runId=not-a-uuid",
                None,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn rejected_requests_do_not_send() {
        let lab = lab();
        let posts = lab.posts.clone();
        let app = router(lab.clone());
        let mut wrong_host = request(&lab, "POST", "/api/v1/media/runs", Some("{}"));
        wrong_host
            .headers_mut()
            .insert("host", "example.com".parse().unwrap());
        assert_eq!(
            app.clone().oneshot(wrong_host).await.unwrap().status(),
            StatusCode::FORBIDDEN
        );
        let mut wrong_origin = request(&lab, "POST", "/api/v1/media/runs", Some("{}"));
        wrong_origin
            .headers_mut()
            .insert("origin", "http://127.0.0.1:9".parse().unwrap());
        assert_eq!(
            app.clone().oneshot(wrong_origin).await.unwrap().status(),
            StatusCode::FORBIDDEN
        );
        let mut remote = request(&lab, "GET", "/api/v1/media/runs", None);
        remote
            .extensions_mut()
            .insert(ConnectInfo(SocketAddr::from(([8, 8, 8, 8], 1))));
        assert_eq!(
            app.clone().oneshot(remote).await.unwrap().status(),
            StatusCode::FORBIDDEN
        );
        let mut plain = request(&lab, "POST", "/api/v1/media/runs", Some("{}"));
        plain
            .headers_mut()
            .insert("content-type", "text/plain".parse().unwrap());
        assert_eq!(
            app.clone().oneshot(plain).await.unwrap().status(),
            StatusCode::UNSUPPORTED_MEDIA_TYPE
        );
        let mut huge = request(&lab, "POST", "/api/v1/media/runs", Some("{}"));
        huge.headers_mut()
            .insert("content-length", "999999".parse().unwrap());
        assert_eq!(
            app.oneshot(huge).await.unwrap().status(),
            StatusCode::PAYLOAD_TOO_LARGE
        );
        assert_eq!(posts.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn an_error_body_does_not_echo_the_session_token() {
        let lab = lab();
        let token = lab.auth.token.clone();
        let mut request = request(&lab, "GET", "/api/v1/media/runs", None);
        request.headers_mut().remove("cookie");
        let response = router(lab).oneshot(request).await.unwrap();
        let bytes = http_body_util::BodyExt::collect(response.into_body())
            .await
            .unwrap()
            .to_bytes();
        assert!(!String::from_utf8(bytes.to_vec()).unwrap().contains(&token));
    }
}
