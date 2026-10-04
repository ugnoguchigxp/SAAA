use super::*;
/// A bounded terminal event decision shares the maintenance lane and yields to a human turn.
pub struct BackgroundSlot {
    _guard: tokio::sync::RwLockWriteGuard<'static, ()>,
}
impl Drop for BackgroundSlot {
    fn drop(&mut self) {
        if let Ok(mut slot) = BACKGROUND.lock() {
            *slot = None;
        }
    }
}
pub async fn background(cancellation: Arc<RunCancellation>) -> BackgroundSlot {
    let guard = SLOT.write().await;
    if let Ok(mut slot) = BACKGROUND.lock() {
        *slot = Some(cancellation.clone());
    }
    if foreground_requested() {
        cancellation.cancel();
    }
    BackgroundSlot { _guard: guard }
}
