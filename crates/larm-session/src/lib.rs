//! One lease for the entire voice conversation. Read guards pin all claimed tokens;
//! renew/reclaim and release take the writer lock and cannot overtake inference.
pub mod catalog;
mod contract;
mod error;
mod http;
pub mod http_api;
use catalog::CatalogProfile;
use contract::Snapshot;
pub use contract::{
    local_url, required_providers, Capacity, ContextWindow, EmbeddingSpace, Provider, BACKCHANNEL,
    BASE_PROVIDERS, DEFAULT_SELECTOR, LEGACY_PROFILE_IDS,
};
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfilePreference {
    Variant(ProfileVariant),
    Explicit(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileVariant {
    Conversation,
    Image,
    Music,
}

impl ProfileVariant {
    pub const ALL: [Self; 3] = [Self::Conversation, Self::Image, Self::Music];
    pub fn selector(self) -> &'static str {
        match self {
            Self::Conversation => DEFAULT_SELECTOR,
            Self::Image => "SAAA-w-Image",
            Self::Music => "SAAA-w-music",
        }
    }
    pub fn from_selector(selector: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|variant| variant.selector() == selector)
    }
    fn services(self) -> &'static [&'static str] {
        match self {
            Self::Conversation => &[],
            Self::Image => &["image"],
            Self::Music => &["music"],
        }
    }
}

/// Credential-free view of a claimed provider for diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderSummary {
    pub name: String,
    pub model: String,
    pub endpoint: String,
    pub context_window: Option<ContextWindow>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionPhase {
    ModelPreparing,
    CapacityWaiting,
    SemanticProbing,
    Ready,
    IdleReleased,
    Reconnecting,
    TerminalFailure,
}
impl ConnectionPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ModelPreparing => "model_preparing",
            Self::CapacityWaiting => "capacity_waiting",
            Self::SemanticProbing => "semantic_probing",
            Self::Ready => "ready",
            Self::IdleReleased => "idle_released",
            Self::Reconnecting => "reconnecting",
            Self::TerminalFailure => "terminal_failure",
        }
    }
}

