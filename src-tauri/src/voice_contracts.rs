use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct TranscribeAudioInput {
    pub(crate) run_id: String,
    pub(crate) conversation_id: String,
    pub(crate) audio_upload_id: String,
    pub(crate) sample_rate: u32,
    pub(crate) model: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SpeakTextInput {
    pub(crate) run_id: String,
    pub(crate) conversation_id: String,
    pub(crate) text: String,
    pub(crate) voice: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct TtsVoiceDescriptor {
    pub(crate) id: String,
    pub(crate) label: String,
    pub(crate) language: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TtsCapabilities {
    pub(crate) available: bool,
    pub(crate) message: String,
    pub(crate) voices: Vec<TtsVoiceDescriptor>,
    pub(crate) output_devices: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub(crate) enum VoiceEvent {
    Transcribing {
        run_id: String,
    },
    TranscriptFinal {
        run_id: String,
        text: String,
    },
    Cancelled {
        run_id: String,
    },
    Failed {
        run_id: String,
        message: String,
        recovery: String,
    },
}
