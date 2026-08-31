use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SpeakTextInput {
    pub(crate) run_id: String,
    pub(crate) conversation_id: String,
    pub(crate) text: String,
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
