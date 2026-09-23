//! `fetch_content` via the Tauri WebView worker (WF-04 / WF-05).
//!
//! Product path is one-shot: 1 tool call = 1 request ID = 1 one-shot
//! worker, destroyed on success, failure, or cancel. The adapter owns no
//! frontend IPC; it calls `LlmFetchManager` directly from the Rust backend.

use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use tauri::Runtime;

use super::contracts::{FetchContentInput, WebFetchCancel, WebFetchFailure};

#[path = "content/projection.rs"]
mod projection;
use projection::project_document;
pub use projection::render_compact;
#[cfg(test)]
use projection::{guard_decision_label, truncate_to_model_max};

/// Compact model-facing result of `fetch_content`.
#[derive(Debug, Clone)]
pub struct FetchContentResult {
    /// Final URL after redirects (validated HTTP(S)).
    pub final_url: String,
    pub text: String,
    pub fetched_at: String,
    pub truncated: bool,
    /// `allow` / `allow_with_warning` / `require_approval` / `deny`.
    pub decision: &'static str,
    pub warning_categories: Vec<String>,
    pub retrieval_status: &'static str,
    pub retrieval_method: &'static str,
}

#[async_trait]
pub trait ContentFetcher: Send + Sync {
    async fn fetch(
        &self,
        request: FetchContentInput,
        deadline: Duration,
        cancellation: WebFetchCancel,
    ) -> Result<FetchContentResult, WebFetchFailure>;
}

/// Direct `LlmFetchManager` adapter. Constructed once in Tauri `setup` and
/// shared through the WebFetch runtime slot; no second frontend-side state.
pub struct TauriWebViewContentFetcher<R: Runtime> {
    manager: Arc<tauri_plugin_llm_fetch::LlmFetchManager<R>>,
    cleanup_budget: Duration,
}

impl<R: Runtime> TauriWebViewContentFetcher<R> {
    pub fn new(manager: Arc<tauri_plugin_llm_fetch::LlmFetchManager<R>>) -> Self {
        Self {
            manager,
            cleanup_budget: Duration::from_millis(1_500),
        }
    }
}

#[async_trait]
impl<R: Runtime> ContentFetcher for TauriWebViewContentFetcher<R> {
    async fn fetch(
        &self,
        request: FetchContentInput,
        deadline: Duration,
        cancellation: WebFetchCancel,
    ) -> Result<FetchContentResult, WebFetchFailure> {
        if cancellation.is_cancelled() {
            return Err(WebFetchFailure::cancelled());
        }
        // Prefer one bounded document request. A browser is needed only for
        // pages whose static HTML has too little readable content.
        let started = std::time::Instant::now();
        let static_budget = html_probe_budget(deadline);
        if !static_budget.is_zero() {
            let static_result = tokio::select! {
                result = tokio::time::timeout(
                    static_budget,
                    super::static_content::fetch(&request, static_budget)
                ) => result.unwrap_or_else(|_| Err(WebFetchFailure::timeout())),
                _ = cancellation.cancelled() => return Err(WebFetchFailure::cancelled()),
            };
            match static_result {
                Ok(Some(result)) => return Ok(result),
                Err(error) if error.code == "UNSAFE_URL" => return Err(error),
                _ => {}
            }
        }
        if cancellation.is_cancelled() {
            return Err(WebFetchFailure::cancelled());
        }
        let deadline = deadline.saturating_sub(started.elapsed());
        if deadline.is_zero() {
            return Err(WebFetchFailure::timeout());
        }
        // Reserve a cleanup budget inside the provider deadline so
        // `manager.cancel()` + worker teardown never overruns the tool call.
        // The plugin validates `timeout_ms` against 500..=request_timeout_ms
        // (product config pins 30s), so clamp there, not at the deadline.
        let plugin_timeout_ms = deadline
            .checked_sub(self.cleanup_budget)
            .unwrap_or(Duration::from_secs(1))
            .as_millis()
            .clamp(1_000, 30_000) as u64;
        let request_id = uuid::Uuid::new_v4().to_string();
        // The model contract allows 200..=20_000 chars but the plugin
        // requires 1_000..=max_characters per request. Request at least
        // 1_000 and trim the projection down to the model ask below.
        let plugin_max_characters = request.max_characters.clamp(1_000, 20_000);
        let plugin_request = tauri_plugin_llm_fetch::FetchRequest {
            request_id: request_id.clone(),
            session_id: None,
            url: request.url.clone(),
            timeout_ms: Some(plugin_timeout_ms),
            settle_quiet_ms: None,
            max_characters: Some(plugin_max_characters),
            requested_use: Some(tauri_plugin_llm_fetch::RequestedContextUse::AnswerWithCitation),
            source: None,
        };
        // Run the plugin fetch in an owned task. If teardown exceeds the
        // response budget, dropping the JoinHandle detaches (rather than
        // cancels) the task, so its request registry/session cleanup still
        // runs to completion in the background.
        let fetch_manager = self.manager.clone();
        let mut fetch = tokio::spawn(async move { fetch_manager.fetch(plugin_request).await });
        tokio::select! {
            biased;
            _ = cancellation.cancelled() => {
                let _ = self.manager.cancel(
                    tauri_plugin_llm_fetch::CancelRequest { request_id: request_id.clone() },
                ).await;
                // Give synchronous teardown a bounded head start. On timeout
                // the owned task detaches and completes cleanup in background.
                let _ = tokio::time::timeout(self.cleanup_budget, &mut fetch).await;
                Err(WebFetchFailure::cancelled())
            }
            _ = tokio::time::sleep(deadline) => {
                let _ = self.manager.cancel(
                    tauri_plugin_llm_fetch::CancelRequest { request_id: request_id.clone() },
                ).await;
                let _ = tokio::time::timeout(self.cleanup_budget, &mut fetch).await;
                Err(WebFetchFailure::timeout())
            }
            result = &mut fetch => {
                match result {
                    Ok(Ok(document)) => Ok(project_document(&document, request.max_characters, request.query.as_deref())),
                    Ok(Err(error)) => Err(map_plugin_error(&error)),
                    Err(_) => Err(WebFetchFailure::unavailable()),
                }
            }
        }
    }
}

