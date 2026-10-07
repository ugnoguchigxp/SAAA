//! Boundaries the host injects. Database work stays synchronous; network work returns a Send future.
use saaa_larm_session::media::{MediaArtifact, MediaError, MediaKind, MediaProgress, MediaResult};
use saaa_provider_routing::{LarmReachability, RegistrySnapshot, ResolvedRoute};
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::watch;
use zeroize::Zeroizing;

use crate::{
    contracts::{ArtifactBytes, BoxFut, ExplicitSecrets, HistoryQuery, MediaHostError},
    ledger::{CancelRecord, FinishOutcome},
};

pub trait MediaStore: Send + Sync {
    fn load_registry(&self) -> Result<RegistrySnapshot, MediaHostError>;
    fn validate_route(&self, route: &ResolvedRoute) -> Result<(), MediaHostError>;
    fn reserve(
        &self,
        now: &str,
        run: &str,
        kind: &MediaKind,
        route: &ResolvedRoute,
    ) -> Result<(), MediaHostError>;
    fn record_phase(
        &self,
        now: &str,
        run: &str,
        phase: &str,
        job: Option<&str>,
    ) -> Result<(), MediaHostError>;
    fn record_attempt(
        &self,
        now: &str,
        run: &str,
        route: &ResolvedRoute,
        attempt_id: &str,
        success: Option<bool>,
    ) -> Result<(), MediaHostError>;
    fn finish(
        &self,
        now: &str,
        run: &str,
        route: &ResolvedRoute,
        result: &Result<MediaResult, MediaError>,
    ) -> Result<FinishOutcome, MediaHostError>;
    fn finish_query(
        &self,
        now: &str,
        run: &str,
        route: &ResolvedRoute,
        result: &Result<MediaResult, MediaError>,
    ) -> Result<FinishOutcome, MediaHostError>;
    fn request_cancel(&self, now: &str, run: &str) -> Result<CancelRecord, MediaHostError>;
    fn mark_absent_cancelled(&self, now: &str, run: &str) -> Result<(), MediaHostError>;
    fn history(&self, query: &HistoryQuery) -> Result<Vec<Value>, MediaHostError>;
    fn get(&self, run: &str) -> Result<Option<Value>, MediaHostError>;
    fn cache(&self, run: &str, index: usize, bytes: &[u8]) -> Result<(), MediaHostError>;
    fn cached(&self, run: &str, index: usize) -> Result<Option<ArtifactBytes>, MediaHostError>;
    fn reconcile_interrupted(&self, now: &str) -> Result<(), MediaHostError>;
    fn release_unsent(&self, run: &str) -> Result<(), MediaHostError>;
}

pub trait CredentialSource: Send + Sync {
    fn named_secret(
        &self,
        service: &str,
        account: &str,
    ) -> Result<Option<Zeroizing<String>>, MediaHostError>;
    fn larm_token(&self) -> Result<Zeroizing<String>, MediaHostError>;
}

pub trait AvailabilitySource: Send + Sync {
    fn larm(&self) -> LarmReachability;
}

pub struct FixedAvailability(pub LarmReachability);

impl AvailabilitySource for FixedAvailability {
    fn larm(&self) -> LarmReachability {
        self.0
    }
}

impl CredentialSource for ExplicitSecrets {
    fn named_secret(
        &self,
        service: &str,
        account: &str,
    ) -> Result<Option<Zeroizing<String>>, MediaHostError> {
        Ok(self.named(service, account))
    }

    fn larm_token(&self) -> Result<Zeroizing<String>, MediaHostError> {
        ExplicitSecrets::larm_token(self)
    }
}

pub struct GenerateCall {
    pub run_id: String,
    pub kind: MediaKind,
    pub prompt: String,
    pub route: ResolvedRoute,
    pub cancel: watch::Receiver<bool>,
    pub progress: Arc<dyn Fn(MediaProgress) + Send + Sync>,
    pub resume: Option<String>,
}

pub trait MediaBackend: Send + Sync {
    fn generate(&self, call: GenerateCall) -> BoxFut<Result<MediaResult, MediaError>>;
    fn reconcile(&self, call: GenerateCall) -> BoxFut<Result<MediaResult, MediaError>>;
    fn cancel_remote(
        &self,
        route: ResolvedRoute,
        kind: MediaKind,
        job_id: String,
    ) -> BoxFut<MediaError>;
    fn fetch_artifact(
        &self,
        route: ResolvedRoute,
        kind: MediaKind,
        artifact: MediaArtifact,
    ) -> BoxFut<Result<Vec<u8>, MediaError>>;
}
