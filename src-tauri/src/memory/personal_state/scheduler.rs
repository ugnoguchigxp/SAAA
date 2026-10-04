use crate::RunCancellation;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, LazyLock, Mutex,
};
pub(super) static SLOT: LazyLock<tokio::sync::RwLock<()>> =
    LazyLock::new(|| tokio::sync::RwLock::new(()));
pub(super) static BACKGROUND: Mutex<Option<Arc<RunCancellation>>> = Mutex::new(None);
pub fn interrupt() {
    if let Ok(slot) = BACKGROUND.lock() {
        if let Some(cancel) = slot.as_ref() {
            cancel.cancel();
        }
    }
}
mod foreground;
pub(super) use foreground::foreground_requested;
#[cfg(any(test, feature = "offline-contracts"))]
pub use foreground::ForegroundSlot;
pub use foreground::{blocking_generation, foreground};
pub fn generation_slot_busy() -> bool {
    foreground_requested() || SLOT.try_write().is_err()
}
#[cfg(test)]
pub fn occupy_for_test() -> tokio::sync::RwLockWriteGuard<'static, ()> {
    SLOT.blocking_write()
}

mod background;
pub use background::background;

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn world_maintenance_foregrounds_share_reads_and_cancel_background_before_waiting() {
        let background = SLOT.write().await;
        let cancel = Arc::new(RunCancellation::default());
        *BACKGROUND.lock().unwrap() = Some(cancel.clone());
        let first = tokio::spawn(foreground());
        tokio::time::timeout(std::time::Duration::from_secs(2), cancel.cancelled())
            .await
            .unwrap();
        assert!(foreground_requested());
        assert!(generation_slot_busy());
        drop(background);
        let first = tokio::time::timeout(std::time::Duration::from_secs(2), first)
            .await
            .unwrap()
            .unwrap();
        let second = tokio::time::timeout(std::time::Duration::from_secs(2), foreground())
            .await
            .unwrap();
        assert!(generation_slot_busy());
        drop(second);
        drop(first);
        *BACKGROUND.lock().unwrap() = None;
    }
}
