pub mod calibration;
mod classifier;
pub mod contracts;
pub mod platform;
pub mod repository;

#[cfg(test)]
use classifier::classify;
use classifier::{classify_with_parameters, shadow_policy, Hysteresis};
use contracts::{
    initial_decision, initial_signals, initial_state, AudioSignal, AudioState, CalendarSignal,
    CalibrationParameters, ConversationSignal, ConversationState, MicrophoneSignal,
    MicrophoneState, OwnedSignalInput, ShadowDecision, SignalHealth, SignalHealthEntry,
    SignalSnapshot, SituationEvent, SituationLedgerEntry, SituationRuntimeFailure,
    SituationRuntimeSettings, SituationSnapshot, SituationState,
};
use rusqlite::Connection;
use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::Notify;

const MAX_EVENTS: usize = 64;

pub struct SituationRuntime {
    inner: Mutex<RuntimeInner>,
    worker_running: AtomicBool,
    worker_wake: Notify,
}

pub(crate) struct SituationSample {
    foreground: contracts::ForegroundSignal,
    calendar: CalendarSignal,
    observed_at: String,
    observed_ms: u128,
}

struct RuntimeInner {
    settings: SituationRuntimeSettings,
    calibration_parameters: CalibrationParameters,
    owned: OwnedSignalInput,
    owned_updated_ms: u128,
    signals: SignalSnapshot,
    state: SituationState,
    decision: ShadowDecision,
    last_failure: Option<SituationRuntimeFailure>,
    hysteresis: Hysteresis,
    last_candidate_scene: String,
    last_persisted_ms: u128,
    next_revision: u64,
    events: VecDeque<(u64, SituationEvent)>,
}

impl SituationRuntime {
    pub fn new(
        settings: SituationRuntimeSettings,
        latest: Option<&SituationLedgerEntry>,
    ) -> Result<Self, String> {
        validate_settings(&settings)?;
        let now = crate::now_iso();
        let state = latest
            .map(|entry| entry.state.clone())
            .unwrap_or_else(|| initial_state(&now));
        let decision = latest
            .map(|entry| entry.decision.clone())
            .unwrap_or_else(|| initial_decision(&now));
        Ok(Self {
            inner: Mutex::new(RuntimeInner {
                settings,
                calibration_parameters: CalibrationParameters::default(),
                owned: OwnedSignalInput {
                    conversation_state: ConversationState::Idle,
                    microphone_state: MicrophoneState::Inactive,
                    audio_state: AudioState::Silent,
                },
                owned_updated_ms: 0,
                signals: initial_signals(&now),
                hysteresis: Hysteresis::from_state(state.clone()),
                last_candidate_scene: state.scene.clone(),
                state,
                decision,
                last_failure: None,
                last_persisted_ms: 0,
                next_revision: 1,
                events: VecDeque::new(),
            }),
            worker_running: AtomicBool::new(false),
            worker_wake: Notify::new(),
        })
    }

