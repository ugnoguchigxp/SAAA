use saaa_larm_session::http_api::{LlmOptions, Thinking, TokenLimit};

use crate::{
    migrate_legacy, resolve_route, validate_snapshot, AdapterKind, BindingReview, Capability,
    LarmReachability, LegacyHarness, LegacyProvider, LegacyRoute, LegacySettings, Purpose,
};

fn provider(
    id: &str,
    adapter: AdapterKind,
    capability: Capability,
    authentication: &str,
    model: &str,
    detail: Option<&str>,
    request_options: Option<serde_json::Value>,
) -> LegacyProvider {
    LegacyProvider {
        id: id.into(),
        label: id.into(),
        location: "cloud".into(),
        enabled: true,
        adapter_kind: adapter,
        capability,
        endpoint: "https://api.example.test/v1".into(),
        authentication: authentication.into(),
        model: model.into(),
        detail: detail.map(str::to_string),
        request_options,
    }
}

fn route(
    source: &str,
    primary: Option<&str>,
    provider_id: Option<&str>,
    timeout_ms: u64,
) -> LegacyRoute {
    LegacyRoute {
        source: source.into(),
        primary_provider_id: primary.map(str::to_string),
        provider_id: provider_id.map(str::to_string),
        fallback_provider_ids: Vec::new(),
        timeout_ms,
        attempt_timeout_ms: None,
    }
}

fn settings(conversation_source: &str) -> LegacySettings {
    LegacySettings {
        harness: LegacyHarness {
            address: "http://larm.test".into(),
            tts_voice: None,
        },
        providers: vec![
            provider(
                "cloud-llm",
                AdapterKind::ChatCompletions,
                Capability::TextGeneration,
                "api-key",
                "m",
                None,
                None,
            ),
            provider(
                "cloud-asr",
                AdapterKind::HttpAsr,
                Capability::Transcription,
                "api-key",
                "w",
                Some("ja"),
                None,
            ),
            provider(
                "cloud-tts",
                AdapterKind::HttpTts,
                Capability::Speech,
                "none",
                "t",
                Some("v"),
                None,
            ),
        ],
        conversation_respond: route(
            conversation_source,
            (conversation_source == "provider").then_some("cloud-llm"),
            None,
            60_000,
        ),
        voice_transcribe: route("provider", None, Some("cloud-asr"), 30_000),
        voice_speak: route("harness", None, None, 30_000),
    }
}

#[test]
fn migration_is_idempotent_and_keeps_each_credential_separate() {
    let first = migrate_legacy(&settings("harness")).unwrap();
    let second = migrate_legacy(&settings("harness")).unwrap();
    assert_eq!(
        serde_json::to_value(&first).unwrap(),
        serde_json::to_value(&second).unwrap()
    );
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
    let snapshot = migrate_legacy(&settings("provider")).unwrap();
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
        resolve_route(
            &snapshot,
            Purpose::ConversationRespond,
            LarmReachability::Unknown
        ),
        Err(crate::ResolveError::NeedsReview(
            Purpose::ConversationRespond
        ))
    );
}

#[test]
fn voice_routes_resolve_to_their_own_resources() {
    let snapshot = migrate_legacy(&settings("harness")).unwrap();
    let asr = resolve_route(
        &snapshot,
        Purpose::VoiceTranscribe,
        LarmReachability::Unknown,
    )
    .unwrap();
    assert_eq!(asr.adapter_kind, AdapterKind::HttpAsr);
    assert_eq!(asr.resource_id, "res:cloud-asr");
    let tts = resolve_route(&snapshot, Purpose::VoiceSpeak, LarmReachability::Unknown).unwrap();
    assert_eq!(tts.adapter_kind, AdapterKind::Larm);
    let chat = resolve_route(
        &snapshot,
        Purpose::ConversationRespond,
        LarmReachability::Unknown,
    )
    .unwrap();
    assert_eq!(chat.adapter_kind, AdapterKind::Larm);
}

#[test]
fn legacy_route_with_unknown_provider_is_an_error_not_a_silent_drop() {
    let mut settings = settings("harness");
    settings.voice_transcribe.provider_id = Some("ghost".into());
    assert!(migrate_legacy(&settings).is_err());
}

#[test]
fn credential_ref_serializes_without_secret_material() {
    let snapshot = migrate_legacy(&settings("harness")).unwrap();
    let text = serde_json::to_string(&snapshot).unwrap();
    assert!(text.contains("\"credentialRef\""));
    assert!(!text.to_lowercase().contains("secret"));
}

#[test]
fn migration_pins_explicit_model_options_and_rejects_unsupported_adapters() {
    let mut settings = settings("provider");
    settings.providers[0].request_options = Some(
        serde_json::to_value(LlmOptions {
            token_limit: TokenLimit::Completion,
            thinking: Thinking::Disabled,
            temperature: Some(0.4),
            ..Default::default()
        })
        .unwrap(),
    );
    let mut snapshot = migrate_legacy(&settings).unwrap();
    let binding = &mut snapshot.bindings[0];
    binding.primary_resource_id = Some("res:cloud-llm".into());
    binding.review = BindingReview::Ready;
    let route = resolve_route(
        &snapshot,
        Purpose::ConversationRespond,
        LarmReachability::Unknown,
    )
    .unwrap();
    let options: LlmOptions = serde_json::from_value(route.request_options.unwrap()).unwrap();
    let mut request = serde_json::json!({});
    options.apply(&mut request, "custom-model", 4096, "provider-default");
    assert_eq!(request["max_completion_tokens"], 4096);
    assert_eq!(request["chat_template_kwargs"]["enable_thinking"], false);
    assert_eq!(request["temperature"], serde_json::json!(0.4f32));
    snapshot.connections[1].adapter_kind = AdapterKind::AgentSession;
    assert!(validate_snapshot(&snapshot).is_err());
    assert!(resolve_route(
        &snapshot,
        Purpose::ConversationRespond,
        LarmReachability::Unknown
    )
    .is_err());
    snapshot.connections[1].adapter_kind = AdapterKind::Larm;
    assert!(validate_snapshot(&snapshot).is_err());
}
