#![allow(clippy::result_large_err)]
//! Axum wiring for the MCP endpoint: authentication, media-type checks, body limits and the
//! JSON-RPC method dispatch. HTTP-boundary failures (auth, host, origin, session, version) are
//! HTTP statuses; JSON-RPC failures are HTTP 200 with an error object.

use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::{header, HeaderMap, HeaderName, HeaderValue, StatusCode, Uri};
use axum::response::Response;
use axum::routing::post;
use axum::Router;
use serde_json::{json, Value};

use super::calls::{self, CallPermit};
use super::context;
use super::http::{
    accept_is_supported, authenticate, content_type_is_json, empty, header_str, json_response,
    read_body,
};
use super::protocol::{
    self, JsonRpcError, Parsed, TypedRequestId, DEADLINE_EXCEEDED, INTERNAL_ERROR, INVALID_PARAMS,
    INVALID_REQUEST, METHOD_NOT_FOUND, NOT_INITIALIZED, SERVER_BUSY,
};
use super::sessions::{ReserveResult, Session, SessionState, SESSION_MAX};
use super::{
    ServerInner, MCP_SERVER_ENDPOINT, MCP_SERVER_NAME, MCP_SERVER_PROTOCOL_VERSION,
    MCP_SERVER_VERSION, PROTOCOL_HEADER, SESSION_HEADER,
};
use crate::tool_selection::gateway;
use crate::RunCancellation;

pub fn router(inner: Arc<ServerInner>) -> Router {
    Router::new()
        .route(
            MCP_SERVER_ENDPOINT,
            post(handle_post).get(handle_get).delete(handle_delete),
        )
        .with_state(inner)
}

struct RpcReply {
    value: Value,
    session_id: Option<String>,
}

impl RpcReply {
    fn value(value: Value) -> Self {
        Self {
            value,
            session_id: None,
        }
    }
}

async fn handle_post(State(inner): State<Arc<ServerInner>>, request: Request) -> Response {
    if inner.is_shutting_down() {
        return empty(StatusCode::SERVICE_UNAVAILABLE);
    }
    let (parts, body) = request.into_parts();
    let role_root_id = match role_root_from_uri(&parts.uri) {
        Some(value) => value,
        None => return empty(StatusCode::BAD_REQUEST),
    };
    if let Err(response) = authenticate(&inner, &parts.headers) {
        return response;
    }
    if !content_type_is_json(&parts.headers) {
        return empty(StatusCode::UNSUPPORTED_MEDIA_TYPE);
    }
    if !accept_is_supported(&parts.headers) {
        return empty(StatusCode::NOT_ACCEPTABLE);
    }
    let bytes = match read_body(body).await {
        Ok(bytes) => bytes,
        Err(response) => return response,
    };
    // Reclaim idle/never-initialized sessions and release their scopes on any client activity.
    purge_sessions(&inner);
    match protocol::parse(&bytes) {
        Parsed::Error(error) => {
            json_response(StatusCode::OK, &protocol::error_response(None, &error))
        }
        Parsed::Notification { method, params } => {
            handle_notification(&inner, &parts.headers, &method, &params);
            empty(StatusCode::ACCEPTED)
        }
        Parsed::Request(request) => {
            match handle_request(
                &inner,
                &parts.headers,
                &request.id,
                &request.method,
                &request.params,
                role_root_id.as_deref(),
            )
            .await
            {
                Ok(reply) => {
                    let mut response = json_response(StatusCode::OK, &reply.value);
                    if let Some(session_id) = reply.session_id {
                        if let Ok(value) = HeaderValue::from_str(&session_id) {
                            response
                                .headers_mut()
                                .insert(HeaderName::from_static(SESSION_HEADER), value);
                        }
                    }
                    response
                }
                Err(response) => response,
            }
        }
    }
}

