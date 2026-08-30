use std::sync::Arc;

use crate::{ModelProviderSettings, ModelProvidersSettings, RunCancellation};

pub(crate) async fn resolve_harness_llm_provider(
    providers: &mut ModelProvidersSettings,
    timeout_ms: u64,
    cancellation: Arc<RunCancellation>,
) -> Result<(), String> {
    let resolved = match crate::providers::service_harness::resolve_service_cancellable(
        &providers.harness.address,
        "llm",
        &cancellation,
    )
    .await
    {
        Ok(service) => {
            ModelProviderSettings::OpenAiCompatible(crate::OpenAiCompatibleProviderSettings {
                id: crate::DYNAMIC_LAN_PROVIDER_ID.to_string(),
                enabled: true,
                label: "Provider Harness LLM".to_string(),
                location: "local".to_string(),
                endpoint: service.base_url,
                model: service.model,
                authentication: "none".to_string(),
            })
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
            validate_legacy_dynamic_lan_timeout(timeout_ms)?;
            ModelProviderSettings::DynamicLan(crate::DynamicLanProviderSettings {
                id: crate::DYNAMIC_LAN_PROVIDER_ID.to_string(),
                enabled: true,
                label: "Legacy Dynamic LAN LLM".to_string(),
                location: "local".to_string(),
                host,
            })
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
    Ok(())
}

fn validate_legacy_dynamic_lan_timeout(timeout_ms: u64) -> Result<(), String> {
    if timeout_ms > crate::providers::dynamic_lan::MAX_REQUEST_TIMEOUT_MS {
        return Err(format!(
            "Legacy Dynamic LAN supports an LLM timeout of at most {} ms; update the Harness or lower the timeout in Settings",
            crate::providers::dynamic_lan::MAX_REQUEST_TIMEOUT_MS
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::dynamic_lan_provider;

    #[test]
    fn legacy_dynamic_lan_rejects_a_timeout_longer_than_its_connection_lifetime() {
        assert!(validate_legacy_dynamic_lan_timeout(
            crate::providers::dynamic_lan::MAX_REQUEST_TIMEOUT_MS
        )
        .is_ok());
        assert!(validate_legacy_dynamic_lan_timeout(1_800_000).is_err());
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