pub struct Session {
    client: reqwest::Client,
    control_token: zeroize::Zeroizing<String>,
    connection: url::Url,
    id: String,
    profile: String,
    catalog: Option<CatalogProfile>,
    required: Vec<&'static str>,
    created_models: std::collections::HashMap<String, String>,
    snapshot: Arc<RwLock<Option<Snapshot>>>,
    closed: AtomicBool,
    released: AtomicBool,
    terminal_reason: std::sync::Mutex<Option<String>>,
    phase: Option<watch::Sender<ConnectionPhase>>,
    stop: watch::Sender<bool>,
}
pub struct Use {
    snapshot: OwnedRwLockReadGuard<Option<Snapshot>>,
    name: String,
    _session: Arc<Session>,
    _capacity_permit: tokio::sync::OwnedSemaphorePermit,
}
impl Use {
    pub fn context_subject(&self) -> Result<&str, &'static str> {
        self.snapshot
            .as_ref()
            .expect("live snapshot")
            .context_subject
            .as_deref()
            .ok_or("larm_context_unavailable")
    }
    pub fn provider(&self) -> &Provider {
        &self.snapshot.as_ref().expect("live snapshot").providers[&self.name]
    }
    pub fn capacity(&self) -> Result<Capacity, &'static str> {
        self.provider()
            .capacity
            .get()
            .copied()
            .ok_or("larm_capacity_unavailable")
    }
    /// Never start provider I/O beyond the pinned credential's expiry.
    pub fn request_budget(&self, requested: Duration) -> Result<Duration, &'static str> {
        let remaining = (self.snapshot.as_ref().expect("live snapshot").expires_at
            - chrono::Utc::now())
        .to_std()
        .map_err(|_| "larm_expired")?
        .saturating_sub(Duration::from_secs(5));
        if remaining.is_zero() {
            return Err("larm_expired");
        }
        Ok(requested.min(remaining))
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
        #[cfg(not(test))]
        let token = std::env::var("LARM_API_TOKEN")
            .map_err(|_| ConnectError::from("credential_missing"))?;
        #[cfg(test)]
        let token = "test-control-token".to_string();
        Self::connect_with_profile_credential_and_key(
            base,
            ProfilePreference::Variant(ProfileVariant::Conversation),
            token,
            format!("saaa-session-{}", uuid::Uuid::new_v4()),
            cancellation,
        )
        .await
    }
    pub async fn connect_with_profile(
        base: &str,
        profile: &str,
        cancellation: watch::Receiver<bool>,
    ) -> Result<Arc<Self>, ConnectError> {
        #[cfg(not(test))]
        let token = std::env::var("LARM_API_TOKEN")
            .map_err(|_| ConnectError::from("credential_missing"))?;
        #[cfg(test)]
        let token = "test-control-token".to_string();
        Self::connect_with_profile_and_credential(base, profile, token, cancellation).await
    }
    pub async fn connect_with_profile_and_credential(
        base: &str,
        profile: &str,
        token: String,
        cancellation: watch::Receiver<bool>,
    ) -> Result<Arc<Self>, ConnectError> {
        Self::connect_with_profile_credential_and_key(
            base,
            if let Some(variant) = ProfileVariant::from_selector(profile) {
                ProfilePreference::Variant(variant)
            } else if LEGACY_PROFILE_IDS.contains(&profile) {
                ProfilePreference::Variant(ProfileVariant::Conversation)
            } else {
                ProfilePreference::Explicit(profile.to_string())
            },
            token,
            format!("saaa-session-{}", uuid::Uuid::new_v4()),
            cancellation,
        )
        .await
    }
    pub async fn connect_with_profile_credential_and_key(
        base: &str,
        preference: ProfilePreference,
        token: String,
        idempotency_key: String,
        cancellation: watch::Receiver<bool>,
    ) -> Result<Arc<Self>, ConnectError> {
        Self::connect_with_profile_credential_key_and_phase(
            base,
            preference,
            token,
            idempotency_key,
            cancellation,
            None,
        )
        .await
    }
    pub async fn connect_with_profile_credential_key_and_phase(
        base: &str,
        preference: ProfilePreference,
        token: String,
        idempotency_key: String,
        cancellation: watch::Receiver<bool>,
        phase: Option<watch::Sender<ConnectionPhase>>,
    ) -> Result<Arc<Self>, ConnectError> {
        if token.is_empty()
            || token.trim().is_empty()
            || token.len() > 4096
            || token.bytes().any(|byte| matches!(byte, 0 | b'\r' | b'\n'))
        {
            return Err("credential_invalid".into());
        }
        if let ProfilePreference::Explicit(profile) = &preference {
            validate_profile(profile)?;
            if ProfileVariant::from_selector(profile).is_none() {
                return Err("larm_unknown_selector".into());
            }
        }
        if idempotency_key.is_empty()
            || idempotency_key.len() > 160
            || !idempotency_key
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        {
            return Err("larm_invalid_idempotency_key".into());
        }
        let base = base.to_string();
        let (alive, abandoned) = watch::channel(false);
        let (send, receive) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let result = Self::connect_inner(
                &base,
                preference,
                token,
                idempotency_key,
                cancellation,
                abandoned,
                phase,
            )
            .await;
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
        preference: ProfilePreference,
        token: String,
        idempotency_key: String,
        mut cancellation: watch::Receiver<bool>,
        mut abandoned: watch::Receiver<bool>,
        phase: Option<watch::Sender<ConnectionPhase>>,
    ) -> Result<Arc<Self>, ConnectError> {
        if let Some(phase) = &phase {
            phase.send_replace(ConnectionPhase::ModelPreparing);
        }
        let started = Instant::now();
        if *cancellation.borrow() {
            return Err("larm_cancelled".into());
        }
        let mut base = url::Url::parse(base).map_err(|_| "larm_invalid_control_url")?;
        if !local_url(&base) || base.path() != "/" {
            return Err("larm_invalid_control_url".into());
        }
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|_| "larm_client_failed")?;
        let required = contract::required_providers();
        let (profile, catalog) = match &preference {
            ProfilePreference::Explicit(profile) => (profile.clone(), None),
            ProfilePreference::Variant(variant) => {
                let profile = tokio::select! { biased;
                    _ = cancelled(&mut cancellation) => return Err("larm_cancelled".into()),
                    _ = cancelled(&mut abandoned) => return Err("larm_cancelled".into()),
                    result = catalog::fetch(&client, &base, &token, variant.selector()) => result?,
                };
                let expected = variant.services();
                if !required.iter().all(|name| profile.provider(name).is_some())
                    || profile.services.len() != expected.len()
                    || !expected
                        .iter()
                        .all(|name| profile.services.iter().any(|service| service.name == *name))
                {
                    return Err("larm_profile_unavailable".into());
                }
                (variant.selector().to_string(), Some(profile))
            }
        };
        validate_profile(&profile)?;
        base.set_path("/v1/agent-connections");
        // Do not race creation against cancellation: receive the id, then release it.
        let mut body = json!({"profile":profile,
            "audience":"saaa-desktop","client":"saaa-desktop","ttlSeconds":900,
            "allowFallback":false,"deploymentPolicy":"existing-only"});
        if let Some(catalog) = &catalog {
            body["expectedCatalogRevision"] = json!(catalog.revision);
        }
        let create = client
            .post(base.clone())
            .timeout(Duration::from_secs(10))
            .header("Idempotency-Key", &idempotency_key)
            .header("Prefer", "wait=0")
            .json(&body);
        let create = authorize(create, &token)?;
        let retry_create = create.try_clone().ok_or("larm_client_failed")?;
        let (create_status, mut created, location, initial_retry_after) =
            match http::json_response(create, &[201, 202]).await {
                Err("larm_transport_failed") => {
                    http::json_response(retry_create, &[201, 202]).await?
                }
                result => result?,
            };
        let location_id = location.as_deref().and_then(|location| {
            let resource = base.join(location).ok()?;
            if resource.origin() != base.origin()
                || resource.query().is_some()
                || resource.fragment().is_some()
            {
                return None;
            }
            let prefix = "/v1/agent-connections/";
            resource
                .path()
                .strip_prefix(prefix)
                .filter(|segment| !segment.contains('/'))
                .map(str::to_string)
        });
        let id = created["id"]
            .as_str()
            .map(str::to_string)
            .or(location_id)
            .ok_or("larm_invalid_connection_id")?;
        created["id"] = json!(&id);
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
        let create_contract_invalid = (create_status == 201 && created["status"] != "ready")
            || (create_status == 202
                && !matches!(
                    created["status"].as_str(),
                    Some("pending" | "deploying" | "probing")
                ))
            || (create_status == 202
                && location.as_deref().is_some_and(|location| {
                    let mut root = base.clone();
                    root.set_path("/");
                    root.join(location).ok().as_ref() != Some(&base)
                }));
        let (stop, _) = watch::channel(false);
        let created_models = created["providers"]
            .as_array()
            .map(|providers| {
                providers
                    .iter()
                    .filter_map(|provider| {
                        Some((
                            provider["name"].as_str()?.to_string(),
                            provider["model"].as_str()?.to_string(),
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let session = Arc::new(Self {
            client,
            control_token: zeroize::Zeroizing::new(token),
            connection: base,
            id: id.clone(),
            profile: created["agentProfile"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            catalog,
            required: required.clone(),
            created_models,
            snapshot: Arc::new(RwLock::new(None)),
            closed: AtomicBool::new(false),
            released: AtomicBool::new(false),
            terminal_reason: std::sync::Mutex::new(None),
            phase,
            stop,
        });
        let startup = async {
            let mut state = created;
            session.report_phase(&state);
            let mut retry_after = initial_retry_after.unwrap_or(Duration::from_secs(1));
            if create_contract_invalid {
                return Err("larm_invalid_contract");
            }
            contract::validate_created(&state, &profile, &required, session.catalog.as_ref())?;
            loop {
                match state["status"].as_str() {
                    Some("ready") => {
                        contract::validate_created(
                            &state,
                            &profile,
                            &required,
                            session.catalog.as_ref(),
                        )?;
                        break;
                    }
                    Some("pending" | "deploying" | "probing") => {}
                    _ => {
                        *session
                            .terminal_reason
                            .lock()
                            .expect("terminal reason lock") = safe_terminal_reason(&state);
                        return Err("larm_startup_terminal");
                    }
                }
                tokio::time::sleep(retry_after).await;
                let (_, next, _, next_retry_after) = http::json_response(
                    session.authorize(session.client.get(session.connection.clone()))?,
                    &[200],
                )
                .await?;
                state = next;
                session.report_phase(&state);
                retry_after = next_retry_after.unwrap_or(Duration::from_secs(1));
                if state["id"] != id {
                    return Err("larm_connection_mismatch");
                }
                if matches!(
                    state["status"].as_str(),
                    Some("failed" | "released" | "expired")
                ) {
                    *session
                        .terminal_reason
                        .lock()
                        .expect("terminal reason lock") = safe_terminal_reason(&state);
                    return Err("larm_startup_terminal");
                }
                contract::validate_created(&state, &profile, &required, session.catalog.as_ref())?;
            }
            let snapshot = session.claim().await?;
            *session.snapshot.write().await = Some(snapshot);
            if let Some(phase) = &session.phase {
                phase.send_replace(ConnectionPhase::Ready);
            }
            Ok(())
        };
        let result = tokio::select! { biased;
            _ = cancelled(&mut cancellation) => Err("larm_cancelled"),
            _ = cancelled(&mut abandoned) => Err("larm_cancelled"),
            result = tokio::time::timeout(Duration::from_secs(300).saturating_sub(started.elapsed()), startup) => result.unwrap_or(Err("larm_startup_timeout")),
        };
        if let Err(error) = result {
            if let Some(phase) = &session.phase {
                phase.send_replace(ConnectionPhase::TerminalFailure);
            }
            let reason = session
                .terminal_reason
                .lock()
                .expect("terminal reason lock")
                .clone();
            return match session.close().await {
                Ok(()) => Err(ConnectError {
                    code: error,
                    reason,
                    cleanup: None,
                }),
                Err(_) => Err(ConnectError {
                    code: error,
                    reason,
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
    fn report_phase(&self, state: &serde_json::Value) {
        let Some(phase) = &self.phase else { return };
        let next = match state["status"].as_str() {
            Some("ready") => ConnectionPhase::SemanticProbing,
            Some("probing") => ConnectionPhase::SemanticProbing,
            Some("released" | "expired") => ConnectionPhase::IdleReleased,
            Some("failed") => ConnectionPhase::TerminalFailure,
            _ if state["reason"]
                .as_str()
                .or_else(|| state["pendingReason"].as_str())
                .or_else(|| state["error"]["code"].as_str())
                .is_some_and(|value| value.contains("capacity")) =>
            {
                ConnectionPhase::CapacityWaiting
            }
            _ => ConnectionPhase::ModelPreparing,
        };
        phase.send_replace(next);
    }
    fn authorize(
        &self,
        call: reqwest::RequestBuilder,
    ) -> Result<reqwest::RequestBuilder, &'static str> {
        authorize(call, self.control_token.as_str())
    }
    async fn claim(&self) -> Result<Snapshot, &'static str> {
        let value = http::json(
            self.authorize(
                self.client
                    .post(self.operation("claim"))
                    .json(&json!({"format":"openai-provider-v1"})),
            )?,
            &[200],
        )
        .await?;
        let snapshot = contract::parse(value, &self.id, &self.required)?;
        if snapshot
            .providers
            .iter()
            .any(|(name, provider)| self.created_models.get(name) != Some(&provider.model))
        {
            return Err("larm_create_claim_mismatch");
        }
        if let Some(catalog) = &self.catalog {
            contract::verify_against_catalog(&snapshot, catalog)?;
        }
        for provider in snapshot.providers.values() {
            self.health(provider).await?;
        }
        Ok(snapshot)
    }
    pub fn profile_id(&self) -> &str {
        &self.profile
    }
    pub fn selector(&self) -> Option<&str> {
        self.catalog
            .as_ref()
            .map(|catalog| catalog.selector.as_str())
    }
    pub fn catalog_revision(&self) -> Option<&str> {
        self.catalog
            .as_ref()
            .map(|catalog| catalog.revision.as_str())
    }
    pub async fn provider_summary(&self) -> Vec<ProviderSummary> {
        let endpoints = self
            .catalog
            .as_ref()
            .map(|catalog| {
                catalog
                    .providers
                    .iter()
                    .map(|provider| (provider.name.clone(), provider.endpoint.clone()))
                    .collect::<std::collections::HashMap<_, _>>()
            })
            .unwrap_or_default();
        let mut summary = self
            .snapshot
            .read()
            .await
            .as_ref()
            .map(|snapshot| {
                snapshot
                    .providers
                    .iter()
                    .map(|(name, provider)| ProviderSummary {
                        name: name.clone(),
                        model: provider.model.clone(),
                        endpoint: endpoints.get(name).cloned().unwrap_or_default(),
                        context_window: provider.context_window,
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        summary.sort_by(|left, right| left.name.cmp(&right.name));
        summary
    }
    pub async fn provider_names(&self) -> Vec<String> {
        self.snapshot
            .read()
            .await
            .as_ref()
            .map(|snapshot| snapshot.providers.keys().cloned().collect())
            .unwrap_or_default()
    }
    pub async fn has_provider(&self, name: &str) -> bool {
        self.snapshot
            .read()
            .await
            .as_ref()
            .is_some_and(|snapshot| snapshot.providers.contains_key(name))
    }

    /// Check the control plane before a new foreground operation. A released
    /// connection must lose all locally cached provider credentials at once.
    pub async fn check_status(&self) -> Result<(), &'static str> {
        if self.closed.load(Ordering::Acquire) {
            return Err("larm_session_closed");
        }
        let state = http::json(
            self.authorize(self.client.get(self.connection.clone()))?,
            &[200],
        )
        .await;
        let state = match state {
            Ok(state) => state,
            Err("larm_connection_idle_released") => {
                self.invalidate_idle_release().await;
                return Err("larm_connection_idle_released");
            }
            Err(error) => return Err(error),
        };
        if state["id"] != self.id {
            return Err("larm_connection_mismatch");
        }
        if state["status"] == "ready" {
            let selector = state["profile"].as_str().ok_or("larm_invalid_contract")?;
            if let Err(error) =
                contract::validate_created(&state, selector, &self.required, self.catalog.as_ref())
            {
                self.closed.store(true, Ordering::Release);
                *self.snapshot.write().await = None;
                if let Some(phase) = &self.phase {
                    phase.send_replace(ConnectionPhase::ModelPreparing);
                }
                return Err(error);
            }
            return Ok(());
        }
        let reason = state["reason"]
            .as_str()
            .or_else(|| state["error"]["code"].as_str());
        let code = if reason == Some("foreground_idle_timeout") {
            "larm_connection_idle_released"
        } else {
            match state["status"].as_str() {
                Some("released") => "larm_connection_idle_released",
                Some("expired") => "larm_expired",
                Some("failed") => "larm_startup_terminal",
                _ => "larm_session_unavailable",
            }
        };
        if matches!(
            state["status"].as_str(),
            Some("released" | "expired" | "failed")
        ) || reason == Some("foreground_idle_timeout")
        {
            if let Some(phase) = &self.phase {
                phase.send_replace(ConnectionPhase::IdleReleased);
            }
            self.closed.store(true, Ordering::Release);
            *self.snapshot.write().await = None;
        }
        Err(code)
    }

    pub async fn invalidate_idle_release(&self) {
        if let Some(phase) = &self.phase {
            phase.send_replace(ConnectionPhase::IdleReleased);
        }
        self.closed.store(true, Ordering::Release);
        *self.snapshot.write().await = None;
    }

    pub async fn embed_query(
        self: &Arc<Self>,
        texts: &[String],
    ) -> Result<Vec<Vec<f32>>, &'static str> {
        if texts.is_empty()
            || texts.len() > 64
            || texts
                .iter()
                .any(|text| text.is_empty() || text.len() > 32_768)
        {
            return Err("larm_invalid_embedding_input");
        }
        let lease = self.acquire("embedding").await?;
        let provider = lease.provider();
        let dimension = provider
            .embedding_space
            .ok_or("larm_missing_embedding_space")?
            .dimension;
        let timeout = lease.request_budget(Duration::from_secs(30))?;
        let value = http::json(
            self.client
                .post(provider.endpoint("embed")?)
                .bearer_auth(provider.token())
                .timeout(timeout)
                .json(&json!({
                    "texts": texts,
                    "type": "query",
                    "normalize": true,
                    "priority": "normal"
                })),
            &[200],
        )
        .await?;
        let raw = value["embeddings"]
            .as_array()
            .filter(|vectors| vectors.len() == texts.len())
            .ok_or("larm_invalid_embedding_response")?;
        raw.iter()
            .map(|vector| {
                vector
                    .as_array()
                    .filter(|values| values.len() == dimension)
                    .ok_or("larm_invalid_embedding_response")?
                    .iter()
                    .map(|value| {
                        value
                            .as_f64()
                            .filter(|value| value.is_finite())
                            .map(|value| value as f32)
                            .ok_or("larm_invalid_embedding_response")
                    })
                    .collect()
            })
            .collect()
    }
    async fn health(
        &self,
        provider: &Provider,
    ) -> Result<Arc<tokio::sync::Semaphore>, &'static str> {
        let mut checked = provider.checked_at.lock().await;
        if checked.is_some_and(|at| at.elapsed() < provider.max_age) {
            return provider
                .limiter
                .get()
                .cloned()
                .ok_or("larm_invalid_capacity");
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
        let raw = &value["capacity"];
        let capacity = Capacity {
            max_concurrent_requests: raw["maxConcurrentRequests"]
                .as_u64()
                .and_then(|value| usize::try_from(value).ok())
                .filter(|value| (1..=1024).contains(value))
                .ok_or("larm_invalid_capacity")?,
            active_requests: raw["activeRequests"]
                .as_u64()
                .and_then(|value| usize::try_from(value).ok())
                .ok_or("larm_invalid_capacity")?,
            max_queued_requests: raw["maxQueuedRequests"]
                .as_u64()
                .and_then(|value| usize::try_from(value).ok())
                .ok_or("larm_invalid_capacity")?,
            queue_depth: raw["queueDepth"]
                .as_u64()
                .and_then(|value| usize::try_from(value).ok())
                .ok_or("larm_invalid_capacity")?,
            queue_timeout_ms: raw["queueTimeoutMs"]
                .as_u64()
                .ok_or("larm_invalid_capacity")?,
            retry_after_ms: raw["retryAfterMs"]
                .as_u64()
                .ok_or("larm_invalid_capacity")?,
            completion_guaranteed: raw["completionGuaranteed"]
                .as_bool()
                .ok_or("larm_invalid_capacity")?,
        };
        if capacity.active_requests > capacity.max_concurrent_requests
            || capacity.queue_depth > capacity.max_queued_requests
            || capacity.queue_timeout_ms > 600_000
            || capacity.retry_after_ms > 600_000
        {
            return Err("larm_invalid_capacity");
        }
        provider.capacity.get_or_init(|| async { capacity }).await;
        let limiter = provider
            .limiter
            .get_or_init(|| async {
                Arc::new(tokio::sync::Semaphore::new(
                    capacity.max_concurrent_requests,
                ))
            })
            .await
            .clone();
        if at.elapsed() >= provider.max_age {
            return Err("larm_stale_health");
        }
        *checked = Some(at);
        Ok(limiter)
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
        let limiter = self.health(provider).await?;
        let permit = limiter.acquire_owned().await.map_err(|_| "larm_capacity")?;
        Ok(Use {
            snapshot: guard,
            name: name.to_string(),
            _session: self.clone(),
            _capacity_permit: permit,
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
                self.authorize(
                    self.client
                        .post(self.operation("renew"))
                        .header(
                            "Idempotency-Key",
                            format!("saaa-renew-{}", uuid::Uuid::new_v4()),
                        )
                        .json(&json!({"ttlSeconds":600})),
                )?,
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
        if let Err(error) = release(
            &self.client,
            self.connection.clone(),
            self.control_token.as_str(),
        )
        .await
        {
            eprintln!("LARM release failed; connection remains closed and can be released again");
            return Err(error);
        }
        self.released.store(true, Ordering::Release);
        Ok(())
    }
}
fn safe_terminal_reason(state: &serde_json::Value) -> Option<String> {
    state["reason"]
        .as_str()
        .or_else(|| state["error"]["code"].as_str())
        .filter(|reason| {
            !reason.is_empty()
                && reason.len() <= 128
                && reason
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        })
        .map(str::to_string)
}
async fn cancelled(receiver: &mut watch::Receiver<bool>) {
    while !*receiver.borrow_and_update() {
        if receiver.changed().await.is_err() {
            break;
        }
    }
}
fn validate_profile(profile: &str) -> Result<(), &'static str> {
    if profile.is_empty()
        || profile.len() > 160
        || !profile
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
    {
        return Err("larm_invalid_profile");
    }
    Ok(())
}
pub(crate) fn authorize(
    call: reqwest::RequestBuilder,
    token: &str,
) -> Result<reqwest::RequestBuilder, &'static str> {
    let mut value = reqwest::header::HeaderValue::from_str(&format!("Bearer {token}"))
        .map_err(|_| "credential_invalid")?;
    value.set_sensitive(true);
    Ok(call.header(reqwest::header::AUTHORIZATION, value))
}
async fn release(client: &reqwest::Client, url: url::Url, token: &str) -> Result<(), &'static str> {
    let response = authorize(client.delete(url), token)?
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
                let token = self.control_token.clone();
                runtime.spawn(async move {
                    if release(&client, url, token.as_str()).await.is_err() {
                        eprintln!("LARM release failed during cleanup");
                    }
                });
            }
        }
    }
}

#[cfg(test)]
mod tests;

pub mod contexts;
pub mod personal_state;
