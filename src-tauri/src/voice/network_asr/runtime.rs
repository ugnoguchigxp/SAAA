use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use reqwest::Client;
use tokio::sync::Mutex;

use super::{base_url_from_host, client, discovery::resolve_at_with_client};
use crate::{NetworkAsrResolution, RunCancellation};

const RESOLUTION_TTL: Duration = Duration::from_secs(300);

pub(crate) struct NetworkAsrRuntime {
    client: Client,
    cached: Mutex<Option<CachedResolution>>,
}

#[derive(Clone)]
struct CachedResolution {
    host: String,
    resolution: NetworkAsrResolution,
    expires_at: Instant,
}

impl NetworkAsrRuntime {
    pub(crate) fn new() -> Result<Self, String> {
        Ok(Self {
            client: client()?,
            cached: Mutex::new(None),
        })
    }

    pub(crate) async fn resolve(
        &self,
        host: &str,
        cancellation: Arc<RunCancellation>,
    ) -> Result<NetworkAsrResolution, String> {
        self.resolve_with_policy(host, cancellation, false).await
    }

    pub(crate) async fn refresh(
        &self,
        host: &str,
        cancellation: Arc<RunCancellation>,
    ) -> Result<NetworkAsrResolution, String> {
        self.resolve_with_policy(host, cancellation, true).await
    }

    async fn resolve_with_policy(
        &self,
        host: &str,
        cancellation: Arc<RunCancellation>,
        force_refresh: bool,
    ) -> Result<NetworkAsrResolution, String> {
        let normalized_host = host.trim().to_ascii_lowercase();
        let mut cached = tokio::select! {
            _ = cancellation.cancelled() => return Err("Transcription cancelled".to_string()),
            cached = self.cached.lock() => cached,
        };
        if let Some(resolution) = reusable_resolution(
            cached.as_ref(),
            &normalized_host,
            Instant::now(),
            force_refresh,
        ) {
            return Ok(resolution);
        }
        let base_url = base_url_from_host(host)?;
        let resolution = resolve_at_with_client(&self.client, &base_url, &cancellation).await?;
        *cached = Some(CachedResolution {
            host: normalized_host,
            resolution: resolution.clone(),
            expires_at: Instant::now() + RESOLUTION_TTL,
        });
        Ok(resolution)
    }

    pub(crate) async fn invalidate(&self, host: &str) {
        let normalized_host = host.trim().to_ascii_lowercase();
        let mut cached = self.cached.lock().await;
        if cached
            .as_ref()
            .is_some_and(|value| value.host == normalized_host)
        {
            *cached = None;
        }
    }

    pub(super) fn client(&self) -> &Client {
        &self.client
    }
}

fn reusable_resolution(
    cached: Option<&CachedResolution>,
    host: &str,
    now: Instant,
    force_refresh: bool,
) -> Option<NetworkAsrResolution> {
    if force_refresh {
        return None;
    }
    cached
        .filter(|cached| cached.host == host && cached.expires_at > now)
        .map(|cached| cached.resolution.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::voice::network_asr::{MODEL_ID, PROVIDER_ID};

    fn cached(now: Instant) -> CachedResolution {
        CachedResolution {
            host: "asr.local".to_string(),
            resolution: NetworkAsrResolution {
                provider_id: PROVIDER_ID.to_string(),
                endpoint: "http://asr.local:8081".to_string(),
                model: MODEL_ID.to_string(),
            },
            expires_at: now + Duration::from_secs(1),
        }
    }

    #[tokio::test]
    async fn reuses_a_successful_resolution_without_another_network_probe() {
        let runtime = NetworkAsrRuntime::new().expect("runtime initializes");
        *runtime.cached.lock().await = Some(cached(Instant::now()));
        let resolution = runtime
            .resolve("ASR.LOCAL", Arc::new(RunCancellation::default()))
            .await
            .expect("cached resolution returns");
        assert_eq!(resolution.endpoint, "http://asr.local:8081");
    }

    #[test]
    fn cache_expires_and_explicit_refresh_bypasses_it() {
        let now = Instant::now();
        let cached = cached(now);
        assert!(reusable_resolution(Some(&cached), "asr.local", now, false).is_some());
        assert!(reusable_resolution(
            Some(&cached),
            "asr.local",
            now + Duration::from_secs(2),
            false
        )
        .is_none());
        assert!(reusable_resolution(Some(&cached), "asr.local", now, true).is_none());
    }

    #[tokio::test]
    async fn invalidation_removes_only_the_matching_host() {
        let runtime = NetworkAsrRuntime::new().expect("runtime initializes");
        *runtime.cached.lock().await = Some(cached(Instant::now()));
        runtime.invalidate("other.local").await;
        assert!(runtime.cached.lock().await.is_some());
        runtime.invalidate("ASR.LOCAL").await;
        assert!(runtime.cached.lock().await.is_none());
    }
}
