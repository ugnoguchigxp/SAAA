use std::sync::Arc;

use crate::{ModelProviderSettings, ModelProvidersSettings, RunCancellation};

pub(crate) async fn resolve_harness_llm_provider(
    providers: &mut ModelProvidersSettings,
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
            let Some(host) = crate::providers::service_harness::legacy_dynamic_lan_host(
                &providers.harness.address,
            )?
            else {
                return Err(error);
            };
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
