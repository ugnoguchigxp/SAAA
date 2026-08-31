use std::sync::Arc;

use crate::{ModelProviderSettings, ModelProvidersSettings, RunCancellation};

pub(crate) async fn resolve_harness_llm_provider(
    providers: &mut ModelProvidersSettings,
    timeout_ms: u64,
    cancellation: Arc<RunCancellation>,
) -> Result<u64, String> {
    let (resolved, effective_timeout_ms) =
        match crate::providers::service_harness::resolve_service_cancellable(
            &providers.harness.address,
            "llm",
            &cancellation,
        )
        .await
        {
            Ok(service) => {
                let stream_url = match service.streaming {
                    Some(crate::providers::service_harness::StreamingDescriptor::Llm(
                        streaming,
                    )) => streaming.url,
                    _ => {
                        return Err(
                            "Provider Harness does not advertise saaa.llm-stream.v1".to_string()
                        )
                    }
                };
                (
                    ModelProviderSettings::OpenAiCompatible(
                        crate::OpenAiCompatibleProviderSettings {
                            id: crate::DYNAMIC_LAN_PROVIDER_ID.to_string(),
                            enabled: true,
                            label: "Provider Harness LLM".to_string(),
                            location: "local".to_string(),
                            endpoint: stream_url,
                            model: service.model,
                            authentication: "none".to_string(),
                        },
                    ),
                    timeout_ms,
                )
            }
            Err(error) => {
                if cancellation.is_cancelled() {
                    return Err("Cancelled by user".to_string());
                }
                let Some(host) = crate::providers::service_harness::legacy_dynamic_lan_host(
                    &providers.harness.address,
                )?
                else {
                    return Err(error);
                };
                (
                    ModelProviderSettings::DynamicLan(crate::DynamicLanProviderSettings {
                        id: crate::DYNAMIC_LAN_PROVIDER_ID.to_string(),
                        enabled: true,
                        label: "Legacy Dynamic LAN LLM".to_string(),
                        location: "local".to_string(),
                        host,
                    }),
                    effective_legacy_dynamic_lan_timeout(timeout_ms),
                )
            }
        };
    if let Some(provider) = providers
        .providers
        .iter_mut()
        .find(|provider| provider.id() == crate::DYNAMIC_LAN_PROVIDER_ID)
    {
        *provider = resolved;
    } else {
        providers.providers.push(resolved);
    }
    Ok(effective_timeout_ms)
}

fn effective_legacy_dynamic_lan_timeout(timeout_ms: u64) -> u64 {
    // Harness settings may allow a longer modern-provider timeout before the
    // runtime discovers that it must use the shorter-lived legacy connection.
    timeout_ms.min(crate::providers::dynamic_lan::MAX_REQUEST_TIMEOUT_MS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::dynamic_lan_provider;

    #[test]
    fn legacy_dynamic_lan_caps_timeout_at_its_connection_lifetime() {
        assert_eq!(effective_legacy_dynamic_lan_timeout(120_000), 120_000);
        assert_eq!(
            effective_legacy_dynamic_lan_timeout(
                crate::providers::dynamic_lan::MAX_REQUEST_TIMEOUT_MS
            ),
            crate::providers::dynamic_lan::MAX_REQUEST_TIMEOUT_MS
        );
        assert_eq!(
            effective_legacy_dynamic_lan_timeout(1_800_000),
            crate::providers::dynamic_lan::MAX_REQUEST_TIMEOUT_MS
        );
    }

    #[tokio::test]
    async fn cancellation_never_falls_through_to_the_legacy_route() {
        let mut providers = ModelProvidersSettings {
            harness: crate::HarnessSettings {
                address: "http://localhost:9810".to_string(),
            },
            providers: vec![dynamic_lan_provider(crate::DYNAMIC_LAN_PROVIDER_ID)],
            reasoning_effort: crate::providers::default_conversation_reasoning_effort(),
        };
        let cancellation = Arc::new(RunCancellation::default());
        cancellation.cancel();

        let error = resolve_harness_llm_provider(&mut providers, 1_800_000, cancellation)
            .await
            .expect_err("cancelled resolution must stop");

        assert_eq!(error, "Cancelled by user");
    }
}
