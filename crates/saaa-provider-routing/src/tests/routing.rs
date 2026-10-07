use crate::{
    resolve_route, validate_snapshot, AdapterKind, BindingReview, Capability, CredentialRef,
    LarmReachability, Purpose, PurposeBinding, RegistrySnapshot, ResolveError, RouteSelection,
    ServiceConnection, ServiceResource, PROVIDER_CREDENTIAL_SERVICE,
};

fn connection(
    id: &str,
    adapter: AdapterKind,
    location: &str,
    authentication: &str,
    account: Option<&str>,
) -> ServiceConnection {
    ServiceConnection {
        connection_id: id.into(),
        label: id.into(),
        adapter_kind: adapter,
        endpoint: if adapter == AdapterKind::Larm {
            "http://larm.test".into()
        } else {
            "https://api.example.test/v1".into()
        },
        location: location.into(),
        authentication: authentication.into(),
        credential_ref: account.map(|account| CredentialRef {
            service: PROVIDER_CREDENTIAL_SERVICE.into(),
            account: account.into(),
        }),
        enabled: true,
    }
}

fn resource(id: &str, connection_id: &str, capability: Capability, model: &str) -> ServiceResource {
    ServiceResource {
        resource_id: id.into(),
        connection_id: connection_id.into(),
        capability,
        model: model.into(),
        detail: None,
        request_options: None,
        enabled: true,
    }
}

fn binding(purpose: Purpose, primary: &str, timeout_ms: u64, cloud_allowed: bool) -> PurposeBinding {
    PurposeBinding {
        purpose,
        enabled: true,
        primary_resource_id: Some(primary.into()),
        fallback_resource_ids: Vec::new(),
        cloud_allowed,
        timeout_ms,
        attempt_timeout_ms: None,
        stored_primary_resource_id: None,
        review: BindingReview::Ready,
    }
}

fn harness_resource(capability: Capability) -> String {
    let suffix = match capability {
        Capability::TextGeneration => "llm",
        Capability::Transcription => "asr",
        Capability::Speech => "tts",
        Capability::ImageGeneration => "image",
        Capability::MusicGeneration => "music",
    };
    format!("res:harness-{suffix}")
}

fn base_snapshot() -> RegistrySnapshot {
    RegistrySnapshot {
        connections: vec![
            connection("conn:harness", AdapterKind::Larm, "local", "none", None),
            connection(
                "conn:cloud-llm",
                AdapterKind::ChatCompletions,
                "cloud",
                "api-key",
                Some("cloud-llm"),
            ),
            connection(
                "conn:cloud-asr",
                AdapterKind::HttpAsr,
                "cloud",
                "api-key",
                Some("cloud-asr"),
            ),
            connection(
                "conn:cloud-tts",
                AdapterKind::HttpTts,
                "cloud",
                "none",
                None,
            ),
        ],
        resources: vec![
            resource(
                "res:harness-llm",
                "conn:harness",
                Capability::TextGeneration,
                "",
            ),
            resource(
                "res:harness-asr",
                "conn:harness",
                Capability::Transcription,
                "",
            ),
            resource("res:harness-tts", "conn:harness", Capability::Speech, ""),
            resource(
                "res:harness-image",
                "conn:harness",
                Capability::ImageGeneration,
                "",
            ),
            resource(
                "res:harness-music",
                "conn:harness",
                Capability::MusicGeneration,
                "",
            ),
            resource(
                "res:cloud-llm",
                "conn:cloud-llm",
                Capability::TextGeneration,
                "m",
            ),
            resource(
                "res:cloud-asr",
                "conn:cloud-asr",
                Capability::Transcription,
                "w",
            ),
            resource("res:cloud-tts", "conn:cloud-tts", Capability::Speech, "t"),
        ],
        bindings: vec![
            binding(
                Purpose::ConversationRespond,
                "res:harness-llm",
                60_000,
                true,
            ),
            binding(Purpose::VoiceTranscribe, "res:cloud-asr", 30_000, true),
            binding(Purpose::VoiceSpeak, "res:harness-tts", 30_000, true),
            binding(
                Purpose::MediaImageGenerate,
                "res:harness-image",
                1_800_000,
                false,
            ),
            binding(
                Purpose::MediaMusicGenerate,
                "res:harness-music",
                1_800_000,
                false,
            ),
        ],
    }
}

