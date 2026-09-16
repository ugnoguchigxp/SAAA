use crate::{CloudAsrProviderSettings, CloudTtsProviderSettings};
pub(crate) fn asr_settings(provider: &saaa_larm_session::Provider) -> CloudAsrProviderSettings {
    CloudAsrProviderSettings {
        id: "larm-session-asr".into(),
        enabled: true,
        label: "LARM ASR".into(),
        location: "local".into(),
        endpoint: provider.base_url.to_string(),
        model: provider.model.clone(),
        language: "auto".into(),
        authentication: "api-key".into(),
    }
}
pub(crate) fn tts_settings(
    provider: &saaa_larm_session::Provider,
    voice: Option<&str>,
) -> Result<CloudTtsProviderSettings, String> {
    Ok(CloudTtsProviderSettings {
        id: "larm-session-tts".into(),
        enabled: true,
        label: "LARM TTS".into(),
        location: "local".into(),
        endpoint: provider.base_url.to_string(),
        model: provider.model.clone(),
        voice: voice
            .or(provider.voice.as_deref())
            .ok_or("Select a TTS voice in Settings or advertise a default voice in LARM")?
            .to_string(),
        response_format: "wav".into(),
        authentication: "api-key".into(),
    })
}
