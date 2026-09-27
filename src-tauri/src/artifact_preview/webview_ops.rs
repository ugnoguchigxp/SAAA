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

struct HubInner {
    sessions: HashMap<String, WebviewSession>,
    pending: VecDeque<WebviewRequest>,
    waiting: Vec<WebviewRequest>,
    finished: Vec<(String, bool)>,
    ui_commands: usize,
    execute_calls: usize,
    reloads: usize,
    virtual_elapsed_ms: u64,
    last_request_id: Option<String>,
    script: Option<AckScript>,
}

impl HubInner {
    fn new() -> Self {
        Self {
            sessions: HashMap::new(),
            pending: VecDeque::new(),
            waiting: Vec::new(),
            finished: Vec::new(),
            ui_commands: 0,
            execute_calls: 0,
            reloads: 0,
            virtual_elapsed_ms: 0,
            last_request_id: None,
            script: None,
        }
    }
}

pub(crate) struct WebviewHub {
    inner: Mutex<HubInner>,
    wake: Condvar,
}

impl WebviewHub {
    pub(crate) fn new() -> Self {
        Self {
            inner: Mutex::new(HubInner::new()),
            wake: Condvar::new(),
        }
    }

    pub(crate) fn set_script(&self, script: Option<AckScript>) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.script = script;
        }
    }

    pub(crate) fn ui_commands(&self) -> usize {
        self.inner
            .lock()
            .map(|inner| inner.ui_commands)
            .unwrap_or(0)
    }

    pub(crate) fn execute_calls(&self) -> usize {
        self.inner
            .lock()
            .map(|inner| inner.execute_calls)
            .unwrap_or(0)
    }

    pub(crate) fn last_request_id(&self) -> Option<String> {
        self.inner
            .lock()
            .ok()
            .and_then(|inner| inner.last_request_id.clone())
    }

    pub(crate) fn reset_counts(&self) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.ui_commands = 0;
            inner.execute_calls = 0;
            inner.reloads = 0;
        }
    }

    pub(crate) fn reloads(&self) -> usize {
        self.inner.lock().map(|inner| inner.reloads).unwrap_or(0)
    }

    pub(crate) fn virtual_elapsed_ms(&self) -> u64 {
        self.inner
            .lock()
            .map(|inner| inner.virtual_elapsed_ms)
            .unwrap_or(0)
    }

    pub(crate) fn generation_of(&self, conversation_id: &str) -> u64 {
        self.inner
            .lock()
            .ok()
            .and_then(|inner| {
                inner
                    .sessions
                    .get(conversation_id)
                    .map(|session| session.generation)
            })
            .unwrap_or(0)
    }

    pub(crate) fn other_artifacts(&self, conversation_id: &str) -> Vec<String> {
        self.inner
            .lock()
            .ok()
            .and_then(|inner| {
                inner
                    .sessions
                    .get(conversation_id)
                    .map(|session| session.other_artifacts.clone())
            })
            .unwrap_or_default()
    }

    pub(crate) fn labels(&self, conversation_id: &str) -> Vec<String> {
        self.inner
            .lock()
            .ok()
            .and_then(|inner| {
                inner
                    .sessions
                    .get(conversation_id)
                    .map(|session| session.labels.clone())
            })
            .unwrap_or_default()
    }

    pub(crate) fn selected(&self, conversation_id: &str) -> Option<usize> {
        self.inner.lock().ok().and_then(|inner| {
            inner
                .sessions
                .get(conversation_id)
                .and_then(|session| session.selected)
        })
    }

    pub(crate) fn report_session(&self, session: WebviewSession) {
        if session.conversation_id.is_empty() {
            return;
        }
        if let Ok(mut inner) = self.inner.lock() {
            inner
                .sessions
                .insert(session.conversation_id.clone(), session);
        }
    }

    pub(crate) fn poll_request(&self) -> Option<WebviewRequest> {
        self.inner.lock().ok()?.pending.pop_front()
    }

    pub(crate) fn complete_request(&self, request_id: &str, applied: bool) {
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        let Some(position) = inner
            .waiting
            .iter()
            .position(|request| request.request_id == request_id)
        else {
            return;
        };
        let request = inner.waiting[position].clone();
        let session = inner.sessions.get(&request.conversation_id);
        let current = session.map(|session| session.generation).unwrap_or(0);
        if current != request.generation {
            return;
        }
        inner.waiting.swap_remove(position);
        inner
            .pending
            .retain(|request| request.request_id != request_id);
        inner.finished.push((request_id.to_string(), applied));
        self.wake.notify_all();
    }

    pub(crate) fn execute(
        &self,
        operation: &str,
        index: Option<usize>,
        conversation_id: &str,
    ) -> Value {
        let request_id = uuid::Uuid::new_v4().to_string();
        let script = {
            let Ok(mut inner) = self.inner.lock() else {
                return json!({"ok": false, "reason": "webview-unavailable"});
            };
            inner.execute_calls += 1;
            if let Some(reason) = reject_before_queue(&inner, operation, index, conversation_id) {
                return json!({"ok": false, "reason": reason});
            }
            let generation = inner
                .sessions
                .get(conversation_id)
                .map(|session| session.generation)
                .unwrap_or(0);
            let request = WebviewRequest {
                request_id: request_id.clone(),
                conversation_id: conversation_id.to_string(),
                generation,
                operation: operation.to_string(),
                index,
            };
            inner.pending.push_back(request.clone());
            inner.waiting.push(request.clone());
            inner.last_request_id = Some(request.request_id);
            inner.ui_commands += 1;
            inner.script
        };
        if let Some(script) = script {
            return self.finish_scripted(&request_id, operation, conversation_id, index, script);
        }
        self.wait_for_ui(&request_id, operation)
    }

    fn finish_scripted(
        &self,
        request_id: &str,
        operation: &str,
        conversation_id: &str,
        index: Option<usize>,
        script: AckScript,
    ) -> Value {
        match script {
            AckScript::Timeout => {
                self.abandon(request_id);
                json!({"ok": false, "reason": "webview-timeout"})
            }
            AckScript::Reject => {
                self.complete_request(request_id, false);
                json!({"ok": false, "reason": "webview-state-changed"})
            }
            AckScript::Apply | AckScript::DuplicateApply | AckScript::DelayedApply => {
                if let Ok(mut inner) = self.inner.lock() {
                    if script == AckScript::DelayedApply {
                        inner.virtual_elapsed_ms = inner.virtual_elapsed_ms.saturating_add(200);
                    }
                    if let Some(session) = inner.sessions.get_mut(conversation_id) {
                        if apply_transition(session, operation, index) {
                            inner.reloads += 1;
                        }
                    }
                }
                self.complete_request(request_id, true);
                if matches!(script, AckScript::DuplicateApply | AckScript::DelayedApply) {
                    self.complete_request(request_id, false);
                }
                json!({"ok": true, "operation": operation})
            }
        }
    }

    fn abandon(&self, request_id: &str) {
        if let Ok(mut inner) = self.inner.lock() {
            inner
                .pending
                .retain(|request| request.request_id != request_id);
            inner
                .waiting
                .retain(|request| request.request_id != request_id);
        }
    }

    fn wait_for_ui(&self, request_id: &str, operation: &str) -> Value {
        let deadline = std::time::Instant::now() + Duration::from_millis(1500);
        let Ok(mut inner) = self.inner.lock() else {
            return json!({"ok": false, "reason": "webview-unavailable"});
        };
        loop {
            if let Some(position) = inner.finished.iter().position(|(id, _)| id == request_id) {
                let applied = inner.finished.swap_remove(position).1;
                return if applied {
                    json!({"ok": true, "operation": operation})
                } else {
                    json!({"ok": false, "reason": "webview-state-changed"})
                };
            }
            let now = std::time::Instant::now();
            if now >= deadline {
                inner
                    .pending
                    .retain(|request| request.request_id != request_id);
                inner
                    .waiting
                    .retain(|request| request.request_id != request_id);
                return json!({"ok": false, "reason": "webview-timeout"});
            }
            let wait = self.wake.wait_timeout(inner, deadline - now);
            inner = match wait {
                Ok((guard, _)) => guard,
                Err(_) => return json!({"ok": false, "reason": "webview-unavailable"}),
            };
        }
    }
}