fn resource_mut<'a>(snapshot: &'a mut RegistrySnapshot, id: &str) -> &'a mut ServiceResource {
    snapshot
        .resources
        .iter_mut()
        .find(|resource| resource.resource_id == id)
        .expect("test resource")
}

fn away_adapter(capability: Capability) -> AdapterKind {
    match capability {
        Capability::TextGeneration => AdapterKind::ChatCompletions,
        Capability::Transcription => AdapterKind::HttpAsr,
        Capability::Speech => AdapterKind::HttpTts,
        Capability::ImageGeneration | Capability::MusicGeneration => AdapterKind::ReplicateMedia,
    }
}

fn failover_snapshot(purpose: Purpose) -> RegistrySnapshot {
    let mut snapshot = base_snapshot();
    let capability = purpose.required_capability();
    snapshot.connections.push(connection(
        "conn:away",
        away_adapter(capability),
        "cloud",
        "none",
        None,
    ));
    snapshot.resources.push(resource(
        "res:away",
        "conn:away",
        capability,
        "owner/model",
    ));
    let binding = snapshot
        .bindings
        .iter_mut()
        .find(|binding| binding.purpose == purpose)
        .unwrap();
    binding.enabled = true;
    binding.review = BindingReview::Ready;
    binding.cloud_allowed = true;
    binding.primary_resource_id = Some(harness_resource(capability));
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

#[test]
fn fingerprint_changes_with_the_resource_but_not_unrelated_resources() {
    let mut snapshot = base_snapshot();
    let before = resolve_route(
        &snapshot,
        Purpose::VoiceTranscribe,
        LarmReachability::Unknown,
    )
    .unwrap();
    resource_mut(&mut snapshot, "res:cloud-tts").model = "other".into();
    assert_eq!(
        before.fingerprint,
        resolve_route(
            &snapshot,
            Purpose::VoiceTranscribe,
            LarmReachability::Unknown
        )
        .unwrap()
        .fingerprint
    );
    resource_mut(&mut snapshot, "res:cloud-asr").model = "other".into();
    assert_ne!(
        before.fingerprint,
        resolve_route(
            &snapshot,
            Purpose::VoiceTranscribe,
            LarmReachability::Unknown
        )
        .unwrap()
        .fingerprint
    );
}

#[test]
fn disabled_resource_is_rejected_without_falling_back() {
    let mut snapshot = base_snapshot();
    resource_mut(&mut snapshot, "res:cloud-asr").enabled = false;
    assert_eq!(
        resolve_route(
            &snapshot,
            Purpose::VoiceTranscribe,
            LarmReachability::Unknown
        ),
        Err(ResolveError::ResourceDisabled("res:cloud-asr".into()))
    );
}

#[test]
fn validation_rejects_capability_mismatch_duplicates_and_unknown_ids() {
    let base = base_snapshot();
    let mut wrong = base.clone();
    wrong.bindings[1].primary_resource_id = Some("res:cloud-tts".into());
    assert!(validate_snapshot(&wrong).is_err());
    let mut duplicate = base.clone();
    duplicate.bindings[1].fallback_resource_ids = vec!["res:cloud-asr".into()];
    assert!(validate_snapshot(&duplicate).is_err());
    let mut unknown = base;
    unknown.bindings[1].primary_resource_id = Some("res:missing".into());
    assert!(validate_snapshot(&unknown).is_err());
}

#[test]
fn every_purpose_uses_larm_at_home_and_the_fallback_when_larm_is_unreachable() {
    for purpose in FAILOVER_PURPOSES {
        let snapshot = failover_snapshot(purpose);
        validate_snapshot(&snapshot).unwrap_or_else(|error| panic!("{purpose:?}: {error}"));
        for home in [LarmReachability::Reachable, LarmReachability::Unknown] {
            let route = resolve_route(&snapshot, purpose, home).unwrap();
            assert_eq!(route.adapter_kind, AdapterKind::Larm, "{purpose:?}");
            assert_eq!(route.selection, RouteSelection::Primary);
        }
        let away = resolve_route(&snapshot, purpose, LarmReachability::Unreachable).unwrap();
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
            .find(|binding| binding.purpose == purpose)
            .unwrap()
            .fallback_resource_ids
            .clear();
        assert_eq!(
            resolve_route(&none, purpose, LarmReachability::Unreachable),
            Err(ResolveError::LocalUnreachable {
                purpose,
                cloud_blocked: false
            })
        );
        let mut blocked = failover_snapshot(purpose);
        blocked
            .bindings
            .iter_mut()
            .find(|binding| binding.purpose == purpose)
            .unwrap()
            .cloud_allowed = false;
        assert_eq!(
            resolve_route(&blocked, purpose, LarmReachability::Unreachable),
            Err(ResolveError::LocalUnreachable {
                purpose,
                cloud_blocked: true
            })
        );
        assert!(resolve_route(&blocked, purpose, LarmReachability::Reachable).is_ok());
    }
}

#[test]
fn a_cloud_primary_ignores_larm_reachability_and_a_broken_primary_never_falls_back() {
    let mut snapshot = failover_snapshot(Purpose::VoiceTranscribe);
    let binding = snapshot
        .bindings
        .iter_mut()
        .find(|binding| binding.purpose == Purpose::VoiceTranscribe)
        .unwrap();
    binding.primary_resource_id = Some("res:away".into());
    binding.fallback_resource_ids.clear();
    let route = resolve_route(
        &snapshot,
        Purpose::VoiceTranscribe,
        LarmReachability::Unreachable,
    )
    .unwrap();
    assert_eq!(route.selection, RouteSelection::Primary);

    let mut disabled = failover_snapshot(Purpose::VoiceTranscribe);
    resource_mut(&mut disabled, "res:harness-asr").enabled = false;
    assert_eq!(
        resolve_route(
            &disabled,
            Purpose::VoiceTranscribe,
            LarmReachability::Unreachable
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
        .find(|binding| binding.purpose == Purpose::VoiceSpeak)
        .unwrap();
    binding.primary_resource_id = Some("res:away".into());
    binding.fallback_resource_ids = vec![harness_resource(Capability::Speech)];
    assert!(validate_snapshot(&snapshot).is_err());

    let route = resolve_route(
        &failover_snapshot(Purpose::MediaImageGenerate),
        Purpose::MediaImageGenerate,
        LarmReachability::Unknown,
    )
    .unwrap();
    let mut stored = serde_json::to_value(&route).unwrap();
    stored.as_object_mut().unwrap().remove("selection");
    let restored: crate::ResolvedRoute = serde_json::from_value(stored).unwrap();
    assert_eq!(restored.selection, RouteSelection::Primary);
}

#[test]
fn a_binding_awaiting_review_may_keep_legacy_larm_fallbacks_without_breaking_load() {
    let mut snapshot = failover_snapshot(Purpose::ConversationRespond);
    let binding = snapshot
        .bindings
        .iter_mut()
        .find(|binding| binding.purpose == Purpose::ConversationRespond)
        .unwrap();
    binding.primary_resource_id = Some("res:away".into());
    binding.fallback_resource_ids = vec![harness_resource(Capability::TextGeneration)];
    binding.review = BindingReview::NeedsReview;
    assert!(validate_snapshot(&snapshot).is_ok());
}
