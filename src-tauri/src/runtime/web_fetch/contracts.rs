//! Shared contracts for the WebFetch runtime (WF-03).
//!
//! Model-facing tool names, strict input limits, backend selection, safe
//! errors, and the cancellation handle shared by the content and search
//! backends. No plugin or sidecar types leak through this module.

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

/// Model-facing backend selection. Chosen once per tool call, never switched
/// mid-call (a page load may have side effects, so webview failures must not
/// silently re-execute on the sidecar).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WebFetchBackend {
    /// Current Bun sidecar implementation. Rollback target.
    Sidecar,
    /// Rust search + WebView content fetch.
    Webview,
    /// The supported macOS build (deployment target 14+) uses webview;
    /// everything else uses the sidecar.
    #[default]
    Auto,
}

impl WebFetchBackend {
    pub fn resolve(self) -> ResolvedBackend {
        match self {
            Self::Sidecar => ResolvedBackend::Sidecar,
            Self::Webview => ResolvedBackend::Webview,
            Self::Auto => {
                #[cfg(target_os = "macos")]
                {
                    ResolvedBackend::Webview
                }
                #[cfg(not(target_os = "macos"))]
                {
                    ResolvedBackend::Sidecar
                }
            }
        }
    }

    pub fn from_env() -> Self {
        match std::env::var("SAAA_WEBFETCH_BACKEND")
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str()
        {
            "sidecar" => Self::Sidecar,
            "webview" => Self::Webview,
            _ => Self::Auto,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolvedBackend {
    Sidecar,
    Webview,
}

/// Cheap-to-clone cancellation handle for WebFetch operations.
///
/// The tool layer bridges [`crate::RunCancellation`] into this handle so the
/// web_fetch module never depends on `AppState` internals. Dropping the
/// handle never cancels; only an explicit `cancel()` does.
#[derive(Clone, Default)]
pub struct WebFetchCancel {
    inner: Arc<WebFetchCancelInner>,
}

#[derive(Default)]
struct WebFetchCancelInner {
    cancelled: AtomicBool,
    notify: tokio::sync::Notify,
}

impl WebFetchCancel {
    pub fn never() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        if !self.inner.cancelled.swap(true, Ordering::SeqCst) {
            self.inner.notify.notify_waiters();
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::SeqCst)
    }

    pub async fn cancelled(&self) {
        let notified = self.inner.notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if !self.is_cancelled() {
            notified.await;
        }
    }

    pub(crate) fn from_run(run: &crate::RunCancellation) -> Self {
        let handle = Self::default();
        if run.is_cancelled() {
            handle.cancel();
        }
        handle
    }

    /// Spawn a bridge that cancels this handle when `run` is cancelled.
    /// Call sites holding the live `RunCancellation` use this so outer
    /// user-cancel reaches the plugin `cancel()` path.
    pub(crate) fn bridge_run_cancellation(&self, run: &crate::RunCancellation) {
        if self.is_cancelled() || run.is_cancelled() {
            self.cancel();
            return;
        }
        let handle = self.clone();
        // Clone the run handle (cheap `Arc` clone) and watch it.
        let run = run.clone();
        tokio::spawn(async move {
            run.cancelled().await;
            handle.cancel();
        });
    }
}

/// Validated `fetch_content` input. Mirrors the strict model schema:
/// unknown fields rejected, `url` 1..=2048 chars, `maxCharacters` 200..=20000.
#[derive(Debug, Clone)]
pub struct FetchContentInput {
    pub url: String,
    pub max_characters: usize,
}

impl FetchContentInput {
    pub fn parse(arguments: &serde_json::Value) -> Result<Self, String> {
        let object = arguments
            .as_object()
            .ok_or_else(|| "Tool arguments do not match the WebFetch schema.".to_string())?;
        for key in object.keys() {
            if key != "url" && key != "maxCharacters" {
                return Err("Tool arguments do not match the WebFetch schema.".to_string());
            }
        }
        let url = object
            .get("url")
            .and_then(serde_json::Value::as_str)
            .filter(|url| !url.is_empty() && url.len() <= 2048)
            .ok_or_else(|| "Tool arguments do not match the WebFetch schema.".to_string())?;
        let max_characters = match object.get("maxCharacters") {
            None | Some(serde_json::Value::Null) => 5_000,
            Some(serde_json::Value::Number(number)) => {
                let value = number.as_u64().ok_or_else(|| {
                    "Tool arguments do not match the WebFetch schema.".to_string()
                })?;
                if !(200..=20_000).contains(&value) {
                    return Err("Tool arguments do not match the WebFetch schema.".to_string());
                }
                value as usize
            }
            Some(_) => {
                return Err("Tool arguments do not match the WebFetch schema.".to_string());
            }
        };
        Ok(Self {
            url: url.to_string(),
            max_characters,
        })
    }
}

/// Validated `web_search` input: `query` 1..=400 chars, `limit` 1..=20.
#[derive(Debug, Clone)]
pub struct SearchInput {
    pub query: String,
    pub limit: usize,
}

impl SearchInput {
    pub fn parse(arguments: &serde_json::Value) -> Result<Self, String> {
        let object = arguments
            .as_object()
            .ok_or_else(|| "Tool arguments do not match the WebFetch schema.".to_string())?;
        for key in object.keys() {
            if key != "query" && key != "limit" {
                return Err("Tool arguments do not match the WebFetch schema.".to_string());
            }
        }
        let query = object
            .get("query")
            .and_then(serde_json::Value::as_str)
            .filter(|query| !query.is_empty() && query.len() <= 400)
            .ok_or_else(|| "Tool arguments do not match the WebFetch schema.".to_string())?;
        let limit = match object.get("limit") {
            None | Some(serde_json::Value::Null) => 5,
            Some(serde_json::Value::Number(number)) => {
                let value = number.as_u64().ok_or_else(|| {
                    "Tool arguments do not match the WebFetch schema.".to_string()
                })?;
                if !(1..=20).contains(&value) {
                    return Err("Tool arguments do not match the WebFetch schema.".to_string());
                }
                value as usize
            }
            Some(_) => {
                return Err("Tool arguments do not match the WebFetch schema.".to_string());
            }
        };
        Ok(Self {
            query: query.to_string(),
            limit,
        })
    }
}

/// Failure of a WebFetch operation. `safe_message` never contains the input
/// URL, proxy details, or raw provider output.
#[derive(Debug, Clone)]
pub struct WebFetchFailure {
    pub code: &'static str,
    pub safe_message: &'static str,
    pub retryable: bool,
}

impl WebFetchFailure {
    pub const fn new(code: &'static str, safe_message: &'static str, retryable: bool) -> Self {
        Self {
            code,
            safe_message,
            retryable,
        }
    }

    pub fn unavailable() -> Self {
        Self::new(
            "web-fetch-unavailable",
            "WebFetch is temporarily unavailable.",
            true,
        )
    }

    pub fn timeout() -> Self {
        Self::new("TIMEOUT", "WebFetch exceeded the provider deadline.", true)
    }

    pub fn cancelled() -> Self {
        Self::new("CANCELLED", "WebFetch was cancelled.", false)
    }

    pub fn invalid_input() -> Self {
        Self::new(
            "INVALID_INPUT",
            "Tool arguments do not match the WebFetch schema.",
            false,
        )
    }

    pub fn unsafe_url() -> Self {
        Self::new(
            "UNSAFE_URL",
            "The URL is not allowed for public web fetching.",
            false,
        )
    }
}

/// Compact a string the same way the TypeScript sidecar does: collapse
/// whitespace, trim, truncate to `max` characters.
pub fn compact_text(value: &str, max: usize) -> String {
    let collapsed = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() > max {
        collapsed.chars().take(max).collect()
    } else {
        collapsed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn fetch_input_rejects_unknown_fields_and_out_of_range_values() {
        assert!(FetchContentInput::parse(&json!({"url": "https://example.com/"})).is_ok());
        assert!(FetchContentInput::parse(
            &json!({"url": "https://example.com/", "unexpected": true})
        )
        .is_err());
        assert!(FetchContentInput::parse(&json!({"url": ""})).is_err());
        assert!(FetchContentInput::parse(
            &json!({"url": "https://example.com/", "maxCharacters": 50})
        )
        .is_err());
        assert!(FetchContentInput::parse(
            &json!({"url": "https://example.com/", "maxCharacters": null})
        )
        .is_ok());
    }

    #[test]
    fn search_input_enforces_query_and_limit_bounds() {
        assert!(SearchInput::parse(&json!({"query": "rust", "limit": 5})).is_ok());
        assert!(SearchInput::parse(&json!({"query": "", "limit": 5})).is_err());
        assert!(SearchInput::parse(&json!({"query": "rust", "limit": 21})).is_err());
        assert!(SearchInput::parse(&json!({"query": "rust"})).is_ok());
        assert_eq!(
            SearchInput::parse(&json!({"query": "rust"})).unwrap().limit,
            5
        );
    }

    #[tokio::test]
    async fn cancellation_does_not_lose_a_notification_between_check_and_wait() {
        for _ in 0..1_000 {
            let cancellation = WebFetchCancel::never();
            let waiter = tokio::spawn({
                let cancellation = cancellation.clone();
                async move { cancellation.cancelled().await }
            });
            cancellation.cancel();
            tokio::time::timeout(std::time::Duration::from_secs(1), waiter)
                .await
                .expect("cancellation waiter must always wake")
                .expect("waiter task must finish");
        }
    }

    #[test]
    fn backend_selection_never_switches_mid_call() {
        // `resolve()` is called once per tool call; the resolved value is
        // `Copy` and has no fallback path, so mid-call switching is
        // impossible by construction.
        let resolved = WebFetchBackend::Sidecar.resolve();
        assert_eq!(resolved, ResolvedBackend::Sidecar);
        assert_eq!(WebFetchBackend::Webview.resolve(), ResolvedBackend::Webview);
    }
}
