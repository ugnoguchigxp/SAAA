use super::*;
use crate::providers::reachability::Reachability;
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
        resolve_route(&snapshot, Purpose::ConversationRespond, Default::default()),
        Err(ResolveError::NeedsReview(Purpose::ConversationRespond))
    );
}

#[test]
fn voice_routes_resolve_to_their_own_resources() {
    let snapshot = migrate_legacy(&providers(), &routing("harness")).unwrap();
    let asr = resolve_route(&snapshot, Purpose::VoiceTranscribe, Default::default()).unwrap();
    assert_eq!(asr.adapter_kind, AdapterKind::HttpAsr);
    assert_eq!(asr.resource_id, "res:cloud-asr");
    let tts = resolve_route(&snapshot, Purpose::VoiceSpeak, Default::default()).unwrap();
    assert_eq!(tts.adapter_kind, AdapterKind::Larm);
    let chat = resolve_route(&snapshot, Purpose::ConversationRespond, Default::default()).unwrap();
    assert_eq!(chat.adapter_kind, AdapterKind::Larm);
}

#[test]
fn fingerprint_changes_with_the_resource_but_not_unrelated_resources() {
    let mut snapshot = migrate_legacy(&providers(), &routing("harness")).unwrap();
    let before = resolve_route(&snapshot, Purpose::VoiceTranscribe, Default::default()).unwrap();
    resource_mut(&mut snapshot, "res:cloud-tts").model = "other".into();
    assert_eq!(
        before.fingerprint,
        resolve_route(&snapshot, Purpose::VoiceTranscribe, Default::default())
            .unwrap()
            .fingerprint
    );
    resource_mut(&mut snapshot, "res:cloud-asr").model = "other".into();
    assert_ne!(
        before.fingerprint,
        resolve_route(&snapshot, Purpose::VoiceTranscribe, Default::default())
            .unwrap()
            .fingerprint
    );
}

#[test]
fn disabled_resource_is_rejected_without_falling_back() {
    let mut snapshot = migrate_legacy(&providers(), &routing("harness")).unwrap();
    resource_mut(&mut snapshot, "res:cloud-asr").enabled = false;
    assert_eq!(
        resolve_route(&snapshot, Purpose::VoiceTranscribe, Default::default()),
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
    let route = resolve_route(&snapshot, Purpose::ConversationRespond, Default::default()).unwrap();
    let options: LlmOptions = serde_json::from_value(route.request_options.unwrap()).unwrap();
    let mut request = serde_json::json!({});
    options.apply(&mut request, "custom-model", 4096, "provider-default");
    assert_eq!(request["max_completion_tokens"], 4096);
    assert_eq!(request["chat_template_kwargs"]["enable_thinking"], false);
    assert_eq!(request["temperature"], serde_json::json!(0.4f32));
    snapshot.connections[1].adapter_kind = AdapterKind::AgentSession;
    assert!(validate_snapshot(&snapshot).is_err());
    assert!(resolve_route(&snapshot, Purpose::ConversationRespond, Default::default()).is_err());
    snapshot.connections[1].adapter_kind = AdapterKind::Larm;
    assert!(validate_snapshot(&snapshot).is_err());
}

fn harness_id(capability: Capability) -> String {
    migrate_legacy(&providers(), &routing("harness"))
        .unwrap()
        .resources
        .into_iter()
        .find(|r| r.connection_id == "conn:harness" && r.capability == capability)
        .unwrap()
        .resource_id
}

fn failover_snapshot(purpose: Purpose) -> RegistrySnapshot {
    let mut snapshot = migrate_legacy(&providers(), &routing("harness")).unwrap();
    let capability = purpose.required_capability();
    let adapter_kind = match capability {
        Capability::TextGeneration => AdapterKind::ChatCompletions,
        Capability::Transcription => AdapterKind::HttpAsr,
        Capability::Speech => AdapterKind::HttpTts,
        _ => AdapterKind::ReplicateMedia,
    };
    snapshot.connections.push(ServiceConnection {
        connection_id: "conn:away".into(),
        label: "away".into(),
        adapter_kind,
        endpoint: "https://api.example.test/v1".into(),
        location: "cloud".into(),
        authentication: "none".into(),
        credential_ref: None,
        enabled: true,
    });
    snapshot.resources.push(ServiceResource {
        resource_id: "res:away".into(),
        connection_id: "conn:away".into(),
        capability,
        model: "owner/model".into(),
        detail: None,
        request_options: None,
        enabled: true,
    });
    let binding = snapshot
        .bindings
        .iter_mut()
        .find(|b| b.purpose == purpose)
        .unwrap();
    binding.enabled = true;
    binding.review = BindingReview::Ready;
    binding.cloud_allowed = true;
    binding.primary_resource_id = Some(harness_id(capability));
    binding.fallback_resource_ids = vec!["res:away".into()];
    snapshot
}

const FAILOVER_PURPOSES: [Purpose; 5] = [
    Purpose::ConversationRespond,
    Purpose::VoiceTranscribe,
    Purpose::VoiceSpeak,
    Purpose::MediaImageGenerate,
    Purpose::MediaMusicGenerate,
];

fn resource_mut<'a>(snapshot: &'a mut RegistrySnapshot, id: &str) -> &'a mut ServiceResource {
    snapshot
        .resources
        .iter_mut()
        .find(|resource| resource.resource_id == id)
        .expect("test resource")
}

