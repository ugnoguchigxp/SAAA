//! `fetch_content` via the Tauri WebView worker (WF-04 / WF-05).
//!
//! Product path is one-shot: 1 tool call = 1 request ID = 1 one-shot
//! worker, destroyed on success, failure, or cancel. The adapter owns no
//! frontend IPC; it calls `LlmFetchManager` directly from the Rust backend.

use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use tauri::Runtime;

use super::contracts::{FetchContentInput, WebFetchCancel, WebFetchFailure};

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
                    Ok(Ok(document)) => Ok(project_document(&document, request.max_characters)),
                    Ok(Err(error)) => Err(map_plugin_error(&error)),
                    Err(_) => Err(WebFetchFailure::unavailable()),
                }
            }
        }
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

fn project_document(
    document: &tauri_plugin_llm_fetch::RetrievedDocument,
    model_max_characters: usize,
) -> FetchContentResult {
    let decision = guard_decision_label(document.security.decision.clone());
    let mut warning_categories: Vec<String> = document
        .security
        .findings
        .iter()
        .filter(|finding| {
            !matches!(
                finding.category,
                tauri_plugin_llm_fetch::SecurityFindingCategory::BenignMention
            )
        })
        .map(|finding| {
            serde_json::to_value(&finding.category)
                .and_then(serde_json::from_value::<String>)
                .unwrap_or_else(|_| "unknown".to_string())
        })
        .collect();
    warning_categories.sort();
    warning_categories.dedup();
    FetchContentResult {
        final_url: document.final_url.clone(),
        text: truncate_to_model_max(&document.text, model_max_characters),
        fetched_at: document.fetched_at.clone(),
        truncated: document.truncated || document.text.chars().count() > model_max_characters,
        decision,
        warning_categories,
    }
}

fn guard_decision_label(decision: tauri_plugin_llm_fetch::GuardDecision) -> &'static str {
    use tauri_plugin_llm_fetch::GuardDecision as Decision;
    match decision {
        Decision::Allow => "allow",
        Decision::AllowWithWarning => "allow_with_warning",
        Decision::RequireApproval => "require_approval",
        Decision::Deny => "deny",
    }
}

/// Trim the extracted text down to the model's `maxCharacters` ask without
/// splitting UTF-8. The plugin floor is 1_000 chars; smaller asks are served
/// from the same extraction and marked truncated.
fn truncate_to_model_max(text: &str, model_max: usize) -> String {
    if text.chars().count() <= model_max {
        return text.to_string();
    }
    text.chars().take(model_max).collect()
}

/// Render the compact `fetch_content_result` JSON. Plugin-internal fields
/// (session ID, worker label, proxy URL, stages, raw exceptions) are never
/// included.
pub fn render_compact(result: &FetchContentResult) -> String {
    serde_json::json!({
        "type": "fetch_content_result",
        "security": {
            "trust": "untrusted",
            "tainted": true,
            "decision": result.decision,
            "warningCategories": result.warning_categories,
        },
        "document": {
            "url": result.final_url,
            "text": result.text,
            "fetchedAt": result.fetched_at,
            "truncated": result.truncated,
        }
    })
    .to_string()
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
