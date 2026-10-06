use crate::{ModelProviderSettings, ModelProvidersSettings, RoutingSettings};
pub(super) fn validate(
    providers: &ModelProvidersSettings,
    routing: &RoutingSettings,
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
            // A live qwen-realtime session is chosen once when the microphone starts, so it
            // cannot take over for LARM in the middle of a session.
            if route.source == "harness"
                && matches!(provider, ModelProviderSettings::CloudAsr(asr) if asr.transport == "qwen-realtime")
            {
                return Err("LARMの代替ASRにはライブ入力(qwen-realtime)を指定できません".into());
            }
        }
    }
    Ok(())
}