    pub fn configure(&self, settings: SituationRuntimeSettings) -> Result<(), String> {
        validate_settings(&settings)?;
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| "Situation runtime lock unavailable".to_string())?;
        let stopped = inner.settings.enabled && !settings.enabled;
        inner.settings = settings;
        if stopped {
            push_event(
                &mut inner,
                SituationEvent::MonitoringStopped {
                    reason: "Paused by user".to_string(),
                },
            );
        }
        drop(inner);
        self.worker_wake.notify_one();
        Ok(())
    }

    pub fn set_calibration_parameters(
        &self,
        parameters: CalibrationParameters,
    ) -> Result<(), String> {
        contracts::validate_calibration_parameters(&parameters)?;
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| "Situation runtime lock unavailable".to_string())?;
        inner.calibration_parameters = parameters;
        Ok(())
    }

    pub fn report_owned(&self, input: OwnedSignalInput) -> Result<(), String> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| "Situation runtime lock unavailable".to_string())?;
        inner.owned = input;
        inner.owned_updated_ms = epoch_millis();
        Ok(())
    }

    pub fn set_conversation_state(&self, state: ConversationState) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.owned.conversation_state = state;
            inner.owned_updated_ms = epoch_millis();
        }
    }

    pub fn set_microphone_state(&self, state: MicrophoneState) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.owned.microphone_state = state;
            inner.owned_updated_ms = epoch_millis();
        }
    }

    pub fn set_audio_state(&self, state: AudioState) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.owned.audio_state = state;
            inner.owned_updated_ms = epoch_millis();
        }
    }

    pub fn record_failure(&self, message: String) {
        if let Ok(mut inner) = self.inner.lock() {
            let failure = SituationRuntimeFailure {
                code: "situation-runtime-error".to_string(),
                message: crate::redact_runtime_text(&message),
                recovery: "Pause and re-enable Situation monitoring. Other SAAA features remain available.".to_string(),
            };
            inner.last_failure = Some(failure.clone());
            push_event(
                &mut inner,
                SituationEvent::Failed {
                    code: failure.code,
                    message: failure.message,
                    recovery: failure.recovery,
                },
            );
        }
    }

    pub fn enabled(&self) -> bool {
        self.inner
            .lock()
            .map(|inner| inner.settings.enabled)
            .unwrap_or(false)
    }

    pub fn sample_interval_ms(&self) -> u64 {
        self.inner
            .lock()
            .map(|inner| inner.settings.sample_interval_ms)
            .unwrap_or(2_000)
    }

    pub fn begin_worker(&self) -> bool {
        self.worker_running
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    }

    pub fn finish_worker(&self) {
        self.worker_running.store(false, Ordering::SeqCst);
    }

    pub fn is_worker_running(&self) -> bool {
        self.worker_running.load(Ordering::SeqCst)
    }

    pub async fn wait_for_next_sample(&self) {
        tokio::select! {
            () = tokio::time::sleep(Duration::from_millis(self.sample_interval_ms())) => {}
            () = self.worker_wake.notified() => {}
        }
    }

    pub(crate) fn sample_platform(&self) -> Result<SituationSample, String> {
        let calendar_enabled = self
            .inner
            .lock()
            .map_err(|_| "Situation runtime lock unavailable".to_string())?
            .settings
            .calendar_enabled;
        Ok(SituationSample {
            foreground: platform::foreground_signal(),
            calendar: platform::calendar_signal(calendar_enabled),
            observed_at: crate::now_iso(),
            observed_ms: epoch_millis(),
        })
    }

    #[cfg(test)]
    pub fn tick(&self, connection: &Connection) -> Result<(), String> {
        let sample = self.sample_platform()?;
        self.tick_sampled(connection, sample)
    }

    pub(crate) fn tick_sampled(
        &self,
        connection: &Connection,
        sample: SituationSample,
    ) -> Result<(), String> {
        let SituationSample {
            foreground,
            calendar,
            observed_at: now,
            observed_ms: now_ms,
        } = sample;
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| "Situation runtime lock unavailable".to_string())?;
        if !inner.settings.enabled {
            return Ok(());
        }

        let previous_health = signal_health(&inner.signals);
        let sequence = inner.signals.sequence.saturating_add(1);
        let owned = fresh_owned(
            &inner.owned,
            inner.owned_updated_ms,
            now_ms,
            inner.settings.sample_interval_ms,
        );
        let signals = SignalSnapshot {
            sequence,
            observed_at: now.clone(),
            foreground,
            conversation: ConversationSignal {
                state: owned.conversation_state,
            },
            microphone: MicrophoneSignal {
                state: owned.microphone_state,
                health: SignalHealth::Ready,
            },
            audio: AudioSignal {
                state: owned.audio_state,
                health: SignalHealth::Ready,
            },
            calendar,
        };
        let next_health = signal_health(&signals);
        let mut pending_events = Vec::new();
        for item in &next_health {
            if previous_health
                .iter()
                .find(|previous| previous.source == item.source)
                .map(|previous| &previous.health)
                != Some(&item.health)
            {
                pending_events.push(SituationEvent::SignalHealthChanged {
                    source: item.source.clone(),
                    health: item.health.clone(),
                });
            }
        }

        let candidate = classify_with_parameters(&signals, &inner.calibration_parameters);
        if candidate.scene != inner.last_candidate_scene {
            let mut projected = inner.state.clone();
            projected.scene.clone_from(&candidate.scene);
            projected.confidence = candidate.confidence;
            projected.evidence.clone_from(&candidate.evidence);
            projected
                .user_attention
                .clone_from(&candidate.user_attention);
            projected
                .audio_environment
                .clone_from(&candidate.audio_environment);
            projected.candidate_since = now.clone();
            projected.updated_at = now.clone();
            pending_events.push(SituationEvent::CandidateChanged { state: projected });
        }
        let mut hysteresis = inner.hysteresis.clone();
        let (state, transitioned) = hysteresis.update_with_parameters(
            &candidate,
            &now,
            now_ms,
            &inner.calibration_parameters,
        );
        let decision = shadow_policy(&state, &signals, &now);
        let decision_changed = decision.proposed_attention != inner.decision.proposed_attention;
        let heartbeat_due = inner.last_persisted_ms == 0
            || now_ms.saturating_sub(inner.last_persisted_ms)
                >= u128::from(inner.settings.heartbeat_interval_ms);
        let entry_kind = if transitioned {
            Some("transition")
        } else if decision_changed {
            Some("decision")
        } else if heartbeat_due {
            Some("heartbeat")
        } else {
            None
        };
        let entry = entry_kind.map(|entry_kind| SituationLedgerEntry {
            id: crate::new_id("situation"),
            observed_at: now.clone(),
            state: state.clone(),
            decision: decision.clone(),
            signal_health: next_health.clone(),
            entry_kind: entry_kind.to_string(),
            feedback: None,
        });
        if let Some(entry) = &entry {
            repository::persist_entry_with_retention(connection, entry, &inner.settings, now_ms)?;
        }

        inner.last_candidate_scene.clone_from(&candidate.scene);
        inner.hysteresis = hysteresis;
        inner.signals = signals;
        inner.state = state;
        inner.decision = decision;
        inner.last_failure = None;
        for event in pending_events {
            push_event(&mut inner, event);
        }
        if let Some(entry) = entry {
            inner.last_persisted_ms = now_ms;
            if transitioned {
                push_event(
                    &mut inner,
                    SituationEvent::StableStateChanged {
                        entry: entry.clone(),
                    },
                );
            }
            if decision_changed {
                push_event(&mut inner, SituationEvent::ShadowDecisionChanged { entry });
            }
        }
        Ok(())
    }

    pub fn snapshot(&self, connection: &Connection) -> Result<SituationSnapshot, String> {
        let (monitoring_enabled, signals, state, decision, last_failure) = {
            let inner = self
                .inner
                .lock()
                .map_err(|_| "Situation runtime lock unavailable".to_string())?;
            (
                inner.settings.enabled,
                inner.signals.clone(),
                inner.state.clone(),
                inner.decision.clone(),
                inner.last_failure.clone(),
            )
        };
        Ok(SituationSnapshot {
            monitoring_enabled,
            monitoring_active: self.is_worker_running(),
            signals,
            state,
            decision,
            last_failure,
            history: repository::list_history(connection)?,
            evaluation: repository::evaluation_summary(connection)?,
        })
    }

    #[cfg(test)]
    pub fn events_after(&self, revision: u64) -> Result<(u64, Vec<SituationEvent>), String> {
        let inner = self
            .inner
            .lock()
            .map_err(|_| "Situation runtime lock unavailable".to_string())?;
        let events = inner
            .events
            .iter()
            .filter(|(event_revision, _)| *event_revision > revision)
            .map(|(_, event)| event.clone())
            .collect();
        Ok((inner.next_revision.saturating_sub(1), events))
    }
}

