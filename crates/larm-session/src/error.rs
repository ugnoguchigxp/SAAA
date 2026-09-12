use crate::Session;
use std::sync::Arc;

/// A startup failure retains a connection whose release has not been confirmed.
/// Callers can retry cleanup instead of losing the only handle to the lease.
#[derive(Clone)]
pub struct ConnectError {
    pub code: &'static str,
    pub cleanup: Option<Arc<Session>>,
}
impl From<&'static str> for ConnectError {
    fn from(code: &'static str) -> Self {
        Self {
            code,
            cleanup: None,
        }
    }
}
impl std::fmt::Debug for ConnectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConnectError")
            .field("code", &self.code)
            .field("release_pending", &self.cleanup.is_some())
            .finish()
    }
}
impl std::fmt::Display for ConnectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}{}",
            self.code,
            if self.cleanup.is_some() {
                "; larm_release_pending"
            } else {
                ""
            }
        )
    }
}
impl std::error::Error for ConnectError {}
