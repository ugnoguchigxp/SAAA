//! UI-independent drain of ready work, recovery, and report delivery.
use crate::AppState;
use serde_json::json;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use tauri::Emitter;
use tokio::sync::Notify;

#[derive(Clone, Default)]
pub(crate) struct Wake {
    inner: Arc<Inner>,
}

#[derive(Default)]
struct Inner {
    notify: Notify,
    pending: AtomicBool,
    emits: Mutex<Vec<serde_json::Value>>,
    app: Mutex<Option<tauri::AppHandle>>,
}

impl Wake {
    pub(crate) fn bind(&self, app: tauri::AppHandle) {
        if let Ok(mut slot) = self.inner.app.lock() {
            *slot = Some(app);
        }
    }

    pub(crate) fn signal(&self) {
        self.inner.pending.store(true, Ordering::SeqCst);
        self.inner.notify.notify_waiters();
        self.inner.notify.notify_one();
    }

    pub(crate) async fn notified(&self) {
        self.inner.notify.notified().await;
    }

    pub(crate) fn take_pending(&self) -> bool {
        self.inner.pending.swap(false, Ordering::SeqCst)
    }

    pub(crate) fn record_emit(&self, value: serde_json::Value) {
        if let Ok(mut slot) = self.inner.emits.lock() {
            slot.push(value);
        }
    }

    pub(crate) fn emitted(&self) -> Vec<serde_json::Value> {
        self.inner
            .emits
            .lock()
            .map(|value| value.clone())
            .unwrap_or_default()
    }

    pub(crate) fn emit_report(
        &self,
        conversation_id: &str,
        message_id: &str,
        revision: i64,
        cursor: i64,
    ) {
        let payload = json!({
            "conversationId": conversation_id,
            "messageId": message_id,
            "reportRevision": revision,
            "cursor": cursor,
        });
        self.record_emit(payload.clone());
        if let Ok(app) = self.inner.app.lock() {
            if let Some(app) = app.as_ref() {
                let _ = app.emit("delegated-report-committed", payload);
            }
        }
    }
}

pub(crate) fn drain(state: &AppState) -> Result<(), String> {
    recover_conversations(state)?;
    for _ in 0..8 {
        super::dispatch::start_next_global(state)?;
    }
    super::report::flush_all_held_reports(state)?;
    Ok(())
}

fn recover_conversations(state: &AppState) -> Result<(), String> {
    let conversations = state.sqlite_readers.read(|connection| {
        let mut statement = connection
            .prepare(
                "SELECT DISTINCT conversation_id FROM steward_tasks
                 WHERE loop_state IN ('queued','running','dispatching','verifying','awaiting_user')
                 UNION
                 SELECT DISTINCT conversation_id FROM steward_reports WHERE flushed=0 AND invalidated=0",
            )
            .map_err(crate::database_error)?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(crate::database_error)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(crate::database_error)
    })?;
    for conversation_id in conversations {
        state.sqlite_writer.write(|connection| {
            super::driver::consume(connection)?;
            super::repository::sync_from_coding(connection, &conversation_id)
        })?;
    }
    Ok(())
}
