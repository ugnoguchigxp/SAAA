//! RAII foreground reservations interrupt background work before waiting.
use super::*;
static FOREGROUND_REQUESTS: AtomicUsize = AtomicUsize::new(0);
/// A waiting request is already foreground activity; RAII also covers cancellation
/// before the background lease releases its write guard. Foregrounds share reads.
pub struct ForegroundSlot {
    _guard: Option<tokio::sync::RwLockReadGuard<'static, ()>>,
}
impl Drop for ForegroundSlot {
    fn drop(&mut self) {
        FOREGROUND_REQUESTS.fetch_sub(1, Ordering::AcqRel);
    }
}
fn reserve_foreground() -> ForegroundSlot {
    FOREGROUND_REQUESTS.fetch_add(1, Ordering::AcqRel);
    interrupt();
    ForegroundSlot { _guard: None }
}
pub(crate) fn foreground_requested() -> bool {
    FOREGROUND_REQUESTS.load(Ordering::Acquire) > 0
}
/// External process adapters reserve foreground activity without serializing
/// independent user-facing speech and answer work behind each other.
pub fn blocking_generation() -> Option<ForegroundSlot> {
    if super::super::super::control_plane::memory_enabled() {
        let mut reserved = reserve_foreground();
        reserved._guard = Some(SLOT.blocking_read());
        Some(reserved)
    } else {
        None
    }
}
pub async fn foreground() -> ForegroundSlot {
    let mut reserved = reserve_foreground();
    reserved._guard = Some(SLOT.read().await);
    reserved
}
