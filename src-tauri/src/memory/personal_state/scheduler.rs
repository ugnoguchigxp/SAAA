use crate::RunCancellation;
use std::sync::{Arc, LazyLock, Mutex};
pub(super) static SLOT: LazyLock<tokio::sync::Mutex<()>> =
    LazyLock::new(|| tokio::sync::Mutex::new(()));
pub(super) static BACKGROUND: Mutex<Option<Arc<RunCancellation>>> = Mutex::new(None);
pub fn interrupt() {
    if let Ok(slot) = BACKGROUND.lock() {
        if let Some(cancel) = slot.as_ref() {
            cancel.cancel();
        }
    }
}
/// External process adapters cannot expose individual model-call boundaries;
/// conservatively reserve the shared slot for the process while Personal State is on.
pub fn blocking_generation() -> Option<tokio::sync::MutexGuard<'static, ()>> {
    if super::super::control_plane::memory_enabled() {
        interrupt();
        Some(SLOT.blocking_lock())
    } else {
        None
    }
}
pub async fn foreground() -> tokio::sync::MutexGuard<'static, ()> {
    interrupt();
    SLOT.lock().await
}
pub fn generation_slot_busy() -> bool {
    SLOT.try_lock().is_err()
}
#[cfg(test)]
pub fn occupy_for_test() -> tokio::sync::MutexGuard<'static, ()> {
    SLOT.blocking_lock()
}
