use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex,
};
use std::time::{Duration, Instant};
use zeroize::Zeroizing;

struct AccessToken {
    value: Zeroizing<String>,
    expires_at: Instant,
}

#[derive(Default)]
pub(crate) struct Handle {
    enabled: AtomicBool,
    calendar_ready: AtomicBool,
    actions: Mutex<Vec<String>>,
    api_calls: Mutex<Vec<String>>,
    http_base: Mutex<Option<String>>,
    access: Mutex<Option<AccessToken>>,
    refresh: Mutex<Option<Zeroizing<String>>>,
}

impl Handle {
    pub(crate) fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::SeqCst);
    }
    pub(crate) fn enabled(&self) -> bool {
        self.enabled.load(Ordering::SeqCst)
    }
    pub(crate) fn set_calendar_ready(&self, ready: bool) {
        self.calendar_ready.store(ready, Ordering::SeqCst);
    }
    pub(crate) fn calendar_ready(&self) -> bool {
        self.calendar_ready.load(Ordering::SeqCst)
    }
    pub(crate) fn record_action(&self, id: &str) {
        if let Ok(mut actions) = self.actions.lock() {
            actions.push(id.to_string());
        }
    }
    pub(crate) fn actions(&self) -> Vec<String> {
        self.actions
            .lock()
            .map(|value| value.clone())
            .unwrap_or_default()
    }
    pub(crate) fn record_api(&self, name: &str) {
        if let Ok(mut calls) = self.api_calls.lock() {
            calls.push(name.to_string());
        }
    }
    pub(crate) fn api_calls(&self) -> Vec<String> {
        self.api_calls
            .lock()
            .map(|value| value.clone())
            .unwrap_or_default()
    }
    pub(crate) fn set_http_base(&self, base: impl Into<String>) {
        if let Ok(mut slot) = self.http_base.lock() {
            *slot = Some(base.into());
        }
    }
    pub(crate) fn http_base(&self) -> String {
        self.http_base
            .lock()
            .ok()
            .and_then(|value| value.clone())
            .unwrap_or_else(|| "https://www.googleapis.com/calendar/v3".into())
    }
    pub(crate) fn set_access(&self, token: &str) {
        self.set_access_with_ttl(token, Duration::from_secs(3_600));
    }
    pub(crate) fn set_access_with_ttl(&self, token: &str, ttl: Duration) {
        if let Ok(mut slot) = self.access.lock() {
            *slot = (!token.is_empty()).then(|| AccessToken {
                value: Zeroizing::new(token.to_string()),
                expires_at: Instant::now() + ttl,
            });
        }
    }
    pub(crate) fn access(&self) -> Option<String> {
        let mut slot = self.access.lock().ok()?;
        if slot
            .as_ref()
            .is_some_and(|access| access.expires_at <= Instant::now())
        {
            *slot = None;
        }
        slot.as_ref().map(|access| (*access.value).clone())
    }
    pub(crate) fn set_refresh(&self, token: &str) {
        if let Ok(mut slot) = self.refresh.lock() {
            *slot = (!token.is_empty()).then(|| Zeroizing::new(token.to_string()));
        }
    }
    pub(crate) fn refresh(&self) -> Option<Zeroizing<String>> {
        self.refresh.lock().ok().and_then(|value| value.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expired_access_tokens_are_removed_before_use() {
        let handle = Handle::default();
        handle.set_access_with_ttl("expired", Duration::ZERO);
        assert!(handle.access().is_none());
    }
}
