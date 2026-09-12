//! One lease for the entire voice conversation. Read guards pin all four tokens;
//! renew/reclaim and release take the writer lock and cannot overtake inference.
mod contract;
mod error;
mod http;
use contract::Snapshot;
pub use contract::{local_url, Provider};
pub use error::ConnectError;
use serde_json::json;
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use tokio::sync::{watch, OwnedRwLockReadGuard, RwLock};

pub struct Session {
    client: reqwest::Client,
    connection: url::Url,
    id: String,
    snapshot: Arc<RwLock<Option<Snapshot>>>,
    closed: AtomicBool,
    released: AtomicBool,
    stop: watch::Sender<bool>,
}
pub struct Use {
    snapshot: OwnedRwLockReadGuard<Option<Snapshot>>,
    name: String,
    _session: Arc<Session>,
}
impl Use {
    pub fn provider(&self) -> &Provider {
        &self.snapshot.as_ref().expect("live snapshot").providers[&self.name]
    }
    pub fn allocation_id(&self) -> &str {
        &self.snapshot.as_ref().expect("live snapshot").allocation_id
    }
}
impl Session {
    pub async fn connect(
        base: &str,
        cancellation: watch::Receiver<bool>,
    ) -> Result<Arc<Self>, ConnectError> {
        let base = base.to_string();
        let (alive, abandoned) = watch::channel(false);
        let (send, receive) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let result = Self::connect_inner(&base, cancellation, abandoned).await;
            if let Err(result) = send.send(result) {
                let cleanup = match result {
                    Ok(session) => Some(session),
                    Err(error) => error.cleanup,
                };
                if let Some(session) = cleanup {
                    let _ = session.close().await;
                }
            }
        });
        let result = receive
            .await
            .map_err(|_| ConnectError::from("larm_startup_task_failed"))?;
        drop(alive);
        result
    }
    async fn connect_inner(
        base: &str,
        mut cancellation: watch::Receiver<bool>,
        mut abandoned: watch::Receiver<bool>,
    ) -> Result<Arc<Self>, ConnectError> {
        let started = Instant::now();
        if *cancellation.borrow() {
            return Err("larm_cancelled".into());
        }
        let mut base = url::Url::parse(base).map_err(|_| "larm_invalid_control_url")?;
        if !local_url(&base) || base.path() != "/" {
            return Err("larm_invalid_control_url".into());
        }
        base.set_path("/v1/agent-connections");
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|_| "larm_client_failed")?;
        // Do not race creation against cancellation: receive the id, then release it.
        let created = http::json(
            client
                .post(base.clone())
                .header(
                    "Idempotency-Key",
                    format!("saaa-session-{}", uuid::Uuid::new_v4()),
                )
                .json(
                    &json!({"agentProfile":"saaa-qwen38-kv-mem","explicitAgentProfile":true,
                "audience":"saaa-desktop","client":"saaa-coding-agent","ttlSeconds":600,
                "allowFallback":false,"deploymentPolicy":"existing-only"}),
                ),
            &[201, 202],
        )
        .await?;
        let id = contract::string(&created, "id")?.to_string();
        if id.len() > 160
            || !id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'))
        {
            return Err("larm_invalid_connection_id".into());
        }
        base.path_segments_mut()
            .map_err(|_| "larm_invalid_control_url")?
            .push(&id);
        let (stop, _) = watch::channel(false);
        let session = Arc::new(Self {
            client,
            connection: base,
            id: id.clone(),
            snapshot: Arc::new(RwLock::new(None)),
            closed: AtomicBool::new(false),
            released: AtomicBool::new(false),
            stop,
        });
        let startup = async {
            let mut state = created;
            loop {
                match state["status"].as_str() {
                    Some("ready") => break,
                    Some("pending" | "probing") => {}
                    _ => return Err("larm_startup_terminal"),
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
                state = http::json(session.client.get(session.connection.clone()), &[200]).await?;
                if state["id"] != id {
                    return Err("larm_connection_mismatch");
                }
            }
            let snapshot = session.claim().await?;
            *session.snapshot.write().await = Some(snapshot);
            Ok(())
        };
        let result = tokio::select! { biased;
            _ = cancelled(&mut cancellation) => Err("larm_cancelled"),
            _ = cancelled(&mut abandoned) => Err("larm_cancelled"),
            result = tokio::time::timeout(Duration::from_secs(300).saturating_sub(started.elapsed()), startup) => result.unwrap_or(Err("larm_startup_timeout")),
        };
        if let Err(error) = result {
            return match session.close().await {
                Ok(()) => Err(error.into()),
                Err(_) => Err(ConnectError {
                    code: error,
                    cleanup: Some(session),
                }),
            };
        }
        let weak = Arc::downgrade(&session);
        let mut stop = session.stop.subscribe();
        tokio::spawn(async move {
            loop {
                tokio::select! { biased;
                    _ = cancelled(&mut stop) => break,
                    _ = tokio::time::sleep(Duration::from_secs(1)) => {},
                }
                let Some(session) = weak.upgrade() else {
                    break;
                };
                if session.renew_if_due().await.is_err() {
                    let _ = session.close().await;
                    break;
                }
            }
        });
        Ok(session)
    }
    fn operation(&self, name: &str) -> url::Url {
        let mut url = self.connection.clone();
        url.path_segments_mut().expect("validated base").push(name);
        url
    }
    async fn claim(&self) -> Result<Snapshot, &'static str> {
        let value = http::json(
            self.client
                .post(self.operation("claim"))
                .json(&json!({"format":"openai-provider-v1"})),
            &[200],
        )
        .await?;
        let snapshot = contract::parse(value, &self.id)?;
        for provider in snapshot.providers.values() {
            self.health(provider).await?;
        }
        Ok(snapshot)
    }
    async fn health(&self, provider: &Provider) -> Result<(), &'static str> {
        let mut checked = provider.checked_at.lock().await;
        if checked.is_some_and(|at| at.elapsed() < provider.max_age) {
            return Ok(());
        }
        let at = Instant::now();
        let value = http::json(
            self.client
                .get(provider.health_url.clone())
                .bearer_auth(provider.token()),
            &[200],
        )
        .await?;
        if value["ready"] != true
            || value["acceptingRequests"] != true
            || value["probe"]["validated"] != true
            || value["probe"]["protocol"] != provider.protocol
        {
            return Err("larm_unhealthy_provider");
        }
        if at.elapsed() >= provider.max_age {
            return Err("larm_stale_health");
        }
        *checked = Some(at);
        Ok(())
    }
    pub async fn acquire(self: &Arc<Self>, name: &str) -> Result<Use, &'static str> {
        if self.closed.load(Ordering::Acquire) {
            return Err("larm_session_closed");
        }
        self.renew_if_due().await?;
        let guard = self.snapshot.clone().read_owned().await;
        if self.closed.load(Ordering::Acquire) {
            return Err("larm_session_closed");
        }
        let snapshot = guard.as_ref().ok_or("larm_session_unavailable")?;
        if snapshot.expires_at <= chrono::Utc::now() {
            return Err("larm_expired");
        }
        let provider = snapshot
            .providers
            .get(name)
            .ok_or("larm_unknown_provider")?;
        if let Err(error) = self.health(provider).await {
            drop(guard);
            let _ = self.close().await;
            return Err(error);
        }
        Ok(Use {
            snapshot: guard,
            name: name.to_string(),
            _session: self.clone(),
        })
    }
    pub async fn renew_if_due(self: &Arc<Self>) -> Result<(), &'static str> {
        let due =
            self.snapshot.read().await.as_ref().is_some_and(|s| {
                s.expires_at <= chrono::Utc::now() + chrono::Duration::seconds(90)
            });
        if !due {
            return if self.closed.load(Ordering::Acquire) {
                Err("larm_session_closed")
            } else {
                Ok(())
            };
        }
        let session = self.clone();
        tokio::spawn(async move { session.renew_inner().await })
            .await
            .map_err(|_| "larm_renew_failed")?
    }
    async fn renew_inner(self: &Arc<Self>) -> Result<(), &'static str> {
        if self.closed.load(Ordering::Acquire) {
            return Err("larm_session_closed");
        }
        let due = |s: &Option<Snapshot>| {
            s.as_ref()
                .is_some_and(|s| s.expires_at <= chrono::Utc::now() + chrono::Duration::seconds(90))
        };
        if !due(&*self.snapshot.read().await) {
            return Ok(());
        }
        let mut snapshot = self.snapshot.write().await;
        if self.closed.load(Ordering::Acquire) {
            return Err("larm_session_closed");
        }
        if !due(&snapshot) {
            return Ok(());
        }
        // Invalidate locally BEFORE renew: an ambiguous HTTP failure must never revive old tokens.
        *snapshot = None;
        let result = async {
            let renewed = http::json(
                self.client
                    .post(self.operation("renew"))
                    .header(
                        "Idempotency-Key",
                        format!("saaa-renew-{}", uuid::Uuid::new_v4()),
                    )
                    .json(&json!({"ttlSeconds":600})),
                &[200],
            )
            .await?;
            if renewed["id"] != self.id {
                return Err("larm_connection_mismatch");
            }
            contract::expiry(&renewed)?;
            let next = self.claim().await?;
            if next.expires_at <= chrono::Utc::now() + chrono::Duration::seconds(90) {
                return Err("larm_renew_too_short");
            }
            Ok(next)
        }
        .await;
        match result {
            Ok(next) => {
                *snapshot = Some(next);
                Ok(())
            }
            Err(error) => {
                self.closed.store(true, Ordering::Release);
                drop(snapshot);
                let _ = self.close().await;
                Err(error)
            }
        }
    }
    pub async fn close(self: &Arc<Self>) -> Result<(), &'static str> {
        self.closed.store(true, Ordering::Release);
        self.stop.send_replace(true);
        let session = self.clone();
        tokio::spawn(async move { session.close_inner().await })
            .await
            .map_err(|_| "larm_release_task_failed")?
    }
    async fn close_inner(&self) -> Result<(), &'static str> {
        self.closed.store(true, Ordering::Release);
        self.stop.send_replace(true);
        let mut snapshot = self.snapshot.write().await;
        *snapshot = None;
        if self.released.load(Ordering::Acquire) {
            return Ok(());
        }
        if let Err(error) = release(&self.client, self.connection.clone()).await {
            eprintln!("LARM release failed; connection remains closed and can be released again");
            return Err(error);
        }
        self.released.store(true, Ordering::Release);
        Ok(())
    }
}
async fn cancelled(receiver: &mut watch::Receiver<bool>) {
    while !*receiver.borrow_and_update() {
        if receiver.changed().await.is_err() {
            break;
        }
    }
}
async fn release(client: &reqwest::Client, url: url::Url) -> Result<(), &'static str> {
    let response = client
        .delete(url)
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .map_err(|_| "larm_release_failed")?;
    if response.status().as_u16() != 204 {
        return Err("larm_release_failed");
    }
    Ok(())
}
impl Drop for Session {
    fn drop(&mut self) {
        if !self.released.load(Ordering::Acquire) {
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                let client = self.client.clone();
                let url = self.connection.clone();
                runtime.spawn(async move {
                    if release(&client, url).await.is_err() {
                        eprintln!("LARM release failed during cleanup");
                    }
                });
            }
        }
    }
}

#[cfg(test)]
mod tests;
