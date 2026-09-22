#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{
        default_settings_input, direct_provider, dynamic_lan_provider, provider,
    };
    use crate::{
        initialize_database, providers, CodexAgentRuntimeSettings, ModelProviderSettings,
        ModelProvidersSettings, OpenAiCompatibleProviderSettings, DEFAULT_AGENT_NAME,
        DEFAULT_USER_NAME, DYNAMIC_LAN_PROVIDER_ID,
    };
    use rusqlite::Connection;
    use serde_json::{json, Value};

    #[test]
    fn defaults_conversation_timeout_to_thirty_minutes() {
        let documents = default_settings_documents();
        let routing = documents
            .iter()
            .find(|(namespace, key, _, _)| *namespace == "routing.tasks" && *key == "default")
            .expect("default routing settings");
        assert_eq!(
            routing.3.pointer("/conversationRespond/timeoutMs"),
            Some(&json!(1_800_000))
        );
    }

    #[test]
    fn enabled_role_policy_requires_a_matching_enabled_llm_provider() {
        let mut documents = default_settings_input();
        let role_policy_index = documents
            .iter()
            .position(|document| document.namespace == "routing.roles")
            .expect("role routing settings");
        documents[role_policy_index].value_json = json!({
            "schemaVersion": 1,
            "enabled": true,
            "actors": [{
                "id": "reasoner", "label": "Reasoner", "aliases": [],
                "transport": "provider", "providerId": DYNAMIC_LAN_PROVIDER_ID,
                "model": null, "location": "local", "resourceGroup": "gpu",
                "maxInputBytes": 4096, "capabilities": ["reason"]
            }],
            "roles": {"frontend": null, "reasoner": "reasoner", "advanced": null, "reviewer": null, "premium": null, "toolSpecialist": null},
            "recipes": [{"id": "direct", "action": "respond", "roles": ["reasoner"], "enabled": true}],
            "limits": {"maxReasoningSteps": 4, "maxToolCalls": 32, "rootTimeoutMs": 180000, "stepTimeoutMs": 60000, "frontendTimeoutMs": 1200, "classificationTimeoutMs": 1500, "maxQueuedInputs": 4, "maxReviewRounds": 1, "maxAutomaticSwitches": 2, "maxEstimatedCostMicros": null},
            "speech": {"mode": "author_verbatim", "ackDelayMs": 250, "maxAckChars": 80, "progressMinIntervalMs": 15000, "maxProgressPerRoot": 2},
            "selection": {"mode": "rules", "shadowArtifactId": null, "classificationMinConfidence": 0.85, "weights": {"quality": 0.6, "latency": 0.25, "cost": 0.15}, "switchMargin": 0.15},
            "premiumApproval": "per_request",
            "learning": {"enabled": false, "localStart": "02:00", "localEnd": "05:00", "idleSeconds": 300, "maxRunSeconds": 600, "batchSize": 100, "allowLocalLabeler": false}
        });
        assert!(validate_settings_batch(&documents).is_ok());
        documents[role_policy_index].value_json["actors"][0]["providerId"] = json!("missing");
        assert!(validate_settings_batch(&documents).is_err());
    }

    #[test]
    fn ambient_listening_requires_consent_and_persists_immediately() {
        let connection = Connection::open_in_memory().expect("database opens");
        initialize_database(&connection).expect("database initializes");
        let defaults = load_voice_settings(&connection).expect("voice settings load");
        assert!(!defaults.listening_enabled);

        let enabled = set_voice_listening_enabled_to_connection(&connection, true)
            .expect("listening preference saves");
        assert_eq!(enabled.value_json["listeningEnabled"], true);
        assert_eq!(enabled.schema_version, SETTINGS_SCHEMA_VERSION);
        assert!(
            load_voice_settings(&connection)
                .expect("updated voice settings load")
                .listening_enabled
        );
    }

    #[test]
    fn accepts_conversation_timeout_up_to_one_hour() {
        let mut routing = serde_json::from_value::<RoutingSettings>(
            default_settings_documents()
                .into_iter()
                .find(|(namespace, key, _, _)| *namespace == "routing.tasks" && *key == "default")
                .expect("default routing settings")
                .3,
        )
        .expect("routing settings decode");
        routing.conversation_respond.timeout_ms = MAX_CONVERSATION_TIMEOUT_MS;
        assert!(validate_routing_settings(&routing).is_ok());
        routing.conversation_respond.timeout_ms = MAX_CONVERSATION_TIMEOUT_MS + 1;
        assert!(validate_routing_settings(&routing).is_err());
    }

    #[test]
    fn settings_survive_reopen_and_invalid_batch_does_not_replace_snapshot() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("settings.sqlite3");
        let mut connection = Connection::open(&path).expect("database opens");
        initialize_database(&connection).expect("database initializes");
        let mut documents = default_settings_input();
        let security = documents
            .iter_mut()
            .find(|document| document.namespace == "security.runtime")
            .expect("security document");
        security.value_json["localOnlyWhenSelected"] = Value::Bool(false);
        let routing = documents
            .iter_mut()
            .find(|document| document.namespace == "routing.tasks")
            .expect("routing document");
        routing.value_json["conversationRespond"]["timeoutMs"] = json!(1_234_000);
        save_settings_documents_to_connection(&mut connection, &documents)
            .expect("valid settings save");
        drop(connection);

        let mut reopened = Connection::open(&path).expect("database reopens");
        initialize_database(&reopened).expect("database reinitializes");
        let loaded = list_settings_documents(&reopened).expect("settings reload");
        let security = loaded
            .iter()
            .find(|document| document.namespace == "security.runtime")
            .expect("security settings reload");
        assert_eq!(security.value_json["localOnlyWhenSelected"], false);
        let routing = loaded
            .iter()
            .find(|document| document.namespace == "routing.tasks")
            .expect("routing settings reload");
        assert_eq!(
            routing.value_json["conversationRespond"]["timeoutMs"],
            1_234_000
        );

        let mut invalid = default_settings_input();
        invalid
            .iter_mut()
            .find(|document| document.namespace == "security.runtime")
            .expect("security document")
            .value_json["credentialStorage"] = Value::String("plaintext".to_string());
        assert!(save_settings_documents_to_connection(&mut reopened, &invalid).is_err());
        let unchanged = read_settings_document(&reopened, "security.runtime", "default")
            .expect("previous snapshot remains");
        assert_eq!(unchanged.value_json["localOnlyWhenSelected"], false);
    }

    #[test]
    fn settings_reject_embedded_credentials_and_cloud_fallback_on_local_route() {
        assert!(validate_model_providers(&ModelProvidersSettings {
            providers: vec![provider("local", "local")],
            reasoning_effort: "mid".to_string(),
            harness: crate::HarnessSettings {
                larm_profile: None,
                tts_voice: None,
                address: "http://localhost:9810".to_string()
            },
        })
        .is_err());
        let mut dynamic_lan = direct_provider(DYNAMIC_LAN_PROVIDER_ID, "local");
        dynamic_lan.endpoint = "http://10.0.0.42:8083/v1".to_string();
        dynamic_lan.model = "ornith15-35b".to_string();
        assert!(validate_model_providers(&ModelProvidersSettings {
            providers: vec![ModelProviderSettings::OpenAiCompatible(dynamic_lan)],
            reasoning_effort: providers::default_conversation_reasoning_effort(),
            harness: crate::HarnessSettings {
                larm_profile: None,
                tts_voice: None,
                address: "http://localhost:9810".to_string()
            },
        })
        .is_ok());
        let mut public_http = direct_provider("public-http", "local");
        public_http.endpoint = "http://203.0.113.10:8080/v1".to_string();
        assert!(validate_model_providers(&ModelProvidersSettings {
            providers: vec![ModelProviderSettings::OpenAiCompatible(public_http)],
            reasoning_effort: providers::default_conversation_reasoning_effort(),
            harness: crate::HarnessSettings {
                larm_profile: None,
                tts_voice: None,
                address: "http://localhost:9810".to_string()
            },
        })
        .is_err());

        let with_credentials = ModelProvidersSettings {
            providers: vec![ModelProviderSettings::OpenAiCompatible(
                OpenAiCompatibleProviderSettings {
                    request_options: None,
                    endpoint: "https://user:secret@example.invalid/v1".to_string(),
                    ..direct_provider("cloud", "cloud")
                },
            )],
            reasoning_effort: providers::default_conversation_reasoning_effort(),
            harness: crate::HarnessSettings {
                larm_profile: None,
                tts_voice: None,
                address: "http://localhost:9810".to_string(),
            },
        };
        assert!(validate_model_providers(&with_credentials).is_err());
        let mut disabled_with_credentials = with_credentials;
        let ModelProviderSettings::OpenAiCompatible(disabled_provider) =
            &mut disabled_with_credentials.providers[0]
        else {
            unreachable!("fixture is direct provider");
        };
        disabled_provider.enabled = false;
        assert!(validate_model_providers(&disabled_with_credentials).is_err());

        let unsafe_id = ModelProvidersSettings {
            providers: vec![provider("local provider", "local")],
            reasoning_effort: providers::default_conversation_reasoning_effort(),
            harness: crate::HarnessSettings {
                larm_profile: None,
                tts_voice: None,
                address: "http://localhost:9810".to_string(),
            },
        };
        assert!(validate_model_providers(&unsafe_id).is_err());
        let ambiguous_ids = ModelProvidersSettings {
            providers: vec![provider("local-a", "local"), provider("local_a", "local")],
            reasoning_effort: providers::default_conversation_reasoning_effort(),
            harness: crate::HarnessSettings {
                larm_profile: None,
                tts_voice: None,
                address: "http://localhost:9810".to_string(),
            },
        };
        assert!(validate_model_providers(&ambiguous_ids).is_ok());

        let mut documents = default_settings_input();
        documents
            .iter_mut()
            .find(|document| document.namespace == "providers.model")
            .expect("provider settings")
            .value_json = json!({
            "harness": { "address": "http://localhost:9810" },
            "providers": [provider("local", "local"), provider("cloud", "cloud")],
            "reasoningEffort": "medium"
        });
        let routing = documents
            .iter_mut()
            .find(|document| document.namespace == "routing.tasks")
            .expect("routing settings");
        routing.value_json["conversationRespond"]["source"] = json!("provider");
        routing.value_json["conversationRespond"]["primaryProviderId"] = json!("local");
        routing.value_json["conversationRespond"]["fallbackProviderIds"] = json!(["cloud"]);
        assert!(validate_settings_batch(&documents).is_err());

        let mut documents = default_settings_input();
        documents
            .iter_mut()
            .find(|document| document.namespace == "providers.model")
            .expect("provider settings")
            .value_json = json!({
            "harness": { "address": "http://localhost:9810" },
            "providers": [
                dynamic_lan_provider("dynamic_lan-primary"),
                provider("local-fallback", "local")
            ],
            "reasoningEffort": "medium"
        });
        let routing_index = documents
            .iter()
            .position(|document| document.namespace == "routing.tasks")
            .expect("routing settings");
        documents[routing_index].value_json["conversationRespond"]["primaryProviderId"] =
            json!("dynamic_lan-primary");
        documents[routing_index].value_json["conversationRespond"]["source"] = json!("provider");
        documents[routing_index].value_json["conversationRespond"]["timeoutMs"] = json!(30_000);
        documents[routing_index].value_json["voiceSpeak"]["source"] = json!("harness");
        documents[routing_index].value_json["voiceSpeak"]["providerId"] = Value::Null;
        documents[routing_index].value_json["conversationRespond"]["fallbackProviderIds"] =
            json!(["local-fallback"]);
        assert!(validate_settings_batch(&documents).is_ok());

        documents[routing_index].value_json["conversationRespond"]["fallbackProviderIds"] =
            json!([]);
        documents[routing_index].value_json["conversationRespond"]["timeoutMs"] =
            json!(providers::dynamic_lan::MAX_REQUEST_TIMEOUT_MS + 1);
        assert!(validate_settings_batch(&documents).is_err());
        documents[routing_index].value_json["conversationRespond"]["timeoutMs"] =
            json!(providers::dynamic_lan::MAX_REQUEST_TIMEOUT_MS);
        assert!(validate_settings_batch(&documents).is_ok());
    }

    #[test]
    fn harness_routes_require_a_nonempty_address() {
        let mut documents = default_settings_input();
        let provider_settings = documents
            .iter_mut()
            .find(|document| document.namespace == "providers.model")
            .expect("provider settings");
        provider_settings.value_json["harness"]["address"] = json!("");
        assert!(validate_settings_batch(&documents).is_err());
    }

    #[test]
    fn legacy_provider_ids_and_default_codex_model_remain_valid() {
        let providers = ModelProvidersSettings {
            providers: vec![provider("Local_Custom", "local")],
            reasoning_effort: providers::default_conversation_reasoning_effort(),
            harness: crate::HarnessSettings {
                larm_profile: None,
                tts_voice: None,
                address: "http://localhost:9810".to_string(),
            },
        };
        assert!(validate_model_providers(&providers).is_ok());

        let codex = CodexAgentRuntimeSettings {
            agent_name: DEFAULT_AGENT_NAME.to_string(),
            user_name: DEFAULT_USER_NAME.to_string(),
            enabled: true,
            provider: "codex-sdk".to_string(),
            model: String::new(),
            runtime_mode: "app-server".to_string(),
            health: "unchecked".to_string(),
            sandbox_mode: "read-only".to_string(),
            approval_policy: "never".to_string(),
            network_enabled: false,
            web_search_enabled: false,
            workspace_policy: "select-per-conversation".to_string(),
        };
        assert!(validate_codex_settings(&codex).is_ok());
    }

    #[test]
    fn human_readable_setting_limits_count_characters_not_utf8_bytes() {
        let mut provider = direct_provider("localized", "local");
        provider.label = "あ".repeat(120);
        provider.model = "モ".repeat(160);
        assert!(validate_model_providers(&ModelProvidersSettings {
            providers: vec![ModelProviderSettings::OpenAiCompatible(provider.clone())],
            reasoning_effort: providers::default_conversation_reasoning_effort(),
            harness: crate::HarnessSettings {
                larm_profile: None,
                tts_voice: None,
                address: "http://localhost:9810".to_string()
            },
        })
        .is_ok());

        provider.label.push('あ');
        assert!(validate_model_providers(&ModelProvidersSettings {
            providers: vec![ModelProviderSettings::OpenAiCompatible(provider)],
            reasoning_effort: providers::default_conversation_reasoning_effort(),
            harness: crate::HarnessSettings {
                larm_profile: None,
                tts_voice: None,
                address: "http://localhost:9810".to_string()
            },
        })
        .is_err());

        let mut documents = default_settings_input();
        let voice = documents
            .iter_mut()
            .find(|document| document.namespace == "voice.runtime")
            .expect("voice settings");
        voice.value_json["inputDeviceId"] = json!("声".repeat(100));
        validate_settings_document(voice).expect("bounded localized device id is accepted");
        voice.value_json["inputDeviceId"] = json!("声".repeat(101));
        assert!(validate_settings_document(voice).is_err());
    }

    #[test]
    fn voice_settings_require_bounded_continuous_listening_options() {
        let mut documents = default_settings_input();
        let voice = documents
            .iter_mut()
            .find(|document| document.namespace == "voice.runtime")
            .expect("voice settings");
        voice.value_json["vadSensitivity"] = json!("maximum");
        assert_eq!(
            validate_settings_document(voice).expect_err("unknown sensitivity is rejected"),
            "Invalid continuous listening settings"
        );

        voice.value_json["vadSensitivity"] = json!("medium");
        voice.value_json["silenceTimeoutMs"] = json!(3_001);
        assert!(validate_settings_document(voice).is_err());
    }

    #[test]
    fn voice_settings_require_registered_supported_languages() {
        let mut documents = default_settings_input();
        let voice = documents
            .iter_mut()
            .find(|document| document.namespace == "voice.runtime")
            .expect("voice settings");
        voice.value_json["allowedLanguages"] = json!(["ja", "en"]);
        validate_settings_document(voice).expect("supported languages are accepted");

        for invalid in [json!([]), json!(["xx"]), json!(["ja", "ja"])] {
            voice.value_json["allowedLanguages"] = invalid;
            assert_eq!(
                validate_settings_document(voice).expect_err("invalid languages are rejected"),
                "Invalid continuous listening settings"
            );
        }
    }
}
