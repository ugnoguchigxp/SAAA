//! Isolated LARM control plane; model traffic goes to the requested endpoint.
use super::QualityRequest;
use axum::{
    extract::{Request, State},
    http::{Method, StatusCode},
    response::{IntoResponse, Response},
    Json, Router,
};
use serde_json::{json, Value};
use std::sync::Arc;

#[derive(Clone)]
struct Control {
    base: String,
    endpoint: String,
    model: String,
    key: String,
}
pub(super) struct Server {
    task: tokio::task::JoinHandle<()>,
    pub(super) base: String,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

pub(super) async fn start(request: &QualityRequest) -> Result<Server, String> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| e.to_string())?;
    let base = format!(
        "http://{}",
        listener.local_addr().map_err(|e| e.to_string())?
    );
    let control = Arc::new(Control {
        base: base.clone(),
        endpoint: request.base_url.clone(),
        model: request.model.clone(),
        key: request.api_key.clone(),
    });
    let router = Router::new().fallback(serve).with_state(control);
    let task = tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    Ok(Server { task, base })
}
fn declaration(name: &str, model: &str) -> Value {
    let (capability, protocol, endpoint) = match name {
        "llm" => (
            "llm.general",
            "openai.chat-completions.v1",
            "/v1/chat/completions",
        ),
        "asr" => (
            "speech.stt",
            "openai.audio-transcriptions.v1",
            "/v1/audio/transcriptions",
        ),
        "tts" => ("speech.tts", "openai.audio-speech.v1", "/v1/audio/speech"),
        _ => ("embedding", "larm.embedding.v1", "/v1/embed"),
    };
    let mut value = json!({"name":name,"capability":capability,"protocol":protocol,"endpoint":endpoint,"model":model});
    if name == "llm" {
        value["contextWindow"] =
            json!({"maxTokens":65536,"outputReserveTokens":4096,"safetyMarginTokens":1976});
    }
    value
}
fn state(control: &Control, claim: bool) -> Value {
    let providers: Vec<_> = ["asr", "llm", "tts", "embedding"].into_iter().map(|name| {
        let mut value = declaration(name, &control.model);
        value["readiness"] = json!("ready"); value["claimable"] = json!(true);
        if claim {
            let base = if name == "llm" { control.endpoint.clone() } else { format!("{}/{name}/v1", control.base) };
            value["baseUrl"] = json!(base);
            value["credential"] = json!({"token": if name == "llm" { &control.key } else { "quality-control-token" }});
            let field = if name == "embedding" { "daemonURL" } else { "baseURL" };
            value["configuration"] = json!({"fields":{"model":control.model,field:base}});
            value["health"] = json!({"url":format!("{}/{name}/health", control.base),"maxAgeMs":10000});
            if name == "embedding" { value["embeddingSpace"] = json!({"dimension":384}); }
        }
        value
    }).collect();
    let mut value = json!({"id":"quality-session","profile":"SAAA","agentProfile":"saaa-conversation-ornith15","providers":providers,"services":[],"status":"ready","expiresAt":(chrono::Utc::now()+chrono::Duration::minutes(15)).to_rfc3339()});
    if claim {
        value["allocationId"] = json!("quality-allocation");
    }
    value
}
async fn serve(State(control): State<Arc<Control>>, request: Request) -> Response {
    let path = request.uri().path();
    if path == "/v3/agent-profiles" {
        let providers: Vec<_> = ["asr", "llm", "tts", "embedding"]
            .into_iter()
            .map(|n| declaration(n, &control.model))
            .collect();
        return Json(json!({"contractVersion":"agent-connection.v3","catalogRevision":"quality-eval","audiences":["saaa-desktop"],"requestedProfile":"SAAA","profiles":[{"id":"saaa-conversation-ornith15","providers":providers,"services":[]}]})).into_response();
    }
    if path.ends_with("/health") {
        let name = path.split('/').nth(1).unwrap_or("llm");
        return Json(json!({"ready":true,"acceptingRequests":true,"capacity":{"maxConcurrentRequests":4,"activeRequests":0,"maxQueuedRequests":4,"queueDepth":0,"queueTimeoutMs":1000,"retryAfterMs":0,"completionGuaranteed":false},"probe":{"validated":true,"protocol":declaration(name, &control.model)["protocol"]}})).into_response();
    }
    if path.starts_with("/v1/agent-connections") {
        if request
            .headers()
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            != Some("Bearer quality-control-token")
        {
            return StatusCode::UNAUTHORIZED.into_response();
        }
        if request.method() == Method::DELETE {
            return StatusCode::NO_CONTENT.into_response();
        }
        let code = if request.method() == Method::POST && !path.ends_with("/claim") {
            StatusCode::CREATED
        } else {
            StatusCode::OK
        };
        return (code, Json(state(&control, path.ends_with("/claim")))).into_response();
    }
    StatusCode::NOT_FOUND.into_response()
}
