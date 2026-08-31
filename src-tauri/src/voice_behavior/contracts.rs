use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use ts_rs::TS;

use super::UPDATE_VOICE_BEHAVIOR_TOOL_NAME;

#[derive(Debug, Clone, Deserialize, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub(crate) struct VoicePresentationDecision {
    #[ts(type = "\"speak\" | \"silent\"")]
    pub(crate) decision: String,
    #[ts(
        type = "\"meeting_blocked\" | \"global_opt_out\" | \"turn_override\" | \"conversation_override\" | \"global_default\" | \"route_blocked\""
    )]
    pub(crate) reason_code: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ConversationVoicePolicySnapshot {
    pub(crate) conversation_id: String,
    #[ts(type = "\"inherit\" | \"muted\"")]
    pub(crate) speech_output: String,
    #[ts(type = "\"inherit\" | \"quick\" | \"balanced\" | \"patient\"")]
    pub(crate) listening_pace: String,
    #[ts(type = "number")]
    pub(crate) policy_revision: i64,
    pub(crate) updated_at: String,
    #[ts(type = "\"speak\" | \"silent\"")]
    pub(crate) effective_speech_output: String,
    #[ts(
        type = "\"meeting_blocked\" | \"global_opt_out\" | \"conversation_override\" | \"global_default\""
    )]
    pub(crate) speech_reason_code: String,
    #[ts(type = "\"quick\" | \"balanced\" | \"patient\"")]
    pub(crate) effective_listening_pace: String,
    pub(crate) effective_silence_timeout_ms: u32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct UpdateConversationVoicePolicyInput {
    pub(crate) conversation_id: String,
    pub(crate) speech_output: Option<String>,
    pub(crate) listening_pace: Option<String>,
    pub(crate) expected_revision: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ResetConversationVoicePolicyInput {
    pub(crate) conversation_id: String,
    pub(crate) expected_revision: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct VoiceToolArguments {
    pub(super) speech_output: Option<SpeechOutputChange>,
    pub(super) listening_pace: Option<ListeningPaceChange>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct SpeechOutputChange {
    pub(super) mode: String,
    pub(super) scope: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ListeningPaceChange {
    pub(super) mode: String,
    pub(super) scope: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct VoiceToolResult {
    pub(super) applied: bool,
    pub(super) policy_revision: i64,
    pub(super) outcomes: VoiceToolOutcomes,
    pub(super) effective: VoiceToolEffective,
    pub(super) takes_effect: VoiceToolTakesEffect,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) error: Option<VoiceToolError>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct VoiceToolOutcomes {
    pub(super) speech_output: &'static str,
    pub(super) listening_pace: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct VoiceToolEffective {
    pub(super) speech_output: String,
    pub(super) speech_reason_code: String,
    pub(super) listening_pace: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct VoiceToolTakesEffect {
    pub(super) speech_output: &'static str,
    pub(super) listening_pace: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct VoiceToolError {
    pub(super) code: &'static str,
    pub(super) message: &'static str,
}

pub(crate) fn tool_definition() -> Value {
    json!({
        "type": "function",
        "function": {
            "name": UPDATE_VOICE_BEHAVIOR_TOOL_NAME,
            "description": "Change only how the current conversation speaks and waits between user turns when the current user message clearly requests it.",
            "strict": true,
            "parameters": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "speechOutput": {
                        "type": ["object", "null"],
                        "additionalProperties": false,
                        "properties": {
                            "mode": { "type": "string", "enum": ["silent", "speak", "inherit"] },
                            "scope": { "type": "string", "enum": ["current_response", "conversation"] }
                        },
                        "required": ["mode", "scope"]
                    },
                    "listeningPace": {
                        "type": ["object", "null"],
                        "additionalProperties": false,
                        "properties": {
                            "mode": { "type": "string", "enum": ["quick", "balanced", "patient", "default"] },
                            "scope": { "type": "string", "enum": ["conversation"] }
                        },
                        "required": ["mode", "scope"]
                    }
                },
                "required": ["speechOutput", "listeningPace"]
            }
        }
    })
}

pub(super) fn parse_tool_arguments(arguments: &str) -> Result<VoiceToolArguments, ()> {
    let value: Value = serde_json::from_str(arguments).map_err(|_| ())?;
    let object = value.as_object().ok_or(())?;
    if object.len() != 2
        || !object.contains_key("speechOutput")
        || !object.contains_key("listeningPace")
    {
        return Err(());
    }
    let parsed: VoiceToolArguments = serde_json::from_value(value).map_err(|_| ())?;
    validate_tool_arguments(&parsed)?;
    Ok(parsed)
}

fn validate_tool_arguments(arguments: &VoiceToolArguments) -> Result<(), ()> {
    if arguments.speech_output.is_none() && arguments.listening_pace.is_none() {
        return Err(());
    }
    if let Some(change) = &arguments.speech_output {
        let allowed = matches!(
            (change.mode.as_str(), change.scope.as_str()),
            ("silent", "current_response")
                | ("silent", "conversation")
                | ("speak", "current_response")
                | ("inherit", "conversation")
        );
        if !allowed {
            return Err(());
        }
    }
    if let Some(change) = &arguments.listening_pace {
        if change.scope != "conversation"
            || !matches!(
                change.mode.as_str(),
                "quick" | "balanced" | "patient" | "default"
            )
        {
            return Err(());
        }
    }
    Ok(())
}

pub(super) fn encode_tool_result(result: VoiceToolResult) -> Result<String, String> {
    serde_json::to_string(&result)
        .map_err(|_| "The conversation voice policy result could not be encoded".to_string())
}