fn larm(reachability: Reachability) -> LocalAvailability {
    LocalAvailability { larm: reachability }
}

#[test]
fn every_purpose_uses_larm_at_home_and_the_fallback_when_larm_is_unreachable() {
    for purpose in FAILOVER_PURPOSES {
        let snapshot = failover_snapshot(purpose);
        validate_snapshot(&snapshot).unwrap_or_else(|e| panic!("{purpose:?}: {e}"));
        for home in [Reachability::Reachable, Reachability::Unknown] {
            let route = resolve_route(&snapshot, purpose, larm(home)).unwrap();
            assert_eq!(route.adapter_kind, AdapterKind::Larm, "{purpose:?}");
            assert_eq!(route.selection, RouteSelection::Primary);
        }
        let away = resolve_route(&snapshot, purpose, larm(Reachability::Unreachable)).unwrap();
        assert_eq!(away.resource_id, "res:away", "{purpose:?}");
        assert_eq!(away.location, "cloud");
        assert_eq!(away.selection, RouteSelection::LocalUnreachable);
    }
}

#[test]
fn unreachable_larm_without_an_allowed_fallback_is_an_explicit_error() {
    for purpose in FAILOVER_PURPOSES {
        let mut none = failover_snapshot(purpose);
        none.bindings
            .iter_mut()
            .find(|b| b.purpose == purpose)
            .unwrap()
            .fallback_resource_ids
            .clear();
        assert_eq!(
            resolve_route(&none, purpose, larm(Reachability::Unreachable)),
            Err(ResolveError::LocalUnreachable {
                purpose,
                cloud_blocked: false
            })
        );
        let mut blocked = failover_snapshot(purpose);
        blocked
            .bindings
            .iter_mut()
            .find(|b| b.purpose == purpose)
            .unwrap()
            .cloud_allowed = false;
        assert_eq!(
            resolve_route(&blocked, purpose, larm(Reachability::Unreachable)),
            Err(ResolveError::LocalUnreachable {
                purpose,
                cloud_blocked: true
            })
        );
        // At home the cloud fallback is simply unused, so the same setting keeps working.
        assert!(resolve_route(&blocked, purpose, larm(Reachability::Reachable)).is_ok());
    }
}

#[test]
fn a_cloud_primary_ignores_larm_reachability_and_a_broken_primary_never_falls_back() {
    let mut snapshot = failover_snapshot(Purpose::VoiceTranscribe);
    snapshot
        .bindings
        .iter_mut()
        .find(|b| b.purpose == Purpose::VoiceTranscribe)
        .unwrap()
        .primary_resource_id = Some("res:away".into());
    snapshot
        .bindings
        .iter_mut()
        .find(|b| b.purpose == Purpose::VoiceTranscribe)
        .unwrap()
        .fallback_resource_ids
        .clear();
    let route = resolve_route(
        &snapshot,
        Purpose::VoiceTranscribe,
        larm(Reachability::Unreachable),
    )
    .unwrap();
    assert_eq!(route.selection, RouteSelection::Primary);

    let mut disabled = failover_snapshot(Purpose::VoiceTranscribe);
    resource_mut(&mut disabled, "res:harness-asr").enabled = false;
    assert_eq!(
        resolve_route(
            &disabled,
            Purpose::VoiceTranscribe,
            larm(Reachability::Unreachable)
        ),
        Err(ResolveError::ResourceDisabled("res:harness-asr".into()))
    );
}

