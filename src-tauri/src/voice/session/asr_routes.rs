use super::asr::{harness_asr_provider, AsrRoute, SelectedAsr};
use crate::{voice::network_asr::NetworkAsrRuntime, RunCancellation};
use std::{sync::Arc, time::Duration};

pub(crate) async fn transcribe_routes(
    network: &NetworkAsrRuntime,
    samples: &[f32],
    sample_rate: u32,
    selected: &SelectedAsr,
    cancellation: Arc<RunCancellation>,
) -> Result<(String, Option<String>), String> {
    let deadline = tokio::time::Instant::now() + Duration::from_millis(selected.timeout_ms);
    let mut last = "ASR provider is unavailable".to_string();
    for route in std::iter::once(&selected.route).chain(&selected.fallbacks) {
        if cancellation.is_cancelled() {
            return Err("Transcription cancelled".into());
        }
        let remaining = deadline
            .saturating_duration_since(tokio::time::Instant::now())
            .as_millis() as u64;
        if remaining == 0 {
            return Err("ASR request reached its configured timeout".into());
        }
        let budget = remaining.min(selected.attempt_timeout_ms);
        let result = tokio::select! { biased;
            _ = cancellation.cancelled() => return Err("Transcription cancelled".into()),
            result = tokio::time::timeout(Duration::from_millis(budget), transcribe_one(network, route, samples, sample_rate, budget, cancellation.clone())) => result.unwrap_or_else(|_| Err("ASR request reached its configured timeout".into())),
        };
        match result {
            Ok(result) => {
                crate::voice::language::enforce_allowed_language(
                    result.1.as_deref(),
                    &selected.allowed_languages,
                )?;
                return Ok(result);
            }
            Err(error) if crate::providers::route_policy::retryable(&error) => {
                last = error;
                crate::providers::http_metrics::record("asrFallback", Duration::ZERO);
            }
            Err(error) => return Err(error),
        }
    }
    Err(last)
}
async fn transcribe_one(
    network: &NetworkAsrRuntime,
    route: &AsrRoute,
    samples: &[f32],
    sample_rate: u32,
    budget: u64,
    cancellation: Arc<RunCancellation>,
) -> Result<(String, Option<String>), String> {
    match route {
        AsrRoute::Cloud(provider) => {
            crate::voice::cloud_asr::transcribe(
                provider,
                samples,
                sample_rate,
                budget,
                cancellation,
            )
            .await
        }
        AsrRoute::Larm(conversation, settings) => {
            let ready = crate::larm_voice::current_at(conversation, settings).await?;
            let lease = ready.session.acquire("asr").await.map_err(str::to_string)?;
            let budget = lease
                .request_budget(Duration::from_millis(budget))
                .map_err(str::to_string)?
                .as_millis() as u64;
            crate::voice::cloud_asr::transcribe_with_api_key(
                &crate::larm_voice::audio::asr_settings(lease.provider()),
                samples,
                sample_rate,
                budget,
                cancellation,
                Some(lease.provider().token()),
            )
            .await
        }
        AsrRoute::Harness(address) => {
            match crate::providers::service_harness::resolve_service_cancellable(
                address,
                "asr",
                &cancellation,
            )
            .await
            {
                Ok(service) => {
                    crate::voice::cloud_asr::transcribe(
                        &harness_asr_provider(service),
                        samples,
                        sample_rate,
                        budget,
                        cancellation,
                    )
                    .await
                }
                Err(error) => {
                    let Some(host) =
                        crate::providers::service_harness::legacy_dynamic_lan_host(address)?
                    else {
                        return Err(error);
                    };
                    let resolved = network.resolve(&host, cancellation.clone()).await?;
                    crate::voice::network_asr::transcribe_at(
                        network.client(),
                        &resolved.endpoint,
                        samples,
                        sample_rate,
                        &resolved.model,
                        cancellation,
                    )
                    .await
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "asr_route_tests.rs"]
mod tests;
