//! LARM media transport. Discovery and submission stay on the existing `larm-session` client.
use saaa_larm_session::media::{
    FailureKind, MediaArtifact, MediaClient, MediaError, MediaKind, MediaProgress, MediaResult,
};
use saaa_provider_routing::ResolvedRoute;
use std::sync::Arc;
use tokio::sync::watch;

fn revoked() -> MediaError {
    MediaError {
        kind: FailureKind::Cancelled,
        code: "route_revoked".into(),
        retryable: false,
        may_have_generated: false,
        job_id: None,
    }
}

pub struct LarmSession {
    client: Arc<MediaClient>,
    started: tokio::time::Instant,
    timeout_ms: u64,
}

impl LarmSession {
    pub async fn discover(
        route: &ResolvedRoute,
        kind: MediaKind,
        token: String,
        validate: Arc<dyn Fn() -> Result<(), String> + Send + Sync>,
    ) -> Result<Self, MediaError> {
        let started = tokio::time::Instant::now();
        let mut client = MediaClient::discover(&route.endpoint, token, kind).await?;
        let remaining = route
            .timeout_ms
            .saturating_sub(started.elapsed().as_millis() as u64);
        client.limits.submission = std::time::Duration::from_millis(remaining);
        client.limits.job = std::time::Duration::from_millis(remaining);
        let client = Arc::new(
            client.with_request_guard(Arc::new(move || validate().map_err(|_| revoked()))),
        );
        Ok(Self {
            client,
            started,
            timeout_ms: route.timeout_ms,
        })
    }

    pub async fn generate(
        &self,
        prompt: &str,
        cancel: watch::Receiver<bool>,
        progress: &(dyn Fn(MediaProgress) + Send + Sync),
    ) -> Result<MediaResult, MediaError> {
        self.client.generate(prompt, cancel, progress).await
    }

    pub async fn resume_music(
        &self,
        job_id: &str,
        cancel: watch::Receiver<bool>,
        progress: &(dyn Fn(MediaProgress) + Send + Sync),
    ) -> Result<MediaResult, MediaError> {
        self.client.resume_music_job(job_id, cancel, progress).await
    }

    pub async fn cancel_music(&self, job_id: &str) -> MediaError {
        self.client.cancel_music_job(job_id).await
    }

    pub async fn artifact(
        &self,
        artifact: &MediaArtifact,
        job_id: Option<String>,
    ) -> Result<Vec<u8>, MediaError> {
        let deadline = self.started + std::time::Duration::from_millis(self.timeout_ms);
        tokio::select! {
            _ = tokio::time::sleep_until(deadline) => Err(MediaError {
                kind: FailureKind::OutcomeUnknown,
                code: "artifact_deadline_exceeded".into(),
                retryable: false,
                may_have_generated: true,
                job_id,
            }),
            value = self.client.artifact_bytes(artifact) => value,
        }
    }
}

type CacheFn = dyn Fn(usize, &[u8]) -> Result<(), String> + Send + Sync;

pub async fn cache_artifacts(
    session: &LarmSession,
    result: &MediaResult,
    cache: &CacheFn,
) -> Result<MediaResult, MediaError> {
    let mut total = 0usize;
    for (index, artifact) in result.artifacts.iter().enumerate() {
        match session.artifact(artifact, result.job_id.clone()).await {
            Ok(bytes) => {
                total = total.saturating_add(bytes.len());
                if total > 64 * 1024 * 1024 || cache(index, &bytes).is_err() {
                    return Err(MediaError {
                        kind: FailureKind::ArtifactFailed,
                        code: "artifact_storage_failed".into(),
                        retryable: false,
                        may_have_generated: true,
                        job_id: result.job_id.clone(),
                    });
                }
            }
            Err(mut error) => {
                error.may_have_generated = true;
                error.job_id = result.job_id.clone();
                return Err(error);
            }
        }
    }
    Ok(result.clone())
}