fn html_probe_budget(deadline: Duration) -> Duration {
    if deadline <= Duration::from_secs(5) {
        Duration::ZERO
    } else {
        (deadline / 5).min(Duration::from_secs(2))
    }
}

fn map_plugin_error(error: &tauri_plugin_llm_fetch::ErrorResponse) -> WebFetchFailure {
    use tauri_plugin_llm_fetch::ErrorCode as Code;
    match error.code {
        Code::InvalidInput => WebFetchFailure::invalid_input(),
        Code::UnsafeUrl => WebFetchFailure::unsafe_url(),
        Code::DnsFailure => WebFetchFailure::new(
            "DNS_FAILURE",
            "The page hostname could not be resolved.",
            true,
        ),
        Code::ProxyFailure => WebFetchFailure::new(
            "PROXY_FAILURE",
            "The configured network proxy could not be used.",
            true,
        ),
        Code::Timeout => WebFetchFailure::timeout(),
        Code::Cancelled => WebFetchFailure::cancelled(),
        Code::UnsupportedPlatform | Code::BackgroundUnsupported | Code::WebviewUnavailable => {
            WebFetchFailure::unavailable()
        }
        // Never leak page, network, or provider details to the model.
        _ => WebFetchFailure::new(
            "web-fetch-unavailable",
            "The page could not be retrieved.",
            true,
        ),
    }
}

#[cfg(test)]
pub struct FakeContentFetcher {
    pub result: Result<FetchContentResult, WebFetchFailure>,
    pub seen: std::sync::Mutex<Vec<String>>,
}

#[cfg(test)]
impl FakeContentFetcher {
    pub fn ok(text: &str) -> Self {
        Self {
            result: Ok(FetchContentResult {
                final_url: "https://final.example/".to_string(),
                text: text.to_string(),
                fetched_at: "2026-01-01T00:00:00.000Z".to_string(),
                truncated: false,
                decision: "allow",
                warning_categories: Vec::new(),
                retrieval_status: "relevant",
                retrieval_method: "webview",
            }),
            seen: std::sync::Mutex::new(Vec::new()),
        }
    }
}

