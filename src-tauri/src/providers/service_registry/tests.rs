use super::*;
use crate::{
    CloudAsrProviderSettings, CloudTtsProviderSettings, ConversationRouteSettings, HarnessSettings,
    ModelProviderSettings, ModelProvidersSettings, OpenAiCompatibleProviderSettings,
    RoutingSettings, VoiceRouteSettings,
};

fn llm(id: &str, authentication: &str) -> ModelProviderSettings {
    ModelProviderSettings::OpenAiCompatible(OpenAiCompatibleProviderSettings {
        request_options: None,
        id: id.into(),
        enabled: true,
        label: id.into(),
        location: "cloud".into(),
        endpoint: "https://api.example.test/v1".into(),
        model: "m".into(),
        authentication: authentication.into(),
    })
}

fn asr(id: &str) -> ModelProviderSettings {
    ModelProviderSettings::CloudAsr(CloudAsrProviderSettings {
        id: id.into(),
        enabled: true,
        label: id.into(),
        location: "cloud".into(),
        endpoint: "https://api.example.test/v1".into(),
        model: "w".into(),
        language: "ja".into(),
        authentication: "api-key".into(),
        transport: "http".into(),
    })
}

fn tts(id: &str) -> ModelProviderSettings {
    ModelProviderSettings::CloudTts(CloudTtsProviderSettings {
        id: id.into(),
        enabled: true,
        label: id.into(),
        location: "cloud".into(),
        endpoint: "https://api.example.test/v1".into(),
        model: "t".into(),
        voice: "v".into(),
        response_format: "wav".into(),
        authentication: "none".into(),
        style: None,
        speed: None,
        pitch_scale: None,
        intonation_scale: None,
    })
}

fn providers() -> ModelProvidersSettings {
    ModelProvidersSettings {
        harness: HarnessSettings {
            larm_profile: None,
            tts_voice: None,
            tts_style: None,
            tts_speed: None,
            tts_pitch_scale: None,
            tts_intonation_scale: None,
            address: "http://larm.test".into(),
        },
        providers: vec![
            llm("cloud-llm", "api-key"),
            asr("cloud-asr"),
            tts("cloud-tts"),
        ],
        reasoning_effort: "medium".into(),
    }
}

fn voice(source: &str, id: Option<&str>) -> VoiceRouteSettings {
    VoiceRouteSettings {
        fallback_provider_ids: vec![],
        attempt_timeout_ms: None,
        source: source.into(),
        provider_id: id.map(str::to_string),
        timeout_ms: 30_000,
    }
}

fn routing(conversation_source: &str) -> RoutingSettings {
    RoutingSettings {
        conversation_respond: ConversationRouteSettings {
            attempt_timeout_ms: None,
            source: conversation_source.into(),
            primary_provider_id: (conversation_source == "provider").then(|| "cloud-llm".into()),
            fallback_provider_ids: vec![],
            timeout_ms: 60_000,
        },
        voice_transcribe: voice("provider", Some("cloud-asr")),
        voice_speak: voice("harness", None),
        coding_assist: crate::CodingRouteSettings {
            provider_id: "codex-sdk".into(),
            timeout_ms: 1,
            read_only: true,
            network_enabled: false,
            web_search_enabled: false,
        },
    }
}

#[test]
fn migration_is_idempotent_and_keeps_each_credential_separate() {
    let first = migrate_legacy(&providers(), &routing("harness")).unwrap();
    let second = migrate_legacy(&providers(), &routing("harness")).unwrap();
    assert_eq!(
        serde_json::to_value(&first).unwrap(),
        serde_json::to_value(&second).unwrap()
    );
    // Same endpoint on three providers: three connections, none merged.
    assert_eq!(first.connections.len(), 4);
    let llm = first.connection("conn:cloud-llm").unwrap();
    assert_eq!(llm.credential_ref.as_ref().unwrap().account, "cloud-llm");
    assert!(first
        .connection("conn:cloud-tts")
        .unwrap()
        .credential_ref
        .is_none());
}

#[test]
fn stored_cloud_conversation_needs_review_and_is_not_resolved() {
    let snapshot = migrate_legacy(&providers(), &routing("provider")).unwrap();
    let binding = snapshot.binding(Purpose::ConversationRespond).unwrap();
    assert_eq!(binding.review, BindingReview::NeedsReview);
    assert_eq!(
        binding.stored_primary_resource_id.as_deref(),
        Some("res:cloud-llm")
    );
    assert_eq!(
        binding.primary_resource_id.as_deref(),
        Some("res:harness-llm")
    );
    assert_eq!(
        resolve_route(&snapshot, Purpose::ConversationRespond),
        Err(ResolveError::NeedsReview(Purpose::ConversationRespond))
    );
}