async fn handle_request(
    inner: &Arc<ServerInner>,
    headers: &HeaderMap,
    id: &TypedRequestId,
    method: &str,
    params: &Value,
    role_root_id: Option<&str>,
) -> Result<RpcReply, Response> {
    match method {
        "initialize" => initialize(inner, headers, id, params, role_root_id).await,
        "ping" => {
            let _session = require_session(inner, headers)?;
            Ok(RpcReply::value(protocol::success_response(id, json!({}))))
        }
        "tools/list" => {
            let session = require_session(inner, headers)?;
            if session.state() != SessionState::Ready {
                return Ok(RpcReply::value(not_initialized(id)));
            }
            if let Some(error) = validate_list_params(params) {
                return Ok(RpcReply::value(protocol::error_response(Some(id), &error)));
            }
            Ok(RpcReply::value(protocol::success_response(
                id,
                calls::tools_list(),
            )))
        }
        "tools/call" => tools_call(inner, headers, id, params).await,
        _ => Ok(RpcReply::value(protocol::error_response(
            Some(id),
            &JsonRpcError::new(METHOD_NOT_FOUND, "Method not found"),
        ))),
    }
}

fn handle_notification(
    inner: &Arc<ServerInner>,
    headers: &HeaderMap,
    method: &str,
    params: &Value,
) {
    match method {
        "notifications/initialized" => {
            if let Ok(session) = require_session(inner, headers) {
                session.mark_ready();
            }
        }
        "notifications/cancelled" => {
            if let Ok(session) = require_session(inner, headers) {
                if let Some(request_id) =
                    params.get("requestId").and_then(TypedRequestId::from_value)
                {
                    session.cancel(&request_id);
                }
            }
        }
        _ => {}
    }
}

async fn initialize(
    inner: &Arc<ServerInner>,
    headers: &HeaderMap,
    id: &TypedRequestId,
    params: &Value,
    role_root_id: Option<&str>,
) -> Result<RpcReply, Response> {
    if headers.contains_key(SESSION_HEADER) {
        return Err(empty(StatusCode::BAD_REQUEST));
    }
    let Some(object) = params.as_object() else {
        return Ok(RpcReply::value(invalid_params(id)));
    };
    if object
        .get("protocolVersion")
        .and_then(Value::as_str)
        .is_none()
    {
        return Ok(RpcReply::value(invalid_params(id)));
    }
    if let Some(capabilities) = object.get("capabilities") {
        if !capabilities.is_object() {
            return Ok(RpcReply::value(invalid_params(id)));
        }
    }
    let client_info = object.get("clientInfo").cloned();
    if let Some(client_info) = &client_info {
        if !client_info.is_object() {
            return Ok(RpcReply::value(invalid_params(id)));
        }
    }
    if inner.sessions.len() >= SESSION_MAX {
        return Ok(RpcReply::value(server_busy(id)));
    }
    let (conversation_id, run_id, role_binding) = match role_root_id {
        Some(root_id) => match context::active_role_binding(&inner.writer, root_id) {
            Some((conversation_id, binding)) => {
                (conversation_id, root_id.to_string(), Some(binding))
            }
            None => return Ok(RpcReply::value(invalid_params(id))),
        },
        None => (
            crate::new_id("mcpconv"),
            format!("mcp-session:{}", uuid::Uuid::new_v4()),
            None,
        ),
    };
    let session = super::sessions::Session::new(
        uuid::Uuid::new_v4().to_string(),
        MCP_SERVER_PROTOCOL_VERSION.to_string(),
        client_info,
        conversation_id.clone(),
        run_id,
        role_binding,
        inner.principal.clone(),
        inner.project_id.clone(),
    );
    // Reserve the session first: the capacity check in `insert` is authoritative, so a rejected
    // initialize never creates a conversation row that would be orphaned.
    let session = match inner.sessions.insert(session) {
        Ok(session) => session,
        Err(_) => return Ok(RpcReply::value(server_busy(id))),
    };
    if session.role_root_id().is_none()
        && context::create_conversation(&inner.writer, &conversation_id).is_err()
    {
        // Roll the reservation back so a storage failure does not hold a session slot.
        inner.sessions.remove(session.id());
        return Ok(RpcReply::value(protocol::error_response(
            Some(id),
            &JsonRpcError::new(INTERNAL_ERROR, "Internal error"),
        )));
    }
    let result = json!({
        "protocolVersion": MCP_SERVER_PROTOCOL_VERSION,
        "capabilities": { "tools": { "listChanged": false } },
        "serverInfo": { "name": MCP_SERVER_NAME, "version": MCP_SERVER_VERSION }
    });
    Ok(RpcReply {
        value: protocol::success_response(id, result),
        session_id: Some(session.id().to_string()),
    })
}