fn reject_before_queue(
    inner: &HubInner,
    operation: &str,
    index: Option<usize>,
    conversation_id: &str,
) -> Option<&'static str> {
    if !matches!(
        operation,
        "next_tab" | "previous_tab" | "select_tab" | "scroll" | "close_tab" | "close_all_tabs"
    ) {
        return Some("webview-operation-unknown");
    }
    let Some(session) = inner.sessions.get(conversation_id) else {
        return Some("webview-not-operable");
    };
    if !session.mounted || session.conversation_id != conversation_id {
        return Some("webview-not-operable");
    }
    if operation != "close_all_tabs" && session.selected.is_none() {
        return Some("webview-not-operable");
    }
    if operation == "scroll" && !session.scrollable {
        return Some("webview-not-scrollable");
    }
    if operation == "close_all_tabs" && session.labels.is_empty() {
        return Some("no-website-tabs");
    }
    if operation == "select_tab" {
        let Some(index) = index else {
            return Some("tab-index-missing");
        };
        if index >= session.labels.len() {
            return Some("tab-index-out-of-range");
        }
    }
    if matches!(operation, "next_tab" | "previous_tab" | "close_tab") && session.labels.is_empty() {
        return Some("no-website-tabs");
    }
    None
}

/// Shared tab transition. Scroll does not move the selection. A single tab stays selected.
pub(crate) fn apply_transition(
    session: &mut WebviewSession,
    operation: &str,
    index: Option<usize>,
) -> bool {
    let before = (session.labels.clone(), session.selected);
    let count = session.labels.len();
    match operation {
        "next_tab" if count > 0 => {
            let selected = session.selected.unwrap_or(0);
            session.selected = Some((selected + 1) % count);
        }
        "previous_tab" if count > 0 => {
            let selected = session.selected.unwrap_or(0);
            session.selected = Some((selected + count - 1) % count);
        }
        "select_tab" => {
            if index.is_some_and(|index| index < count) {
                session.selected = index;
            }
        }
        "close_tab" if count > 0 => {
            if let Some(selected) = session.selected {
                if selected < count {
                    session.labels.remove(selected);
                }
            }
            session.selected = if session.labels.is_empty() {
                None
            } else {
                Some(session.selected.unwrap_or(0).min(session.labels.len() - 1))
            };
        }
        "close_all_tabs" => {
            session.labels.clear();
            session.selected = None;
            session.scrollable = false;
        }
        _ => {}
    }
    (session.labels.clone(), session.selected) != before
}

