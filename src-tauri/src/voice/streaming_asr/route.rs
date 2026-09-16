use std::sync::Arc;

use rusqlite::OptionalExtension;

use super::{
    batch_runtime::{self, BatchDecode, BatchRoute},
    speaker_gate_runtime::{PreparedSpeakerScorer, SpeakerScorer},
};
use crate::{
    voice::session::{harness_asr_provider, select_streaming_asr, vad_rms_threshold, AsrRoute},
    AppState, RunCancellation,
};

pub(crate) struct PreparedSession {
    pub(crate) batch_decoder: Arc<dyn BatchDecode>,
    pub(crate) speaker_scorer: Option<Arc<dyn SpeakerScorer>>,
    pub(crate) vad_threshold: f32,
}

pub(crate) async fn prepare(
    state: &AppState,
    conversation_id: &str,
) -> Result<PreparedSession, String> {
    let (selected, verifier) = state.sqlite_readers.read(|connection| {
        let exists = connection
            .query_row(
                "SELECT 1 FROM conversations WHERE id=?1",
                [conversation_id],
                |_| Ok(()),
            )
            .optional()
            .map_err(|error| format!("Database error: {error}"))?
            .is_some();
        if !exists {
            return Err("Conversation does not exist".to_string());
        }
        Ok((
            select_streaming_asr(connection)?,
            state.voice_profile.prepare_streaming_verifier(connection)?,
        ))
    })?;

    let timeout_ms = selected.timeout_ms;
    let allowed_languages = selected.allowed_languages;
    let vad_threshold = vad_rms_threshold(&selected.vad_sensitivity);
    let scorer =
        verifier.map(|value| Arc::new(PreparedSpeakerScorer::new(value)) as Arc<dyn SpeakerScorer>);
    let batch_route = if crate::larm_voice::enabled() {
        BatchRoute::Larm(
            crate::larm_voice::current(conversation_id)
                .await?
                .session
                .clone(),
        )
    } else {
        match selected.route {
            AsrRoute::Cloud(provider) => BatchRoute::Cloud(provider),
            AsrRoute::Harness(address) => {
                match crate::providers::service_harness::resolve_asr_service(&address).await {
                    Ok(service) => BatchRoute::Cloud(harness_asr_provider(service.batch)),
                    Err(primary_error) => {
                        let Some(host) =
                            crate::providers::service_harness::legacy_dynamic_lan_host(&address)?
                        else {
                            return Err(if primary_error.starts_with("asr-") {
                                primary_error
                            } else {
                                "asr-provider-unavailable".to_string()
                            });
                        };
                        let resolution = state
                            .network_asr
                            .resolve(&host, Arc::new(RunCancellation::default()))
                            .await
                            .map_err(|_| "asr-provider-unavailable".to_string())?;
                        BatchRoute::LegacyNetwork {
                            client: state.network_asr.client().clone(),
                            endpoint: resolution.endpoint,
                            model: resolution.model,
                        }
                    }
                }
            }
        }
    };

    Ok(PreparedSession {
        batch_decoder: batch_runtime::decoder(
            batch_route,
            timeout_ms,
            allowed_languages.clone(),
            vad_threshold,
        ),
        speaker_scorer: scorer,
        vad_threshold,
    })
}