pub fn validate_settings(settings: &SituationRuntimeSettings) -> Result<(), String> {
    if !(500..=60_000).contains(&settings.sample_interval_ms)
        || !(1..=30).contains(&settings.retention_days)
        || !(100..=10_000).contains(&settings.max_ledger_entries)
        || !(60_000..=3_600_000).contains(&settings.heartbeat_interval_ms)
        || !settings.sensitive_application_categories
    {
        return Err("Invalid Situation runtime settings".to_string());
    }
    Ok(())
}

pub fn validate_scene(scene: &str) -> Result<(), String> {
    if scene.is_empty()
        || scene.len() > 80
        || !scene.chars().all(|character| {
            character.is_ascii_uppercase()
                || character.is_ascii_digit()
                || matches!(character, '-' | '_' | '.')
        })
    {
        return Err("Invalid Situation scene identifier".to_string());
    }
    Ok(())
}

fn signal_health(signals: &SignalSnapshot) -> Vec<SignalHealthEntry> {
    vec![
        SignalHealthEntry {
            source: "foreground".to_string(),
            health: signals.foreground.health.clone(),
        },
        SignalHealthEntry {
            source: "microphone".to_string(),
            health: signals.microphone.health.clone(),
        },
        SignalHealthEntry {
            source: "audio".to_string(),
            health: signals.audio.health.clone(),
        },
        SignalHealthEntry {
            source: "calendar".to_string(),
            health: signals.calendar.health.clone(),
        },
    ]
}

fn push_event(inner: &mut RuntimeInner, event: SituationEvent) {
    let revision = inner.next_revision;
    inner.next_revision = inner.next_revision.saturating_add(1);
    inner.events.push_back((revision, event));
    while inner.events.len() > MAX_EVENTS {
        inner.events.pop_front();
    }
}