#[test]
fn voice_routes_resolve_to_their_own_resources() {
    let snapshot = migrate_legacy(&providers(), &routing("harness")).unwrap();
    let asr = resolve_route(&snapshot, Purpose::VoiceTranscribe).unwrap();
    assert_eq!(asr.adapter_kind, AdapterKind::HttpAsr);
    assert_eq!(asr.resource_id, "res:cloud-asr");
    let tts = resolve_route(&snapshot, Purpose::VoiceSpeak).unwrap();
    assert_eq!(tts.adapter_kind, AdapterKind::Larm);
    let chat = resolve_route(&snapshot, Purpose::ConversationRespond).unwrap();
    assert_eq!(chat.adapter_kind, AdapterKind::Larm);
}

#[test]
fn fingerprint_changes_with_the_resource_but_not_unrelated_resources() {
    let mut snapshot = migrate_legacy(&providers(), &routing("harness")).unwrap();
    let before = resolve_route(&snapshot, Purpose::VoiceTranscribe).unwrap();
    snapshot.resource_mut_for_test("res:cloud-tts").model = "other".into();
    assert_eq!(
        before.fingerprint,
        resolve_route(&snapshot, Purpose::VoiceTranscribe)
            .unwrap()
            .fingerprint
    );
    snapshot.resource_mut_for_test("res:cloud-asr").model = "other".into();
    assert_ne!(
        before.fingerprint,
        resolve_route(&snapshot, Purpose::VoiceTranscribe)
            .unwrap()
            .fingerprint
    );
}

#[test]
fn disabled_resource_is_rejected_without_falling_back() {
    let mut snapshot = migrate_legacy(&providers(), &routing("harness")).unwrap();
    snapshot.resource_mut_for_test("res:cloud-asr").enabled = false;
    assert_eq!(
        resolve_route(&snapshot, Purpose::VoiceTranscribe),
        Err(ResolveError::ResourceDisabled("res:cloud-asr".into()))
    );
}

#[test]
fn validation_rejects_capability_mismatch_duplicates_and_unknown_ids() {
    let base = migrate_legacy(&providers(), &routing("harness")).unwrap();
    let mut wrong = base.clone();
    wrong.bindings[1].primary_resource_id = Some("res:cloud-tts".into());
    assert!(validate_snapshot(&wrong).is_err());
    let mut duplicate = base.clone();
    duplicate.bindings[1].fallback_resource_ids = vec!["res:cloud-asr".into()];
    assert!(validate_snapshot(&duplicate).is_err());
    let mut unknown = base.clone();
    unknown.bindings[1].primary_resource_id = Some("res:missing".into());
    assert!(validate_snapshot(&unknown).is_err());
}

#[test]
fn legacy_route_with_unknown_provider_is_an_error_not_a_silent_drop() {
    let mut route = routing("harness");
    route.voice_transcribe.provider_id = Some("ghost".into());
    assert!(migrate_legacy(&providers(), &route).is_err());
}

#[test]
fn credential_ref_serializes_without_secret_material() {
    let snapshot = migrate_legacy(&providers(), &routing("harness")).unwrap();
    let text = serde_json::to_string(&snapshot).unwrap();
    assert!(text.contains("\"credentialRef\""));
    assert!(!text.to_lowercase().contains("secret"));
}

#[test]
fn migration_pins_explicit_model_options_and_rejects_unsupported_adapters() {
    use saaa_larm_session::http_api::{LlmOptions, Thinking, TokenLimit};
    let mut providers = providers();
    if let ModelProviderSettings::OpenAiCompatible(p) = &mut providers.providers[0] {
        p.request_options = Some(LlmOptions {
            token_limit: TokenLimit::Completion,
            thinking: Thinking::Disabled,
            temperature: Some(0.4),
            ..Default::default()
        });
    }
    let mut snapshot = migrate_legacy(&providers, &routing("provider")).unwrap();
    let binding = &mut snapshot.bindings[0];
    binding.primary_resource_id = Some("res:cloud-llm".into());
    binding.review = BindingReview::Ready;
    let route = resolve_route(&snapshot, Purpose::ConversationRespond).unwrap();
    let options: LlmOptions = serde_json::from_value(route.request_options.unwrap()).unwrap();
    let mut request = serde_json::json!({});
    options.apply(&mut request, "custom-model", 4096, "provider-default");
    assert_eq!(request["max_completion_tokens"], 4096);
    assert_eq!(request["chat_template_kwargs"]["enable_thinking"], false);
    assert_eq!(request["temperature"], serde_json::json!(0.4f32));
    snapshot.connections[1].adapter_kind = AdapterKind::AgentSession;
    assert!(validate_snapshot(&snapshot).is_err());
    assert!(resolve_route(&snapshot, Purpose::ConversationRespond).is_err());
    snapshot.connections[1].adapter_kind = AdapterKind::Larm;
    assert!(validate_snapshot(&snapshot).is_err());
}
