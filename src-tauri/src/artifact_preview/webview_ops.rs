use std::collections::VecDeque;
use std::sync::{Condvar, Mutex};
use std::time::Duration;

use serde::Serialize;
use serde_json::{json, Value};

#[derive(Clone, Debug, Default)]
pub(crate) struct WebviewSession {
    pub conversation_id: String,
    pub generation: u64,
    pub labels: Vec<String>,
    pub selected: Option<usize>,
    pub scrollable: bool,
    pub mounted: bool,
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

struct Gate {
    session: WebviewSession,
    pending: VecDeque<WebviewRequest>,
    waiting: Vec<String>,
    finished: Vec<(String, bool)>,
}

static GATE: Mutex<Gate> = Mutex::new(Gate {
    session: WebviewSession {
        conversation_id: String::new(),
        generation: 0,
        labels: Vec::new(),
        selected: None,
        scrollable: false,
        mounted: false,
    },
    pending: VecDeque::new(),
    waiting: Vec::new(),
    finished: Vec::new(),
});
static WAKE: Condvar = Condvar::new();

pub(crate) fn is_active_for(conversation_id: &str) -> bool {
    GATE.lock().is_ok_and(|gate| {
        gate.session.conversation_id == conversation_id
            && gate.session.mounted
            && gate.session.selected.is_some()
            && gate.session.scrollable
    })
}

pub(crate) fn report_session(session: WebviewSession) {
    if let Ok(mut gate) = GATE.lock() {
        gate.session = session;
    }
}

pub(crate) fn poll_request() -> Option<WebviewRequest> {
    GATE.lock().ok()?.pending.pop_front()
}

pub(crate) fn complete_request(request_id: &str, applied: bool) {
    if let Ok(mut gate) = GATE.lock() {
        let Some(position) = gate.waiting.iter().position(|id| id == request_id) else {
            return;
        };
        gate.waiting.swap_remove(position);
        gate.pending
            .retain(|request| request.request_id != request_id);
        gate.finished.push((request_id.to_string(), applied));
        WAKE.notify_all();
    }
}

pub(crate) fn execute(operation: &str, index: Option<usize>, conversation_id: &str) -> Value {
    let request_id = uuid::Uuid::new_v4().to_string();
    {
        let Ok(mut gate) = GATE.lock() else {
            return json!({"ok": false, "reason": "webview-unavailable"});
        };
        if !matches!(
            operation,
            "next_tab" | "previous_tab" | "select_tab" | "scroll" | "close_tab" | "close_all_tabs"
        ) {
            return json!({"ok": false, "reason": "webview-operation-unknown"});
        }
        if !gate.session.mounted || gate.session.conversation_id != conversation_id {
            return json!({"ok": false, "reason": "webview-not-operable"});
        }
        if operation != "close_all_tabs" && gate.session.selected.is_none() {
            return json!({"ok": false, "reason": "webview-not-operable"});
        }
        if operation == "scroll" && !gate.session.scrollable {
            return json!({"ok": false, "reason": "webview-not-scrollable"});
        }
        if operation == "close_all_tabs" && gate.session.labels.is_empty() {
            return json!({"ok": false, "reason": "no-website-tabs"});
        }
        if operation == "select_tab" {
            let Some(index) = index else {
                return json!({"ok": false, "reason": "tab-index-missing"});
            };
            if index >= gate.session.labels.len() {
                return json!({"ok": false, "reason": "tab-index-out-of-range"});
            }
        }
        if matches!(operation, "next_tab" | "previous_tab" | "close_tab")
            && gate.session.labels.is_empty()
        {
            return json!({"ok": false, "reason": "no-website-tabs"});
        }
        let generation = gate.session.generation;
        gate.pending.push_back(WebviewRequest {
            request_id: request_id.clone(),
            conversation_id: conversation_id.to_string(),
            generation,
            operation: operation.to_string(),
            index,
        });
        gate.waiting.push(request_id.clone());
    }
    let deadline = std::time::Instant::now() + Duration::from_millis(1500);
    let Ok(mut gate) = GATE.lock() else {
        return json!({"ok": false, "reason": "webview-unavailable"});
    };
    loop {
        if let Some(position) = gate.finished.iter().position(|(id, _)| id == &request_id) {
            let applied = gate.finished.swap_remove(position).1;
            return if applied {
                json!({"ok": true, "operation": operation})
            } else {
                json!({"ok": false, "reason": "webview-state-changed"})
            };
        }
        let now = std::time::Instant::now();
        if now >= deadline {
            gate.pending
                .retain(|request| request.request_id != request_id);
            gate.waiting.retain(|id| id != &request_id);
            return json!({"ok": false, "reason": "webview-timeout"});
        }
        let wait = WAKE.wait_timeout(gate, deadline - now);
        gate = match wait {
            Ok((guard, _)) => guard,
            Err(_) => return json!({"ok": false, "reason": "webview-unavailable"}),
        };
    }
}