#[test]
fn larm_can_only_be_a_primary_and_a_stored_route_without_selection_reads_as_primary() {
    let mut snapshot = failover_snapshot(Purpose::VoiceSpeak);
    let binding = snapshot
        .bindings
        .iter_mut()
        .find(|b| b.purpose == Purpose::VoiceSpeak)
        .unwrap();
    binding.primary_resource_id = Some("res:away".into());
    binding.fallback_resource_ids = vec![harness_id(Capability::Speech)];
    assert!(validate_snapshot(&snapshot).is_err());

    let route = resolve_route(
        &failover_snapshot(Purpose::MediaImageGenerate),
        Purpose::MediaImageGenerate,
        Default::default(),
    )
    .unwrap();
    let mut stored = serde_json::to_value(&route).unwrap();
    stored.as_object_mut().unwrap().remove("selection");
    let restored: ResolvedRoute = serde_json::from_value(stored).unwrap();
    assert_eq!(restored.selection, RouteSelection::Primary);
}

#[test]
fn a_binding_awaiting_review_may_keep_legacy_larm_fallbacks_without_breaking_load() {
    let mut snapshot = failover_snapshot(Purpose::ConversationRespond);
    let binding = snapshot
        .bindings
        .iter_mut()
        .find(|b| b.purpose == Purpose::ConversationRespond)
        .unwrap();
    binding.primary_resource_id = Some("res:away".into());
    binding.fallback_resource_ids = vec![harness_id(Capability::TextGeneration)];
    binding.review = BindingReview::NeedsReview;
    assert!(validate_snapshot(&snapshot).is_ok());
}

#[test]
fn local_availability_matches_domain_reachability() {
    let snapshot = migrate_legacy(&providers(), &routing("harness")).unwrap();
    for (local, domain) in [
        (Reachability::Unknown, LarmReachability::Unknown),
        (Reachability::Reachable, LarmReachability::Reachable),
        (Reachability::Unreachable, LarmReachability::Unreachable),
    ] {
        let desktop = resolve_route(
            &snapshot,
            Purpose::VoiceSpeak,
            LocalAvailability { larm: local },
        );
        let direct = saaa_provider_routing::resolve_route(&snapshot, Purpose::VoiceSpeak, domain);
        assert_eq!(desktop, direct);
    }
}

#[test]
fn desktop_settings_conversion_matches_an_independent_legacy_fixture() {
    use saaa_provider_routing::{LegacyHarness, LegacyProvider, LegacyRoute, LegacySettings};
    let desktop = migrate_legacy(&providers(), &routing("harness")).unwrap();
    let domain = saaa_provider_routing::migrate_legacy(&LegacySettings {
        harness: LegacyHarness {
            address: "http://larm.test".into(),
            tts_voice: None,
        },
        providers: vec![
            LegacyProvider {
                id: "cloud-llm".into(),
                label: "cloud-llm".into(),
                location: "cloud".into(),
                enabled: true,
                adapter_kind: AdapterKind::ChatCompletions,
                capability: Capability::TextGeneration,
                endpoint: "https://api.example.test/v1".into(),
                authentication: "api-key".into(),
                model: "m".into(),
                detail: None,
                request_options: None,
            },
            LegacyProvider {
                id: "cloud-asr".into(),
                label: "cloud-asr".into(),
                location: "cloud".into(),
                enabled: true,
                adapter_kind: AdapterKind::HttpAsr,
                capability: Capability::Transcription,
                endpoint: "https://api.example.test/v1".into(),
                authentication: "api-key".into(),
                model: "w".into(),
                detail: Some("ja".into()),
                request_options: None,
            },
            LegacyProvider {
                id: "cloud-tts".into(),
                label: "cloud-tts".into(),
                location: "cloud".into(),
                enabled: true,
                adapter_kind: AdapterKind::HttpTts,
                capability: Capability::Speech,
                endpoint: "https://api.example.test/v1".into(),
                authentication: "none".into(),
                model: "t".into(),
                detail: Some("v".into()),
                request_options: None,
            },
        ],
        conversation_respond: LegacyRoute {
            source: "harness".into(),
            primary_provider_id: None,
            provider_id: None,
            fallback_provider_ids: vec![],
            timeout_ms: 60_000,
            attempt_timeout_ms: None,
        },
        voice_transcribe: LegacyRoute {
            source: "provider".into(),
            primary_provider_id: None,
            provider_id: Some("cloud-asr".into()),
            fallback_provider_ids: vec![],
            timeout_ms: 30_000,
            attempt_timeout_ms: None,
        },
        voice_speak: LegacyRoute {
            source: "harness".into(),
            primary_provider_id: None,
            provider_id: None,
            fallback_provider_ids: vec![],
            timeout_ms: 30_000,
            attempt_timeout_ms: None,
        },
    })
    .unwrap();
    assert_eq!(
        serde_json::to_value(&desktop).unwrap(),
        serde_json::to_value(&domain).unwrap()
    );
    assert_eq!(desktop.connections.len(), 4);
    assert!(domain
        .connection("conn:cloud-tts")
        .unwrap()
        .credential_ref
        .is_none());
}
