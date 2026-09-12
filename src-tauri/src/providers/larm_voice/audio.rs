use crate::{CloudAsrProviderSettings, CloudTtsProviderSettings, RunCancellation};
use std::sync::Arc;

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
pub(crate) fn tts_settings(provider: &saaa_larm_session::Provider) -> CloudTtsProviderSettings {
    CloudTtsProviderSettings {
        id: "larm-session-tts".into(),
        enabled: true,
        label: "LARM TTS".into(),
        location: "local".into(),
        endpoint: provider.base_url.to_string(),
        model: provider.model.clone(),
        voice: "Kasukabe_Tsumugi".into(),
        response_format: "wav".into(),
        authentication: "api-key".into(),
    }
}
pub(crate) async fn transcribe(
    session: &Arc<saaa_larm_session::Session>,
    samples: &[f32],
    cancellation: Arc<RunCancellation>,
) -> Result<(String, Option<String>), String> {
    tokio::select! { biased;
        _ = cancellation.cancelled() => Err("asr-cancelled".into()),
        result = tokio::time::timeout(std::time::Duration::from_secs(15), async {
            let lease = session.acquire("asr").await.map_err(str::to_string)?;
            crate::voice::cloud_asr::transcribe_with_api_key(&asr_settings(lease.provider()), samples, 16_000, 15_000,
                cancellation.clone(), Some(lease.provider().token())).await
        }) => result.map_err(|_| "LARM ASR timed out".to_string())?,
    }
}
