use super::*;
use crate::situation::contracts::{
    CalendarState, ForegroundCategory, ForegroundSignal, InputActivitySignal, InputActivityState,
    TimeBucket,
};
pub(super) fn signals(category: ForegroundCategory) -> SignalSnapshot {
    let now = crate::now_iso();
    SignalSnapshot {
        sequence: 1,
        observed_at: now,
        foreground: ForegroundSignal {
            category,
            health: SignalHealth::Ready,
        },
        conversation: ConversationSignal {
            state: ConversationState::Idle,
        },
        microphone: MicrophoneSignal {
            state: MicrophoneState::Inactive,
            health: SignalHealth::Ready,
        },
        audio: AudioSignal {
            state: AudioState::Silent,
            health: SignalHealth::Ready,
        },
        calendar: CalendarSignal {
            state: CalendarState::Free,
            time_bucket: TimeBucket::None,
            health: SignalHealth::Ready,
        },
        input_activity: InputActivitySignal {
            state: InputActivityState::Unknown,
            health: SignalHealth::Unsupported,
        },
    }
}
#[test]
pub(super) fn classifier_is_deterministic_and_sensitive_is_safe() {
    let coding = signals(ForegroundCategory::Coding);
    assert_eq!(classify(&coding).scene, "CODING");
    assert_eq!(classify(&coding).scene, classify(&coding).scene);
    let sensitive = signals(ForegroundCategory::Sensitive);
    let candidate = classify(&sensitive);
    assert_eq!(candidate.scene, "UNKNOWN");
    let state = initial_state(&crate::now_iso());
    let decision = shadow_policy(
        &state,
        &sensitive,
        &crate::now_iso(),
        &CalibrationParameters::default(),
    );
    assert_eq!(decision.proposed_attention, "IGNORE");
    assert_eq!(decision.actual_execution, "NONE");
    assert_eq!(decision.actual_presentation, "SILENT");
}
#[test]
pub(super) fn hysteresis_rejects_short_noise_and_accepts_three_fresh_samples() {
    let now = crate::now_iso();
    let mut hysteresis = Hysteresis::new(&now);
    let candidate = classify(&signals(ForegroundCategory::Coding));
    assert!(!hysteresis.update(&candidate, &now, 11_000).1);
    assert!(!hysteresis.update(&candidate, &now, 13_000).1);
    let (stable, changed) = hysteresis.update(&candidate, &now, 15_000);
    assert!(changed);
    assert_eq!(stable.scene, "CODING");

    let noise = classify(&signals(ForegroundCategory::Unknown));
    let (stable, changed) = hysteresis.update(&noise, &now, 16_000);
    assert!(!changed);
    assert_eq!(stable.scene, "CODING");
}
#[test]
pub(super) fn all_shadow_policy_paths_are_non_intervening() {
    for category in [
        ForegroundCategory::Coding,
        ForegroundCategory::Communication,
        ForegroundCategory::Sensitive,
        ForegroundCategory::Unknown,
    ] {
        let snapshot = signals(category);
        let candidate = classify(&snapshot);
        let state = SituationState {
            scene: candidate.scene,
            confidence: candidate.confidence,
            user_attention: candidate.user_attention,
            audio_environment: candidate.audio_environment,
            evidence: candidate.evidence,
            candidate_since: crate::now_iso(),
            stable_since: crate::now_iso(),
            updated_at: crate::now_iso(),
            rule_version: contracts::RULE_VERSION.to_string(),
        };
        let decision = shadow_policy(
            &state,
            &snapshot,
            &crate::now_iso(),
            &CalibrationParameters::default(),
        );
        assert_eq!(decision.actual_execution, "NONE");
        assert_eq!(decision.actual_presentation, "SILENT");
    }
}
#[test]
pub(super) fn shadow_policy_exposes_all_four_counterfactual_attention_decisions() {
    let now = crate::now_iso();
    let mut snapshot = signals(ForegroundCategory::Coding);
    let mut state = SituationState {
        scene: "CODING".to_string(),
        confidence: 80,
        user_attention: "available".to_string(),
        audio_environment: "silence".to_string(),
        evidence: vec![],
        candidate_since: now.clone(),
        stable_since: now.clone(),
        updated_at: now.clone(),
        rule_version: contracts::RULE_VERSION.to_string(),
    };
    assert_eq!(
        shadow_policy(&state, &snapshot, &now, &CalibrationParameters::default(),)
            .proposed_attention,
        "SUGGEST"
    );

    state.scene = "MEETING".to_string();
    state.user_attention = "busy".to_string();
    assert_eq!(
        shadow_policy(&state, &snapshot, &now, &CalibrationParameters::default(),)
            .proposed_attention,
        "OBSERVE"
    );

    snapshot.conversation.state = ConversationState::UserInput;
    assert_eq!(
        shadow_policy(&state, &snapshot, &now, &CalibrationParameters::default(),)
            .proposed_attention,
        "RESPOND"
    );

    snapshot.conversation.state = ConversationState::Idle;
    snapshot.foreground.category = ForegroundCategory::Sensitive;
    assert_eq!(
        shadow_policy(&state, &snapshot, &now, &CalibrationParameters::default(),)
            .proposed_attention,
        "IGNORE"
    );
}
#[test]
pub(super) fn idle_activity_suppresses_only_suggestions_and_never_explicit_or_sensitive_safety() {
    let now = crate::now_iso();
    let parameters = CalibrationParameters::default();
    let mut snapshot = signals(ForegroundCategory::Coding);
    let baseline = classify(&snapshot);
    snapshot.input_activity = InputActivitySignal {
        state: InputActivityState::Idle,
        health: SignalHealth::Ready,
    };
    let idle = classify(&snapshot);
    assert_eq!(idle.scene, baseline.scene);
    assert_eq!(idle.confidence, baseline.confidence);
    assert_eq!(idle.user_attention, baseline.user_attention);
    assert_eq!(idle.audio_environment, baseline.audio_environment);
    assert_eq!(idle.evidence, baseline.evidence);
    let state = SituationState {
        scene: idle.scene,
        confidence: idle.confidence,
        user_attention: idle.user_attention,
        audio_environment: idle.audio_environment,
        evidence: idle.evidence,
        candidate_since: now.clone(),
        stable_since: now.clone(),
        updated_at: now.clone(),
        rule_version: contracts::RULE_VERSION.to_string(),
    };
    let decision = shadow_policy(&state, &snapshot, &now, &parameters);
    assert_eq!(decision.proposed_attention, "OBSERVE");
    assert_eq!(decision.reason_codes, ["input-idle"]);

    let mut busy_state = state.clone();
    busy_state.scene = "MEETING".to_string();
    busy_state.user_attention = "busy".to_string();
    let busy_decision = shadow_policy(&busy_state, &snapshot, &now, &parameters);
    assert_eq!(busy_decision.proposed_attention, "OBSERVE");
    assert_eq!(busy_decision.reason_codes, ["user-busy"]);

    let mut passive_state = state.clone();
    passive_state.confidence = parameters.classification_min_confidence - 1;
    let passive_decision = shadow_policy(&passive_state, &snapshot, &now, &parameters);
    assert_eq!(passive_decision.proposed_attention, "OBSERVE");
    assert_eq!(passive_decision.reason_codes, ["passive-observation"]);

    snapshot.conversation.state = ConversationState::UserInput;
    assert_eq!(
        shadow_policy(&state, &snapshot, &now, &parameters).proposed_attention,
        "RESPOND"
    );
    snapshot.conversation.state = ConversationState::Idle;
    snapshot.foreground.category = ForegroundCategory::Sensitive;
    assert_eq!(
        shadow_policy(&state, &snapshot, &now, &parameters).proposed_attention,
        "IGNORE"
    );
}
#[test]
pub(super) fn idle_unknown_and_non_ready_activity_preserve_existing_policy() {
    let now = crate::now_iso();
    let parameters = CalibrationParameters::default();
    let mut snapshot = signals(ForegroundCategory::Unknown);
    snapshot.input_activity = InputActivitySignal {
        state: InputActivityState::Idle,
        health: SignalHealth::Ready,
    };
    let unknown = classify(&snapshot);
    let unknown_state = SituationState {
        scene: unknown.scene,
        confidence: unknown.confidence,
        user_attention: unknown.user_attention,
        audio_environment: unknown.audio_environment,
        evidence: unknown.evidence,
        candidate_since: now.clone(),
        stable_since: now.clone(),
        updated_at: now.clone(),
        rule_version: contracts::RULE_VERSION.to_string(),
    };
    assert_eq!(
        shadow_policy(&unknown_state, &snapshot, &now, &parameters).proposed_attention,
        "IGNORE"
    );

    snapshot.foreground.category = ForegroundCategory::Coding;
    let coding = classify(&snapshot);
    let coding_state = SituationState {
        scene: coding.scene,
        confidence: coding.confidence,
        user_attention: coding.user_attention,
        audio_environment: coding.audio_environment,
        evidence: coding.evidence,
        candidate_since: now.clone(),
        stable_since: now.clone(),
        updated_at: now.clone(),
        rule_version: contracts::RULE_VERSION.to_string(),
    };
    for health in [SignalHealth::Degraded, SignalHealth::Unsupported] {
        snapshot.input_activity.health = health;
        assert_eq!(
            shadow_policy(&coding_state, &snapshot, &now, &parameters).proposed_attention,
            "SUGGEST"
        );
    }
}
#[test]
pub(super) fn disabled_runtime_returns_disabled_signals_without_platform_sampling() {
    let runtime = SituationRuntime::new(SituationRuntimeSettings::default(), None)
        .expect("runtime initializes");
    let sample = runtime.sample_platform().expect("disabled sample projects");
    assert_eq!(sample.foreground.health, SignalHealth::Disabled);
    assert_eq!(sample.calendar.health, SignalHealth::Disabled);
    assert_eq!(sample.input_activity.health, SignalHealth::Disabled);
    assert_eq!(sample.input_activity.state, InputActivityState::Unknown);
}
#[test]
pub(super) fn input_activity_health_change_is_emitted_once() {
    let connection =
        SqliteWriter::from_connection(Connection::open_in_memory().expect("database opens"));
    crate::initialize_database(&connection.lock().expect("database lock"))
        .expect("database initializes");
    let runtime = SituationRuntime::new(
        SituationRuntimeSettings {
            enabled: true,
            ..SituationRuntimeSettings::default()
        },
        None,
    )
    .expect("runtime initializes");
    let snapshot = signals(ForegroundCategory::Coding);
    for observed_ms in [1_000_u128, 3_000] {
        runtime
            .tick_sampled(
                &connection,
                SituationSample {
                    foreground: snapshot.foreground.clone(),
                    calendar: snapshot.calendar.clone(),
                    input_activity: InputActivitySignal {
                        state: InputActivityState::Active,
                        health: SignalHealth::Ready,
                    },
                    observed_at: observed_ms.to_string(),
                    observed_ms,
                },
            )
            .expect("sample succeeds");
    }
    let inner = runtime.inner.lock().expect("runtime lock");
    let changes = inner
        .events
        .iter()
        .filter(|(_, event)| {
            matches!(
                event,
                SituationEvent::SignalHealthChanged { source, .. }
                    if source == "input-activity"
            )
        })
        .count();
    assert_eq!(changes, 1);
}
#[test]
pub(super) fn denied_optional_signal_does_not_block_available_local_signal() {
    let mut snapshot = signals(ForegroundCategory::Coding);
    snapshot.calendar = CalendarSignal {
        state: CalendarState::Unavailable,
        time_bucket: TimeBucket::None,
        health: SignalHealth::PermissionDenied,
    };
    assert_eq!(classify(&snapshot).scene, "CODING");
}
#[test]
pub(super) fn owned_lifecycle_flows_through_runtime_to_ledger_and_pause_stops_writes() {
    let connection =
        SqliteWriter::from_connection(Connection::open_in_memory().expect("database opens"));
    crate::initialize_database(&connection.lock().expect("database lock"))
        .expect("database initializes");
    let settings = SituationRuntimeSettings {
        enabled: true,
        ..SituationRuntimeSettings::default()
    };
    let runtime = SituationRuntime::new(settings.clone(), None).expect("runtime initializes");
    runtime
        .report_owned(OwnedSignalInput {
            conversation_state: ConversationState::AgentRunning,
            microphone_state: MicrophoneState::Inactive,
            audio_state: AudioState::Silent,
        })
        .expect("owned signal reports");
    runtime.tick(&connection).expect("runtime ticks");
    let snapshot = runtime
        .snapshot(&connection.lock().expect("database lock"))
        .expect("snapshot loads");
    assert_eq!(snapshot.state.scene, "CONVERSATION");
    assert_eq!(snapshot.decision.proposed_attention, "RESPOND");
    assert_eq!(snapshot.decision.actual_execution, "NONE");
    assert_eq!(snapshot.decision.actual_presentation, "SILENT");
    assert_eq!(snapshot.history.len(), 1);

    runtime
        .configure(SituationRuntimeSettings::default())
        .expect("runtime pauses");
    runtime.tick(&connection).expect("paused tick is harmless");
    let paused = runtime
        .snapshot(&connection.lock().expect("database lock"))
        .expect("paused snapshot loads");
    assert_eq!(paused.history.len(), 1);
    assert!(!paused.monitoring_enabled);
}
#[test]
pub(super) fn failed_ledger_write_does_not_advance_runtime_state() {
    let connection =
        SqliteWriter::from_connection(Connection::open_in_memory().expect("database opens"));
    crate::initialize_database(&connection.lock().expect("database lock"))
        .expect("database initializes");
    let runtime = SituationRuntime::new(
        SituationRuntimeSettings {
            enabled: true,
            ..SituationRuntimeSettings::default()
        },
        None,
    )
    .expect("runtime initializes");
    runtime
        .report_owned(OwnedSignalInput {
            conversation_state: ConversationState::AgentRunning,
            microphone_state: MicrophoneState::Inactive,
            audio_state: AudioState::Silent,
        })
        .expect("owned signal reports");
    connection
        .lock()
        .expect("database lock")
        .execute("DROP TABLE situation_ledger", [])
        .expect("fixture removes ledger");

    assert!(runtime.tick(&connection).is_err());
    let inner = runtime.inner.lock().expect("runtime lock");
    assert_eq!(inner.signals.sequence, 0);
    assert_eq!(inner.state.scene, "UNKNOWN");
    assert_eq!(inner.last_persisted_ms, 0);
}
#[test]
pub(super) fn failed_quality_window_is_bounded_to_two_intervals() {
    let connection =
        SqliteWriter::from_connection(Connection::open_in_memory().expect("database opens"));
    crate::initialize_database(&connection.lock().expect("database lock"))
        .expect("database initializes");
    connection
        .lock()
        .expect("database lock")
        .execute("DROP TABLE situation_ledger", [])
        .expect("fixture removes ledger");
    let runtime = SituationRuntime::new(
        SituationRuntimeSettings {
            enabled: true,
            heartbeat_interval_ms: 60_000,
            ..SituationRuntimeSettings::default()
        },
        None,
    )
    .expect("runtime initializes");
    let snapshot = signals(ForegroundCategory::Coding);
    let sample = |observed_ms: u128| SituationSample {
        foreground: snapshot.foreground.clone(),
        calendar: snapshot.calendar.clone(),
        input_activity: snapshot.input_activity.clone(),
        observed_at: observed_ms.to_string(),
        observed_ms,
    };

    assert!(runtime.tick_sampled(&connection, sample(1_000)).is_err());
    {
        let inner = runtime.inner.lock().expect("runtime lock");
        assert_eq!(inner.quality.sample_count, 1);
        assert_eq!(inner.quality_started_ms, 1_000);
    }
    assert!(runtime.tick_sampled(&connection, sample(121_000)).is_err());
    let inner = runtime.inner.lock().expect("runtime lock");
    assert_eq!(inner.quality.sample_count, 0);
    assert_eq!(inner.quality_started_ms, 0);
}
#[test]
pub(super) fn runtime_rejects_invalid_sampling_configuration() {
    let invalid = SituationRuntimeSettings {
        sample_interval_ms: 0,
        ..SituationRuntimeSettings::default()
    };
    assert!(SituationRuntime::new(invalid, None).is_err());
}
#[test]
pub(super) fn optional_calendar_degrades_without_exposing_details() {
    let calendar = platform::calendar_signal(true);
    assert_eq!(calendar.state, CalendarState::Unavailable);
    assert_eq!(calendar.health, SignalHealth::Unsupported);
    assert_eq!(calendar.time_bucket, TimeBucket::None);
}
#[test]
pub(super) fn stale_owned_signal_falls_back_to_idle() {
    let active = OwnedSignalInput {
        conversation_state: ConversationState::AgentRunning,
        microphone_state: MicrophoneState::SaaaCapturing,
        audio_state: AudioState::SaaaSpeaking,
    };
    assert_eq!(fresh_owned(&active, 1_000, 6_999, 2_000), active);
    let stale = fresh_owned(&active, 1_000, 7_001, 2_000);
    assert_eq!(stale.conversation_state, ConversationState::Idle);
    assert_eq!(stale.microphone_state, MicrophoneState::Inactive);
    assert_eq!(stale.audio_state, AudioState::Silent);
}
#[test]
pub(super) fn eight_hour_fixture_replay_and_event_queue_remain_bounded() {
    let now = crate::now_iso();
    let mut hysteresis = Hysteresis::new(&now);
    let coding = classify(&signals(ForegroundCategory::Coding));
    let noise = classify(&signals(ForegroundCategory::Unknown));
    for sample in 0..14_400_u128 {
        let candidate = if sample.is_multiple_of(47) {
            &noise
        } else {
            &coding
        };
        let _ = hysteresis.update(candidate, &sample.to_string(), sample * 2_000);
    }

    let runtime = SituationRuntime::new(SituationRuntimeSettings::default(), None)
        .expect("runtime initializes");
    for index in 0..1_000 {
        runtime.record_failure(format!("bounded fixture failure {index}"));
    }
    let (_, events) = runtime.events_after(0).expect("events load");
    assert_eq!(events.len(), MAX_EVENTS);
}
#[test]
pub(super) fn shadow_module_has_no_outbound_or_intervention_calls() {
    let source = [
        include_str!("../calibration.rs"),
        include_str!("../classifier.rs"),
        include_str!("../contracts.rs"),
        include_str!("../repository.rs"),
        include_str!("../platform/mod.rs"),
        include_str!("../platform/macos.rs"),
        include_str!("../platform/unsupported.rs"),
    ]
    .join("\n");
    for forbidden in [
        "reqwest::",
        "TcpStream",
        "UdpSocket",
        "Command::new",
        "start_turn",
        "speak_text",
        "codex_",
        "notification",
    ] {
        assert!(
            !source.contains(forbidden),
            "Situation Shadow module must not contain {forbidden}"
        );
    }
}
#[test]
pub(super) fn disabled_monitor_does_not_start_a_worker() {
    let connection = Connection::open_in_memory().expect("database opens");
    crate::initialize_database(&connection).expect("database initializes");
    let state = crate::test_support::app_state(connection);
    assert!(!state.situation.enabled());
    spawn_situation_monitor(state.sqlite_writer.clone(), state.situation.clone());
    assert!(!state.situation.is_worker_running());
    assert!(state.situation.begin_worker());
    assert!(!state.situation.begin_worker());
    state.situation.finish_worker();
    assert!(!state.situation.is_worker_running());
    assert_eq!(state.situation.sample_interval_ms(), 2_000);
}