async fn tools_call(
    inner: &Arc<ServerInner>,
    headers: &HeaderMap,
    id: &TypedRequestId,
    params: &Value,
) -> Result<RpcReply, Response> {
    let session = require_session(inner, headers)?;
    if session.state() != SessionState::Ready {
        return Ok(RpcReply::value(not_initialized(id)));
    }
    let Some(object) = params.as_object() else {
        return Ok(RpcReply::value(invalid_params(id)));
    };
    let Some(name) = object.get("name").and_then(Value::as_str) else {
        return Ok(RpcReply::value(invalid_params(id)));
    };
    if gateway::internal_name(name).is_none() {
        return Ok(RpcReply::value(invalid_params(id)));
    }
    let arguments = object
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    if !arguments.is_object() {
        return Ok(RpcReply::value(invalid_params(id)));
    }
    let cancellation = RunCancellation::default();
    match inner
        .sessions
        .reserve_call(&session, id, cancellation.clone())
    {
        ReserveResult::Reserved => {}
        ReserveResult::Duplicate => {
            return Ok(RpcReply::value(protocol::error_response(
                Some(id),
                &JsonRpcError::new(INVALID_REQUEST, "Invalid Request"),
            )));
        }
        ReserveResult::HistoryFull => {
            // The id history is never pruned; a full history needs a fresh session rather than a
            // silent replay of a side effect.
            return Ok(RpcReply::value(protocol::error_response(
                Some(id),
                &JsonRpcError::new(SERVER_BUSY, "Session request history is full"),
            )));
        }
        ReserveResult::SessionLimit | ReserveResult::GlobalLimit => {
            return Ok(RpcReply::value(server_busy(id)));
        }
        ReserveResult::Closing => return Err(empty(StatusCode::NOT_FOUND)),
    }
    let context = context::session_context(&session);
    let service = inner.service.clone();
    let writer = inner.writer.clone();
    let role_binding = session.role_binding().cloned();
    let name = name.to_string();
    let task_cancellation = cancellation.clone();
    let deadline = std::time::Duration::from_millis(
        inner
            .call_deadline_ms
            .load(std::sync::atomic::Ordering::SeqCst),
    );
    // The management task owns the reserved slot. If the HTTP handler future is dropped on client
    // disconnect (or the deadline detaches the task), the slot stays reserved until the backend
    // actually settles, so the in-flight counter and shutdown drain remain accurate.
    let permit = CallPermit::new(inner.clone(), session.clone(), id.clone());
    let task = tokio::spawn(async move {
        let _permit = permit;
        calls::execute_tool_call(
            &service,
            &writer,
            &context,
            role_binding.as_ref(),
            &name,
            &arguments,
            &task_cancellation,
        )
        .await
    });
    let reply = match tokio::time::timeout(deadline, task).await {
        Ok(Ok(Ok(result))) => protocol::success_response(id, result),
        Ok(Ok(Err(error))) => protocol::error_response(Some(id), &error),
        Ok(Err(_)) => protocol::error_response(
            Some(id),
            &JsonRpcError::new(INTERNAL_ERROR, "Internal error"),
        ),
        Err(_) => {
            cancellation.cancel();
            protocol::error_response(
                Some(id),
                &JsonRpcError::new(DEADLINE_EXCEEDED, "Request timed out"),
            )
        }
    };
    session.touch();
    Ok(RpcReply::value(reply))
}

/// A role root can only be selected at initialization and is bound to the authenticated MCP
/// session. Query parsing is strict so an arbitrary URL cannot smuggle a second authority value.
fn role_root_from_uri(uri: &Uri) -> Option<Option<String>> {
    let Some(query) = uri.query() else {
        return Some(None);
    };
    let mut values = url::form_urlencoded::parse(query.as_bytes());
    let (key, root_id) = values.next()?;
    if key != "rrRoot" || values.next().is_some() || root_id.is_empty() || root_id.len() > 160 {
        return None;
    }
    Some(Some(root_id.into_owned()))
}

