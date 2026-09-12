//! A narrow, stateless Streamable HTTP MCP endpoint. No sampling or downstream tool execution.
use axum::{
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use saaa_reasoning_contract::{
    Request, Response as AnswerResponse, MAX_INPUT_BYTES, PROTOCOL, TOOL,
};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::{watch, Semaphore};
pub mod provider;

#[derive(Clone)]
pub struct Service {
    provider: provider::Provider,
    token: String,
    active: Arc<Mutex<Control>>,
    capacity: Arc<Semaphore>,
}
#[derive(Default)]
struct Control {
    active: HashMap<String, watch::Sender<bool>>,
    cancelled: VecDeque<(String, Instant)>,
}
impl Control {
    fn cancel(&mut self, id: &str) {
        if let Some(sender) = self.active.get(id) {
            let _ = sender.send(true);
        }
        if id.len() > 160 {
            return;
        }
        if self.cancelled.len() == 128 {
            self.cancelled.pop_front();
        }
        self.cancelled.push_back((id.to_string(), Instant::now()));
    }
    fn was_cancelled(&mut self, id: &str) -> bool {
        while self
            .cancelled
            .front()
            .is_some_and(|(_, at)| at.elapsed() > Duration::from_secs(15))
        {
            self.cancelled.pop_front();
        }
        self.cancelled.iter().any(|(value, _)| value == id)
    }
}
impl Service {
    pub fn new(provider: provider::Provider, token: String) -> Result<Self, String> {
        if token.trim().len() < 16 {
            return Err("MCP bearer token must contain at least 16 characters".into());
        }
        Ok(Self {
            provider,
            token,
            active: Arc::default(),
            capacity: Arc::new(Semaphore::new(1)),
        })
    }
    pub fn router(self) -> Router {
        Router::new()
            .route("/mcp", post(handle))
            .layer(DefaultBodyLimit::max(MAX_INPUT_BYTES + 4096))
            .with_state(self)
    }
}
struct Active {
    id: String,
    map: Arc<Mutex<Control>>,
}
impl Drop for Active {
    fn drop(&mut self) {
        if let Ok(mut map) = self.map.lock() {
            map.active.remove(&self.id);
        }
    }
}
fn result(id: Value, value: Value) -> Response {
    Json(json!({"jsonrpc":"2.0","id":id,"result":value})).into_response()
}
fn error(id: Value, code: i32, message: &str) -> Response {
    Json(json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}})).into_response()
}
fn tool_error(id: Value, code: &str) -> Response {
    result(
        id,
        json!({"isError":true,"content":[{"type":"text","text":code}]}),
    )
}
async fn handle(
    State(service): State<Service>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    // Not a browser API. Reject Origin-bearing requests instead of enabling CORS.
    if headers.contains_key("origin") {
        return StatusCode::FORBIDDEN.into_response();
    }
    let authorized = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v == format!("Bearer {}", service.token));
    if !authorized {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let id = body.get("id").cloned().unwrap_or(Value::Null);
    if body["jsonrpc"] != "2.0" {
        return error(id, -32600, "Invalid JSON-RPC request");
    }
    let method = body["method"].as_str().unwrap_or_default();
    if method == "initialize" {
        return result(
            id,
            json!({"protocolVersion":PROTOCOL,"capabilities":{"tools":{}},"serverInfo":{"name":"saaa-reasoning","version":"0.1.0"}}),
        );
    }
    if headers
        .get("mcp-protocol-version")
        .and_then(|h| h.to_str().ok())
        != Some(PROTOCOL)
    {
        return StatusCode::BAD_REQUEST.into_response();
    }
    if method == "notifications/initialized" {
        return StatusCode::ACCEPTED.into_response();
    }
    if method == "notifications/cancelled" {
        if let Some(request_id) = body["params"]["requestId"].as_str() {
            if let Ok(mut control) = service.active.lock() {
                control.cancel(request_id);
            }
        }
        return StatusCode::ACCEPTED.into_response();
    }
    if id.is_null() {
        return StatusCode::ACCEPTED.into_response();
    }
    if method == "ping" {
        return result(id, json!({}));
    }
    if method == "tools/list" {
        return result(
            id,
            json!({"tools":[saaa_reasoning_contract::schema::tool()]}),
        );
    }
    if method != "tools/call" {
        return error(id, -32601, "Method not found");
    }
    if body["params"]["name"] != TOOL {
        return error(id, -32602, "Unknown tool");
    }
    let request = match serde_json::from_value::<Request>(body["params"]["arguments"].clone()) {
        Ok(r) if r.validate().is_ok() => r,
        _ => return error(id, -32602, "Invalid reasoning request"),
    };
    // UUID call ID and contract request ID are deliberately the same in this profile.
    if id.as_str() != Some(&request.request_id) {
        return error(id, -32602, "Request ID mismatch");
    }
    let _permit = match service.capacity.clone().try_acquire_owned() {
        Ok(p) => p,
        Err(_) => return tool_error(id, "busy"),
    };
    let (cancel, mut cancelled) = watch::channel(false);
    match service.active.lock() {
        Ok(mut map) => {
            if map.was_cancelled(&request.request_id) {
                return tool_error(id, "cancelled");
            }
            map.active.insert(request.request_id.clone(), cancel);
        }
        Err(_) => return tool_error(id, "unavailable"),
    }
    let active = Active {
        id: request.request_id.clone(),
        map: service.active.clone(),
    };
    let outcome = tokio::select! { biased;
        _ = cancelled.changed() => Err("cancelled"),
        value = tokio::time::timeout(Duration::from_millis(request.budget.timeout_ms), service.provider.answer(&request)) => value.unwrap_or(Err("timeout")),
    };
    drop(active);
    match outcome.and_then(|a| AnswerResponse::bind(&request, a)) {
        Ok(answer) => {
            let value = serde_json::to_value(answer).expect("serializable contract");
            result(
                id,
                json!({"isError":false,"structuredContent":value,"content":[{"type":"text","text":value.to_string()}]}),
            )
        }
        Err(code) => tool_error(id, code),
    }
}
