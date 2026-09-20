use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex,
};

#[derive(Default)]
pub(crate) struct Handle {
    enabled: AtomicBool,
    calendar_ready: AtomicBool,
    actions: Mutex<Vec<String>>,
    api_calls: Mutex<Vec<String>>,
    http_base: Mutex<Option<String>>,
    access: Mutex<Option<String>>,
    refresh: Mutex<Option<String>>,
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
        if let Ok(mut slot) = self.access.lock() {
            *slot = (!token.is_empty()).then(|| token.to_string());
        }
    }
    pub(crate) fn access(&self) -> Option<String> {
        self.access.lock().ok().and_then(|value| value.clone())
    }
    pub(crate) fn set_refresh(&self, token: &str) {
        if let Ok(mut slot) = self.refresh.lock() {
            *slot = (!token.is_empty()).then(|| token.to_string());
        }
    }
    pub(crate) fn refresh(&self) -> Option<String> {
        self.refresh.lock().ok().and_then(|value| value.clone())
    }
}
