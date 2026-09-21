//! UI-independent drain of ready work, recovery, and report delivery.
use crate::AppState;
use serde_json::json;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex, OnceLock,
};
use tauri::Emitter;
use tokio::sync::Notify;

static GLOBAL: OnceLock<Wake> = OnceLock::new();

#[derive(Clone, Default)]
pub(crate) struct Wake {
    inner: Arc<Inner>,
}

#[derive(Default)]
struct Inner {
    notify: Notify,
    pending: AtomicBool,
    busy: AtomicBool,
    emits: Mutex<Vec<serde_json::Value>>,
    app: Mutex<Option<tauri::AppHandle>>,
}

impl Wake {
    pub(crate) fn bind(&self, app: tauri::AppHandle) {
        if let Ok(mut slot) = self.inner.app.lock() {
            *slot = Some(app);
        }
        let _ = GLOBAL.set(self.clone());
    }

    pub(crate) fn install_process(&self) {
        let _ = GLOBAL.set(self.clone());
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

    pub(crate) fn next_due_ms(&self, state: &AppState) -> Option<i64> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_millis() as i64)
            .unwrap_or(0);
        state
            .sqlite_readers
            .read(|connection| {
                connection
                    .query_row(
                        "SELECT MIN(available_at_ms) FROM steward_reports
                         WHERE flushed=0 AND invalidated=0 AND available_at_ms > ?1",
                        [now],
                        |row| row.get::<_, Option<i64>>(0),
                    )
                    .map_err(crate::database_error)
            })
            .ok()
            .flatten()
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

pub(crate) fn signal_committed() {
    if let Some(wake) = GLOBAL.get() {
        wake.signal();
    }
}

pub(crate) fn drain(state: &AppState) -> Result<(), String> {
    if state.steward_wake.inner.busy.swap(true, Ordering::SeqCst) {
        state
            .steward_wake
            .inner
            .pending
            .store(true, Ordering::SeqCst);
        return Ok(());
    }
    let result = drain_loop(state);
    state.steward_wake.inner.busy.store(false, Ordering::SeqCst);
    if state.steward_wake.inner.pending.load(Ordering::SeqCst) {
        state.steward_wake.signal();
    }
    result
}

fn drain_loop(state: &AppState) -> Result<(), String> {
    for _ in 0..8 {
        state
            .steward_wake
            .inner
            .pending
            .store(false, Ordering::SeqCst);
        recover_conversations(state)?;
        for _ in 0..8 {
            super::dispatch::start_next_global(state)?;
        }
        super::report::flush_all_held_reports(state)?;
        if !state.steward_wake.inner.pending.load(Ordering::SeqCst) {
            break;
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::schema::initialize_database;
    use crate::test_support::app_state;
    use rusqlite::Connection;

    #[test]
    fn rf5_w_01_signal_during_drain_is_not_dropped() {
        let wake = Wake::default();
        wake.inner.busy.store(true, Ordering::SeqCst);
        wake.signal();
        assert!(wake.inner.pending.load(Ordering::SeqCst));
        wake.inner.busy.store(false, Ordering::SeqCst);
        let connection = Connection::open_in_memory().unwrap();
        initialize_database(&connection).unwrap();
        let state = app_state(connection);
        state.steward_wake.install_process();
        drain(&state).unwrap();
        assert!(!state.steward_wake.inner.pending.load(Ordering::SeqCst));
    }
}
