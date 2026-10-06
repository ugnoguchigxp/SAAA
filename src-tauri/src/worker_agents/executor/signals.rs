//! In-process wakeups and cancellation handles, keyed by task id. Truth always lives in SQLite;
//! these only shorten waits and let a cancel reach a running attempt.
use crate::RunCancellation;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex, PoisonError};
use tokio::sync::Notify;

static NOTIFIERS: LazyLock<Mutex<HashMap<String, Arc<Notify>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static ACTIVE: LazyLock<Mutex<HashMap<String, RunCancellation>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub(super) fn notifier(task_id: &str) -> Arc<Notify> {
    NOTIFIERS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .entry(task_id.to_string())
        .or_default()
        .clone()
}

/// Wakes a waiter. `notify_one` stores a permit, so a waiter that has not parked yet still wakes.
/// The entry is dropped when nobody else holds it; a later waiter re-reads the database first.
pub(super) fn wake(task_id: &str) {
    let mut map = NOTIFIERS.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(notify) = map.get(task_id) {
        notify.notify_one();
        if Arc::strong_count(notify) == 1 {
            map.remove(task_id);
        }
    }
}

pub(super) fn forget(task_id: &str) {
    NOTIFIERS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .remove(task_id);
}

/// Registration of the cancellation handle of a task being processed. Dropping it unregisters.
pub(super) struct ActiveGuard {
    task_id: String,
}

impl Drop for ActiveGuard {
    fn drop(&mut self) {
        ACTIVE
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.task_id);
    }
}

/// `None` when this task is already being processed in this process (a duplicate queue job).
pub(super) fn register(task_id: &str, cancellation: &RunCancellation) -> Option<ActiveGuard> {
    let mut map = ACTIVE.lock().unwrap_or_else(PoisonError::into_inner);
    if map.contains_key(task_id) {
        return None;
    }
    map.insert(task_id.to_string(), cancellation.clone());
    Some(ActiveGuard {
        task_id: task_id.to_string(),
    })
}

/// Fires the in-process cancellation of a running task, if any.
pub(super) fn fire(task_id: &str) {
    let handle = ACTIVE
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(task_id)
        .cloned();
    if let Some(handle) = handle {
        handle.cancel();
    }
    wake(task_id);
}