async fn handle_get(State(inner): State<Arc<ServerInner>>, headers: HeaderMap) -> Response {
    if let Err(response) = authenticate(&inner, &headers) {
        return response;
    }
    match require_session(&inner, &headers) {
        Ok(_) => {
            let mut response = empty(StatusCode::METHOD_NOT_ALLOWED);
            response
                .headers_mut()
                .insert(header::ALLOW, HeaderValue::from_static("POST, DELETE"));
            response
        }
        Err(response) => response,
    }
}

async fn handle_delete(State(inner): State<Arc<ServerInner>>, headers: HeaderMap) -> Response {
    if let Err(response) = authenticate(&inner, &headers) {
        return response;
    }
    let Some(session_id) = header_str(&headers, SESSION_HEADER) else {
        return empty(StatusCode::BAD_REQUEST);
    };
    // Resolve through `get` first so an idle-expired session is unknown (404), not silently
    // accepted as a valid delete target.
    let Some(session) = inner.sessions.get(session_id) else {
        return empty(StatusCode::NOT_FOUND);
    };
    let Some(session) = inner.sessions.remove(session.id()) else {
        return empty(StatusCode::NOT_FOUND);
    };
    session.cancel_all();
    inner.discard_session_scope(&session);
    empty(StatusCode::NO_CONTENT)
}

/// Releases the scopes of sessions that exceeded their idle or initialization TTL.
fn purge_sessions(inner: &ServerInner) {
    for session in inner.sessions.purge_expired() {
        inner.discard_session_scope(&session);
    }
}

fn require_session(inner: &ServerInner, headers: &HeaderMap) -> Result<Arc<Session>, Response> {
    let Some(session_id) = header_str(headers, SESSION_HEADER) else {
        return Err(empty(StatusCode::BAD_REQUEST));
    };
    let Some(session) = inner.sessions.get(session_id) else {
        return Err(empty(StatusCode::NOT_FOUND));
    };
    if session.is_closing() {
        return Err(empty(StatusCode::NOT_FOUND));
    }
    if let Some(version) = header_str(headers, PROTOCOL_HEADER) {
        if version != session.protocol_version() {
            return Err(empty(StatusCode::BAD_REQUEST));
        }
    }
    // Any valid request refreshes the idle TTL, not only tools/call, so a client that lists or
    // pings keeps its session alive.
    session.touch();
    Ok(session)
}

fn validate_list_params(params: &Value) -> Option<JsonRpcError> {
    match params {
        Value::Null => None,
        Value::Object(object) => {
            if object.contains_key("cursor") {
                Some(JsonRpcError::new(INVALID_PARAMS, "Invalid params"))
            } else {
                None
            }
        }
        _ => Some(JsonRpcError::new(INVALID_PARAMS, "Invalid params")),
    }
}

fn invalid_params(id: &TypedRequestId) -> Value {
    protocol::error_response(
        Some(id),
        &JsonRpcError::new(INVALID_PARAMS, "Invalid params"),
    )
}

fn server_busy(id: &TypedRequestId) -> Value {
    protocol::error_response(Some(id), &JsonRpcError::new(SERVER_BUSY, "Server busy"))
}

fn not_initialized(id: &TypedRequestId) -> Value {
    protocol::error_response(
        Some(id),
        &JsonRpcError::new(NOT_INITIALIZED, "Server not initialized"),
    )
}

#[cfg(test)]
mod tests {
    use super::role_root_from_uri;
    use axum::http::Uri;

    #[test]
    fn rr_21_role_root_query_is_strict_and_decoded_once() {
        let uri: Uri = "http://127.0.0.1:43127/mcp?rrRoot=root%2Fone"
            .parse()
            .expect("uri");
        assert_eq!(role_root_from_uri(&uri), Some(Some("root/one".into())));
        let absent: Uri = "http://127.0.0.1:43127/mcp".parse().expect("uri");
        assert_eq!(role_root_from_uri(&absent), Some(None));
        for invalid in [
            "http://127.0.0.1/mcp?rrRoot=",
            "http://127.0.0.1/mcp?rrRoot=one&rrRoot=two",
            "http://127.0.0.1/mcp?root=one",
        ] {
            assert_eq!(role_root_from_uri(&invalid.parse().expect("uri")), None);
        }
    }
}
