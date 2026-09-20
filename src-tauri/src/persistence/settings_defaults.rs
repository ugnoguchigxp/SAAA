use super::{regional_preferences, DEFAULT_CONVERSATION_TIMEOUT_MS, SETTINGS_SCHEMA_VERSION};
use crate::{
    providers, situation, DEFAULT_AGENT_NAME, DEFAULT_DYNAMIC_LAN_HOST, DEFAULT_USER_NAME,
    DYNAMIC_LAN_PROVIDER_ID,
};
use serde_json::{json, Value};

pub(crate) fn default_settings_documents() -> Vec<(&'static str, &'static str, i64, Value)> {
    vec![
        (
            "providers.model",
            "default",
            SETTINGS_SCHEMA_VERSION,
            json!({
                "harness": {
                    "address": format!("http://{}:9810", DEFAULT_DYNAMIC_LAN_HOST)
                },
                "providers": [{
                    "kind": "dynamic-lan",
                    "id": DYNAMIC_LAN_PROVIDER_ID,
                    "enabled": true,
                    "label": "Provider Harness LLM",
                    "location": "local",
                    "host": DEFAULT_DYNAMIC_LAN_HOST
                }, {
                    "kind": "system-tts",
                    "id": "system-tts",
                    "enabled": true,
                    "label": "System Voice",
                    "location": "local",
                    "voice": "default"
                }],
                "reasoningEffort": providers::DEFAULT_CONVERSATION_REASONING_EFFORT
            }),
        ),
        (
            "providers.agent",
            "codex-sdk",
            SETTINGS_SCHEMA_VERSION,
            json!({
                "agentName": DEFAULT_AGENT_NAME,
                "userName": DEFAULT_USER_NAME,
                "enabled": false,
                "provider": "codex-sdk",
                "model": "gpt-5.6-luna",
                "runtimeMode": "app-server",
                "health": "unchecked",
                "sandboxMode": "read-only",
                "approvalPolicy": "never",
                "networkEnabled": false,
                "webSearchEnabled": false,
                "workspacePolicy": "select-per-conversation"
            }),
        ),
        (
            "routing.tasks",
            "default",
            SETTINGS_SCHEMA_VERSION,
            json!({
                "conversationRespond": {
                    "source": "harness",
                    "primaryProviderId": null,
                    "fallbackProviderIds": [],
                    "timeoutMs": DEFAULT_CONVERSATION_TIMEOUT_MS
                },
                "voiceTranscribe": {
                    "source": "harness",
                    "providerId": null,
                    "timeoutMs": 120000
                },
                "voiceSpeak": {
                    "source": "provider",
                    "providerId": "system-tts",
                    "timeoutMs": 30000
                },
                "codingAssist": {
                    "providerId": "codex-sdk",
                    "timeoutMs": 120000,
                    "readOnly": true,
                    "networkEnabled": false,
                    "webSearchEnabled": false
                }
            }),
        ),
        (
            "voice.runtime",
            "default",
            SETTINGS_SCHEMA_VERSION,
            json!({
                "listeningEnabled": false,
                "inputDeviceId": "default",
                "outputDeviceId": "default",
                "vadSensitivity": "medium",
                "silenceTimeoutMs": 1500,
                "allowedLanguages": [crate::voice::language::DEFAULT_LANGUAGE_CODE],
                "autoSpeak": true
            }),
        ),
        (
            "security.runtime",
            "default",
            SETTINGS_SCHEMA_VERSION,
            json!({
                "localOnlyWhenSelected": true,
                "diagnosticsRedaction": true
            }),
        ),
        (
            "ui.preferences",
            "default",
            SETTINGS_SCHEMA_VERSION,
            regional_preferences::default_value(),
        ),
        (
            "situation.runtime",
            "default",
            SETTINGS_SCHEMA_VERSION,
            serde_json::to_value(situation::contracts::SituationRuntimeSettings::default())
                .expect("default Situation settings serialize"),
        ),
        (
            "routing.roles",
            "default",
            SETTINGS_SCHEMA_VERSION,
            crate::role_routing::default_document_value(),
        ),
    ]
}
