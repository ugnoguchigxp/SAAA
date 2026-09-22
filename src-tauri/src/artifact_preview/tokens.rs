use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use uuid::Uuid;

use super::{catalog::ArtifactRevision, contracts::MAX_ACTIVE_TOKENS};

#[derive(Clone)]
pub(crate) struct PreviewRuntime {
    inner: Arc<Mutex<Ledger>>,
    now_ms: Arc<dyn Fn() -> i64 + Send + Sync>,
}

struct Ledger {
    tokens: HashMap<String, TokenEntry>,
    last_denial: Option<&'static str>,
}

#[derive(Clone, Debug)]
pub(crate) struct TokenEntry {
    pub artifact_id: String,
    pub revision_id: String,
    pub digest: String,
    pub scope: String,
    pub webview_label: String,
    pub expires_at_ms: i64,
}

impl Default for PreviewRuntime {
    fn default() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Ledger {
                tokens: HashMap::new(),
                last_denial: None,
            })),
            now_ms: Arc::new(wall_clock_ms),
        }
    }
}

impl PreviewRuntime {
    #[cfg(test)]
    pub(crate) fn with_clock(now_ms: Arc<dyn Fn() -> i64 + Send + Sync>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Ledger {
                tokens: HashMap::new(),
                last_denial: None,
            })),
            now_ms,
        }
    }

    pub(crate) fn issue(
        &self,
        revision: &ArtifactRevision,
        ttl_ms: i64,
    ) -> Result<(String, String, i64), &'static str> {
        let mut ledger = self.lock();
        let now = (self.now_ms)();
        ledger.expire(now);
        if ledger.tokens.len() >= MAX_ACTIVE_TOKENS {
            return Err("preview-token-limit");
        }
        let token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
        let label_id = Uuid::new_v4().simple().to_string();
        let label = format!("artifact-preview-{}", &label_id[..12]);
        let expires_at_ms = now.saturating_add(ttl_ms);
        ledger.tokens.insert(
            token.clone(),
            TokenEntry {
                artifact_id: revision.artifact_id.into(),
                revision_id: revision.revision_id.into(),
                digest: revision.digest(),
                scope: revision.scope.into(),
                webview_label: label.clone(),
                expires_at_ms,
            },
        );
        Ok((token, label, expires_at_ms))
    }

    pub(crate) fn release(&self, token: &str) {
        let mut ledger = self.lock();
        ledger.tokens.remove(token);
    }

    pub(crate) fn lookup_active(&self, token: &str) -> Result<TokenEntry, &'static str> {
        let mut ledger = self.lock();
        let now = (self.now_ms)();
        let expired = ledger
            .tokens
            .get(token)
            .is_some_and(|entry| entry.expires_at_ms <= now);
        if expired {
            ledger.tokens.remove(token);
            ledger.last_denial = Some("token-expired");
            return Err("token-expired");
        }
        ledger.expire(now);
        match ledger.tokens.get(token) {
            Some(entry) => Ok(entry.clone()),
            None => {
                ledger.last_denial = Some("token-unknown");
                Err("token-unknown")
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn active_count(&self) -> usize {
        let mut ledger = self.lock();
        ledger.expire((self.now_ms)());
        ledger.tokens.len()
    }

    #[cfg(test)]
    pub(crate) fn last_denial(&self) -> Option<&'static str> {
        self.lock().last_denial
    }

    pub(crate) fn record_denial(&self, reason: &'static str) {
        self.lock().last_denial = Some(reason);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Ledger> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl Ledger {
    fn expire(&mut self, now: i64) {
        self.tokens.retain(|_, entry| entry.expires_at_ms > now);
    }
}

fn wall_clock_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

pub(crate) fn iso_from_millis(ms: i64) -> String {
    let seconds = ms.div_euclid(1000);
    let nanos = (ms.rem_euclid(1000) * 1_000_000) as u32;
    chrono::DateTime::from_timestamp(seconds, nanos)
        .map(|time| time.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
        .unwrap_or_else(|| "1970-01-01T00:00:00.000Z".into())
}