fn epoch_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn fresh_owned(
    input: &OwnedSignalInput,
    updated_ms: u128,
    now_ms: u128,
    sample_interval_ms: u64,
) -> OwnedSignalInput {
    let fresh_for = u128::from(sample_interval_ms).saturating_mul(3);
    if updated_ms > 0 && now_ms.saturating_sub(updated_ms) <= fresh_for {
        input.clone()
    } else {
        OwnedSignalInput {
            conversation_state: ConversationState::Idle,
            microphone_state: MicrophoneState::Inactive,
            audio_state: AudioState::Silent,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use contracts::{CalendarState, ForegroundCategory, ForegroundSignal, TimeBucket};

    fn signals(category: ForegroundCategory) -> SignalSnapshot {
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
        }
    }

    #[test]
    fn classifier_is_deterministic_and_sensitive_is_safe() {
        let coding = signals(ForegroundCategory::Coding);
        assert_eq!(classify(&coding).scene, "CODING");
        assert_eq!(classify(&coding).scene, classify(&coding).scene);
        let sensitive = signals(ForegroundCategory::Sensitive);
        let candidate = classify(&sensitive);
        assert_eq!(candidate.scene, "UNKNOWN");
        let state = initial_state(&crate::now_iso());
        let decision = shadow_policy(&state, &sensitive, &crate::now_iso());
        assert_eq!(decision.proposed_attention, "IGNORE");
        assert_eq!(decision.actual_execution, "NONE");
        assert_eq!(decision.actual_presentation, "SILENT");
    }

    #[test]
    fn hysteresis_rejects_short_noise_and_accepts_three_fresh_samples() {
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
    fn all_shadow_policy_paths_are_non_intervening() {
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
            let decision = shadow_policy(&state, &snapshot, &crate::now_iso());
            assert_eq!(decision.actual_execution, "NONE");
            assert_eq!(decision.actual_presentation, "SILENT");
        }
    }

    #[test]
    fn shadow_policy_exposes_all_four_counterfactual_attention_decisions() {
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
            shadow_policy(&state, &snapshot, &now).proposed_attention,
            "SUGGEST"
        );

        state.scene = "MEETING".to_string();
        state.user_attention = "busy".to_string();
        assert_eq!(
            shadow_policy(&state, &snapshot, &now).proposed_attention,
            "OBSERVE"
        );

        snapshot.conversation.state = ConversationState::UserInput;
        assert_eq!(
            shadow_policy(&state, &snapshot, &now).proposed_attention,
            "RESPOND"
        );

        snapshot.conversation.state = ConversationState::Idle;
        snapshot.foreground.category = ForegroundCategory::Sensitive;
        assert_eq!(
            shadow_policy(&state, &snapshot, &now).proposed_attention,
            "IGNORE"
        );
    }

    #[test]
    fn denied_optional_signal_does_not_block_available_local_signal() {
        let mut snapshot = signals(ForegroundCategory::Coding);
        snapshot.calendar = CalendarSignal {
            state: CalendarState::Unavailable,
            time_bucket: TimeBucket::None,
            health: SignalHealth::PermissionDenied,
        };
        assert_eq!(classify(&snapshot).scene, "CODING");
    }

    #[test]
    fn owned_lifecycle_flows_through_runtime_to_ledger_and_pause_stops_writes() {
        let connection = Connection::open_in_memory().expect("database opens");
        crate::initialize_database(&connection).expect("database initializes");
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
        let snapshot = runtime.snapshot(&connection).expect("snapshot loads");
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
            .snapshot(&connection)
            .expect("paused snapshot loads");
        assert_eq!(paused.history.len(), 1);
        assert!(!paused.monitoring_enabled);
    }

    #[test]
    fn failed_ledger_write_does_not_advance_runtime_state() {
        let connection = Connection::open_in_memory().expect("database opens");
        crate::initialize_database(&connection).expect("database initializes");
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
            .execute("DROP TABLE situation_ledger", [])
            .expect("fixture removes ledger");

        assert!(runtime.tick(&connection).is_err());
        let inner = runtime.inner.lock().expect("runtime lock");
        assert_eq!(inner.signals.sequence, 0);
        assert_eq!(inner.state.scene, "UNKNOWN");
        assert_eq!(inner.last_persisted_ms, 0);
    }

    #[test]
    fn runtime_rejects_invalid_sampling_configuration() {
        let invalid = SituationRuntimeSettings {
            sample_interval_ms: 0,
            ..SituationRuntimeSettings::default()
        };
        assert!(SituationRuntime::new(invalid, None).is_err());
    }

    #[test]
    fn optional_calendar_degrades_without_exposing_details() {
        let calendar = platform::calendar_signal(true);
        assert_eq!(calendar.state, CalendarState::Unavailable);
        assert_eq!(calendar.health, SignalHealth::Unsupported);
        assert_eq!(calendar.time_bucket, TimeBucket::None);
    }

    #[test]
    fn stale_owned_signal_falls_back_to_idle() {
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
    fn eight_hour_fixture_replay_and_event_queue_remain_bounded() {
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
    fn shadow_module_has_no_outbound_or_intervention_calls() {
        let source = [
            include_str!("classifier.rs"),
            include_str!("contracts.rs"),
            include_str!("repository.rs"),
            include_str!("platform/mod.rs"),
            include_str!("platform/macos.rs"),
            include_str!("platform/unsupported.rs"),
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
}