struct OfferStamp {
    conversation_id: String,
    generation: u64,
}

fn offer_stamps() -> &'static Mutex<HashMap<String, OfferStamp>> {
    static STAMPS: std::sync::OnceLock<Mutex<HashMap<String, OfferStamp>>> =
        std::sync::OnceLock::new();
    STAMPS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(crate) fn stamp_offer(execution_ref: &str, conversation_id: &str) {
    let generation = active_hub().generation_of(conversation_id);
    if let Ok(mut stamps) = offer_stamps().lock() {
        stamps.insert(
            execution_ref.to_string(),
            OfferStamp {
                conversation_id: conversation_id.to_string(),
                generation,
            },
        );
    }
}

pub(crate) fn reject_stale_offer(
    execution_ref: &str,
    conversation_id: &str,
) -> Option<&'static str> {
    let Ok(stamps) = offer_stamps().lock() else {
        return Some("webview-unavailable");
    };
    let Some(stamp) = stamps.get(execution_ref) else {
        return Some("webview-not-operable");
    };
    if stamp.conversation_id != conversation_id {
        return Some("webview-conversation-changed");
    }
    let generation = stamp.generation;
    drop(stamps);
    if generation != active_hub().generation_of(conversation_id) {
        return Some("webview-generation-changed");
    }
    None
}

