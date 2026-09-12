use crate::RunCancellation;
use saaa_reasoning_contract::{Request, Response, PROTOCOL, TOOL, VERSION};
use serde_json::{json, Value};
use std::{
    sync::{Arc, OnceLock},
    time::Duration,
};
use tokio::sync::{Mutex, Semaphore};
mod transport;

#[derive(Clone)]
pub(crate) struct Client {
    http: reqwest::Client,
    url: url::Url,
    token: String,
    session: Arc<Mutex<Option<Option<String>>>>,
    capacity: Arc<Semaphore>,
}
static CONFIG: OnceLock<Result<Option<Client>, String>> = OnceLock::new();
pub(crate) fn configured(input_origin: &str) -> Result<Option<&'static Client>, String> {
    if input_origin != "voice" {
        return Ok(None);
    }
    CONFIG
        .get_or_init(|| {
            let mode =
                std::env::var("SAAA_CONVERSATION_REASONING_MODE").unwrap_or_else(|_| "off".into());
            match mode.as_str() {
                "off" | "larm" => Ok(None),
                "deterministic" => Client::new(
                    &std::env::var("SAAA_REASONING_MCP_URL")
                        .map_err(|_| "Reasoning MCP URL is missing")?,
                    std::env::var("SAAA_REASONING_MCP_TOKEN")
                        .map_err(|_| "Reasoning MCP token is missing")?,
                )
                .map(Some),
                _ => Err("Unsupported reasoning mode; use off, deterministic or larm".into()),
            }
        })
        .as_ref()
        .map(|c| c.as_ref())
        .map_err(Clone::clone)
}
impl Client {
    pub(crate) fn new(endpoint: &str, token: String) -> Result<Self, String> {
        let url = url::Url::parse(endpoint).map_err(|_| "Invalid reasoning MCP URL")?;
        if !super::dynamic_lan::url_is_local(&url)
            || !matches!(url.scheme(), "http" | "https")
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || token.trim().len() < 16
        {
            return Err("Reasoning MCP requires an explicit private URL and token".into());
        }
        Ok(Self {
            http: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .no_proxy()
                .build()
                .map_err(|_| "Reasoning HTTP client failed")?,
            url,
            token,
            session: Arc::new(Mutex::new(None)),
            capacity: Arc::new(Semaphore::new(1)),
        })
    }
    async fn initialize(&self) -> Result<Option<String>, String> {
        let mut cached = self.session.lock().await;
        if let Some(session) = &*cached {
            return Ok(session.clone());
        }
        let id = crate::new_id("mcp_init");
        let (value, session) = self.rpc(None, json!({"jsonrpc":"2.0","id":id,"method":"initialize",
            "params":{"protocolVersion":PROTOCOL,"capabilities":{},"clientInfo":{"name":"saaa","version":"0.1.0"}}})).await?;
        if value["id"] != id || value["result"]["protocolVersion"] != PROTOCOL {
            return Err("Reasoning MCP version mismatch".into());
        }
        self.notify(session.as_deref(), "notifications/initialized", json!({}))
            .await?;
        let id = crate::new_id("mcp_tools");
        let (list, _) = self
            .rpc(
                session.as_deref(),
                json!({"jsonrpc":"2.0","id":id,"method":"tools/list","params":{}}),
            )
            .await?;
        let tool = list["result"]["tools"]
            .as_array()
            .and_then(|tools| tools.iter().find(|t| t["name"] == TOOL));
        if list["id"] != id
            || !tool.is_some_and(|t| {
                t["outputSchema"]["properties"]["schemaVersion"]["const"] == VERSION
            })
        {
            return Err("Reasoning MCP contract unavailable".into());
        }
        *cached = Some(session.clone());
        Ok(session)
    }
    pub(crate) async fn answer(
        &self,
        request: &Request,
        cancellation: Arc<RunCancellation>,
    ) -> Result<Response, String> {
        request.validate().map_err(str::to_string)?;
        let _permit = self
            .capacity
            .try_acquire()
            .map_err(|_| "Reasoning is busy")?;
        let started = std::time::Instant::now();
        let mut call_session = None;
        let work = async {
            let session = tokio::time::timeout(Duration::from_secs(3), self.initialize())
                .await
                .map_err(|_| "Reasoning MCP initialization timed out")??;
            call_session = Some(session.clone());
            let mut remaining = request.clone();
            remaining.budget.timeout_ms = request
                .budget
                .timeout_ms
                .saturating_sub(started.elapsed().as_millis() as u64);
            if remaining.budget.timeout_ms == 0 {
                return Err("Reasoning timed out".into());
            }
            let (value, _) = self
                .rpc(
                    session.as_deref(),
                    json!({"jsonrpc":"2.0","id":request.request_id,
                "method":"tools/call","params":{"name":TOOL,"arguments":remaining}}),
                )
                .await?;
            if value["id"] != request.request_id
                || value.get("error").is_some()
                || value["result"]["isError"] == true
            {
                return Err("Reasoning service could not complete the request".into());
            }
            let result: Response =
                serde_json::from_value(value["result"]["structuredContent"].clone())
                    .map_err(|_| "Invalid reasoning response")?;
            result.validate(request).map_err(str::to_string)?;
            Ok(result)
        };
        let result = tokio::select! { biased;
            _ = cancellation.cancelled() => Err("Cancelled by user".to_string()),
            result = tokio::time::timeout(Duration::from_millis(request.budget.timeout_ms),work) => result.unwrap_or_else(|_|Err("Reasoning timed out".into())),
        };
        if result.is_err() {
            if let Some(session) = call_session {
                // Cleanup is bounded and never blocks local audio cancellation.
                let _ = tokio::time::timeout(
                    Duration::from_millis(200),
                    self.notify(
                        session.as_deref(),
                        "notifications/cancelled",
                        json!({"requestId":request.request_id,"reason":"Request no longer active"}),
                    ),
                )
                .await;
            }
            *self.session.lock().await = None;
        }
        result
    }
}
#[cfg(test)]
pub(crate) mod tests;

pub(crate) async fn for_turn(
    input: &crate::StartTurnInput,
    cancellation: &RunCancellation,
) -> Result<Option<Arc<Client>>, String> {
    if let Some(client) = tokio::select! { biased;
        _ = cancellation.cancelled() => return Err("Cancelled by user".into()),
        result = crate::larm_voice::reasoning_client(&input.conversation_id, &input.input_origin) => result?,
    } {
        return Ok(Some(client));
    }
    Ok(configured(&input.input_origin)?.map(|c| Arc::new(c.clone())))
}
