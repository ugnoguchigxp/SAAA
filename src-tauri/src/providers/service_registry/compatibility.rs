//! Executable contracts, shared by validation, resolution and UI projection.
use super::types::*;

pub(crate) fn unsupported_reason(
    snapshot: &RegistrySnapshot,
    purpose: Purpose,
    resource_id: &str,
) -> Option<String> {
    let resource = snapshot.resource(resource_id)?;
    let connection = snapshot.connection(&resource.connection_id)?;
    let supported = match purpose {
        Purpose::ConversationRespond => {
            matches!(
                connection.adapter_kind,
                AdapterKind::ChatCompletions | AdapterKind::AnthropicMessages
            ) || (connection.adapter_kind == AdapterKind::Larm
                && connection.connection_id == "conn:harness"
                && resource_id == "res:harness-llm")
        }
        Purpose::VoiceTranscribe => {
            resource_id == "res:harness-asr" || (connection.adapter_kind == AdapterKind::HttpAsr)
        }
        Purpose::VoiceSpeak => {
            resource_id == "res:harness-tts"
                || (matches!(
                    connection.adapter_kind,
                    AdapterKind::HttpTts | AdapterKind::SystemTts
                ))
        }
        Purpose::MediaImageGenerate | Purpose::MediaMusicGenerate => {
            connection.adapter_kind == AdapterKind::ReplicateMedia
                || (connection.connection_id == "conn:harness"
                    && connection.adapter_kind == AdapterKind::Larm)
        }
    };
    (!supported).then(|| "この接続方式は、この用途の実行経路にまだ対応していません".into())
}