pub(crate) fn is_offered_for(conversation_id: &str) -> bool {
    active_hub().is_offered(conversation_id)
}

/// Production IPC and the scripted simulator both use this port.
pub(crate) trait WebviewOperationPort: Send + Sync {
    fn report_session(&self, session: WebviewSession);
    fn poll_request(&self) -> Option<WebviewRequest>;
    fn complete_request(&self, request_id: &str, applied: bool);
    fn execute(&self, operation: &str, index: Option<usize>, conversation_id: &str) -> Value;
    fn is_offered(&self, conversation_id: &str) -> bool;
}

impl WebviewOperationPort for WebviewHub {
    fn report_session(&self, session: WebviewSession) {
        WebviewHub::report_session(self, session);
    }
    fn poll_request(&self) -> Option<WebviewRequest> {
        WebviewHub::poll_request(self)
    }
    fn complete_request(&self, request_id: &str, applied: bool) {
        WebviewHub::complete_request(self, request_id, applied);
    }
    fn execute(&self, operation: &str, index: Option<usize>, conversation_id: &str) -> Value {
        WebviewHub::execute(self, operation, index, conversation_id)
    }
    fn is_offered(&self, conversation_id: &str) -> bool {
        WebviewHub::is_offered(self, conversation_id)
    }
}

impl WebviewHub {
    fn is_offered(&self, conversation_id: &str) -> bool {
        self.inner.lock().is_ok_and(|inner| {
            inner.sessions.get(conversation_id).is_some_and(|session| {
                session.mounted && (session.selected.is_some() || !session.labels.is_empty())
            })
        })
    }
}

/// Drops `artifact_webview` before ranking when that conversation has no operable website tabs.
pub(crate) fn restrict_candidates(
    conversation_id: &str,
    mut eligible: Vec<EligibleRevision>,
    mut lexical: Vec<String>,
    mut embeddings: Vec<(String, Vec<f32>)>,
) -> (Vec<EligibleRevision>, Vec<String>, Vec<(String, Vec<f32>)>) {
    if is_offered_for(conversation_id) {
        return (eligible, lexical, embeddings);
    }
    let blocked: HashSet<String> = eligible
        .iter()
        .filter(|item| item.revision.tool_id == "artifact_webview")
        .map(|item| item.revision.id.clone())
        .collect();
    eligible.retain(|item| item.revision.tool_id != "artifact_webview");
    lexical.retain(|id| !blocked.contains(id));
    embeddings.retain(|(id, _)| !blocked.contains(id));
    (eligible, lexical, embeddings)
}

/// Failures that must not open an invocation or reach the backend.
pub(crate) fn reject_without_backend(
    conversation_id: &str,
    arguments: &Value,
) -> Option<&'static str> {
    let operation = arguments
        .get("operation")
        .and_then(Value::as_str)
        .unwrap_or("");
    let index = arguments
        .get("index")
        .and_then(Value::as_u64)
        .map(|value| value as usize);
    if !is_offered_for(conversation_id) {
        return Some("webview-not-operable");
    }
    let hub = active_hub();
    let Ok(inner) = hub.inner.lock() else {
        return Some("webview-unavailable");
    };
    match reject_before_queue(&inner, operation, index, conversation_id) {
        Some("webview-not-scrollable") => None,
        other => other,
    }
}

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
        apply_transition(&mut session, "next_tab", None);
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
