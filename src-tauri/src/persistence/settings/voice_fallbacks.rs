use crate::{
    ModelProviderSettings, ModelProvidersSettings, RoutingSettings, SecurityRuntimeSettings,
};
pub(super) fn validate(
    providers: &ModelProvidersSettings,
    routing: &RoutingSettings,
    security: &SecurityRuntimeSettings,
) -> Result<(), String> {
    let enabled_provider = |id: &str| {
        providers
            .providers
            .iter()
            .find(|p| p.id() == id && p.enabled())
    };
    for (route, capability) in [
        (&routing.voice_transcribe, "ASR"),
        (&routing.voice_speak, "TTS"),
    ] {
        let primary_local = route.source == "harness"
            || route
                .provider_id
                .as_deref()
                .and_then(enabled_provider)
                .is_some_and(|p| p.location() == "local");
        let mut seen = std::collections::HashSet::new();
        if let Some(id) = &route.provider_id {
            seen.insert(id);
        }
        for id in &route.fallback_provider_ids {
            let provider =
                enabled_provider(id).ok_or("Voice fallback provider is missing or disabled")?;
            let valid = match capability {
                "ASR" => matches!(provider, ModelProviderSettings::CloudAsr(_)),
                _ => matches!(
                    provider,
                    ModelProviderSettings::CloudTts(_) | ModelProviderSettings::SystemTts(_)
                ),
            };
            if !valid || !seen.insert(id) {
                return Err("Invalid or duplicate voice fallback".into());
            }
            if security.local_only_when_selected && primary_local && provider.location() == "cloud"
            {
                return Err(
                    "Cloud fallback is blocked while the local-only policy is active".into(),
                );
            }
        }
    }
    Ok(())
}
