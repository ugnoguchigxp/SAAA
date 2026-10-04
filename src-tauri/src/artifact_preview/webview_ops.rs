//! Conversation-scoped website-tab operations. Production waits for the UI ack. The simulator
//! uses the same checks with a scripted ack so a scenario never sleeps for the live timeout.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use serde::Serialize;
use serde_json::{json, Value};

use crate::tool_selection::repository::EligibleRevision;

#[derive(Clone, Debug, Default)]
pub(crate) struct WebviewSession {
    pub conversation_id: String,
    pub generation: u64,
    pub labels: Vec<String>,
    pub selected: Option<usize>,
    pub scrollable: bool,
    pub mounted: bool,
    /// Non-website artifacts kept beside the website tabs. Operations do not touch them.
    pub other_artifacts: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WebviewRequest {
    pub request_id: String,
    pub conversation_id: String,
    pub generation: u64,
    pub operation: String,
    pub index: Option<usize>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AckScript {
    /// Apply the shared transition, then ack once.
    Apply,
    /// Advance a virtual clock, then apply once. A later ack does not apply again.
    DelayedApply,
    /// Ack `applied=false` without changing tabs.
    Reject,
    /// Leave the request unanswered and record a timeout.
    Timeout,
    /// Apply once. A second ack for the same request is ignored.
    DuplicateApply,
}

/// Production IPC and the scripted simulator both use this port.
pub(crate) trait WebviewOperationPort: Send + Sync {
    fn report_session(&self, session: WebviewSession);
    fn poll_request(&self) -> Option<WebviewRequest>;
    fn complete_request(&self, request_id: &str, applied: bool);
    fn execute(&self, operation: &str, index: Option<usize>, conversation_id: &str) -> Value;
    fn is_offered(&self, conversation_id: &str) -> bool;
}

#[path = "webview_ops/hub.rs"]
mod hub;
#[path = "webview_ops/offers.rs"]
mod offers;
pub(crate) use hub::{reject_without_backend, WebviewHub};
pub(crate) use offers::{is_offered_for, reject_stale_offer, restrict_candidates, stamp_offer};

fn global_hub() -> Arc<WebviewHub> {
    static HUB: std::sync::OnceLock<Arc<WebviewHub>> = std::sync::OnceLock::new();
    HUB.get_or_init(|| Arc::new(WebviewHub::new())).clone()
}

static OVERRIDE: Mutex<Option<Arc<WebviewHub>>> = Mutex::new(None);

fn active_hub() -> Arc<WebviewHub> {
    OVERRIDE
        .lock()
        .ok()
        .and_then(|slot| slot.clone())
        .unwrap_or_else(global_hub)
}

pub(crate) fn install_hub(hub: Arc<WebviewHub>) {
    if let Ok(mut slot) = OVERRIDE.lock() {
        *slot = Some(hub);
    }
}

pub(crate) fn clear_hub_override() {
    if let Ok(mut slot) = OVERRIDE.lock() {
        *slot = None;
    }
}

#[cfg(any(test, feature = "offline-contracts"))]
pub(crate) fn is_active_for(conversation_id: &str) -> bool {
    is_offered_for(conversation_id)
}

pub(crate) fn report_session(session: WebviewSession) {
    active_hub().report_session(session);
}

pub(crate) fn poll_request() -> Option<WebviewRequest> {
    active_hub().poll_request()
}

pub(crate) fn complete_request(request_id: &str, applied: bool) {
    active_hub().complete_request(request_id, applied);
}

pub(crate) fn execute(operation: &str, index: Option<usize>, conversation_id: &str) -> Value {
    active_hub().execute(operation, index, conversation_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_tab_next_keeps_selection() {
        let mut session = WebviewSession {
            conversation_id: "c".into(),
            labels: vec!["a".into()],
            selected: Some(0),
            mounted: true,
            ..WebviewSession::default()
        };
        hub::apply_transition(&mut session, "next_tab", None);
        assert_eq!(session.selected, Some(0));
        assert_eq!(session.labels, vec!["a".to_string()]);
    }

    #[test]
    fn late_ack_after_timeout_does_not_apply() {
        let hub = WebviewHub::new();
        hub.set_script(Some(AckScript::Timeout));
        hub.report_session(WebviewSession {
            conversation_id: "c".into(),
            generation: 1,
            labels: vec!["a".into(), "b".into()],
            selected: Some(0),
            scrollable: true,
            mounted: true,
            ..WebviewSession::default()
        });
        let result = hub.execute("next_tab", None, "c");
        assert_eq!(result["reason"], "webview-timeout");
        hub.complete_request("missing", true);
        assert_eq!(hub.selected("c"), Some(0));
        hub.set_script(Some(AckScript::Apply));
        let result = hub.execute("next_tab", None, "c");
        assert_eq!(result["ok"], true);
        assert_eq!(hub.selected("c"), Some(1));
    }
}