#[cfg(test)]
#[async_trait]
impl ContentFetcher for FakeContentFetcher {
    async fn fetch(
        &self,
        request: FetchContentInput,
        _deadline: Duration,
        cancellation: WebFetchCancel,
    ) -> Result<FetchContentResult, WebFetchFailure> {
        if cancellation.is_cancelled() {
            return Err(WebFetchFailure::cancelled());
        }
        self.seen
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(request.url.clone());
        match &self.result {
            Ok(result) => Ok(result.clone()),
            Err(failure) => Err(failure.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_projection_hides_plugin_internals() {
        let result = FetchContentResult {
            final_url: "https://final.example/".to_string(),
            text: "hello".to_string(),
            fetched_at: "2026-01-01T00:00:00.000Z".to_string(),
            truncated: false,
            decision: "allow_with_warning",
            warning_categories: vec!["tool_invocation".to_string()],
            retrieval_status: "relevant",
            retrieval_method: "webview",
        };
        let rendered: serde_json::Value = serde_json::from_str(&render_compact(&result)).unwrap();
        assert_eq!(
            rendered.pointer("/type").and_then(|v| v.as_str()),
            Some("fetch_content_result")
        );
        assert_eq!(
            rendered.pointer("/security/trust").and_then(|v| v.as_str()),
            Some("untrusted")
        );
        assert_eq!(
            rendered
                .pointer("/security/tainted")
                .and_then(|v| v.as_bool()),
            Some(true)
        );
        assert!(rendered.get("sessionId").is_none());
        assert!(rendered.pointer("/document/title").is_none());
        assert!(rendered.pointer("/document/text").is_some());
        assert_eq!(
            rendered
                .pointer("/document/retrievalMethod")
                .and_then(|v| v.as_str()),
            Some("webview")
        );
        assert_eq!(
            rendered
                .pointer("/document/retrievalStatus")
                .and_then(|v| v.as_str()),
            Some("relevant")
        );
    }

    #[test]
    fn html_probe_preserves_time_for_browser_fallback() {
        assert_eq!(
            html_probe_budget(Duration::from_secs(30)),
            Duration::from_secs(2)
        );
        assert_eq!(
            html_probe_budget(Duration::from_secs(10)),
            Duration::from_secs(2)
        );
        assert_eq!(html_probe_budget(Duration::from_secs(5)), Duration::ZERO);
    }

    #[test]
    fn plugin_errors_map_to_safe_codes_without_url_details() {
        let timeout =
            tauri_plugin_llm_fetch::ErrorResponse::new(tauri_plugin_llm_fetch::ErrorCode::Timeout);
        assert_eq!(map_plugin_error(&timeout).code, "TIMEOUT");
        let unsafe_url = tauri_plugin_llm_fetch::ErrorResponse::new(
            tauri_plugin_llm_fetch::ErrorCode::UnsafeUrl,
        );
        assert_eq!(map_plugin_error(&unsafe_url).code, "UNSAFE_URL");
        let dns = tauri_plugin_llm_fetch::ErrorResponse::new(
            tauri_plugin_llm_fetch::ErrorCode::DnsFailure,
        );
        assert_eq!(map_plugin_error(&dns).code, "DNS_FAILURE");
        assert!(map_plugin_error(&dns).retryable);
        let failed = tauri_plugin_llm_fetch::ErrorResponse::new(
            tauri_plugin_llm_fetch::ErrorCode::NavigationFailed,
        );
        let mapped = map_plugin_error(&failed);
        assert_eq!(mapped.code, "web-fetch-unavailable");
        assert!(!mapped.safe_message.contains("http"));
    }

    #[test]
    fn guard_decisions_are_preserved_without_downgrading() {
        use tauri_plugin_llm_fetch::GuardDecision as Decision;
        assert_eq!(guard_decision_label(Decision::Allow), "allow");
        assert_eq!(
            guard_decision_label(Decision::AllowWithWarning),
            "allow_with_warning"
        );
        assert_eq!(
            guard_decision_label(Decision::RequireApproval),
            "require_approval"
        );
        assert_eq!(guard_decision_label(Decision::Deny), "deny");
    }

    #[test]
    fn projection_truncation_preserves_document_whitespace() {
        assert_eq!(
            truncate_to_model_max("first\n\nsecond", 20),
            "first\n\nsecond"
        );
        assert_eq!(truncate_to_model_max("あいうえお", 3), "あいう");
    }
}
