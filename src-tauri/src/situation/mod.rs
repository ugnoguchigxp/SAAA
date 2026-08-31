pub mod calibration;
mod monitor;
pub(crate) use monitor::spawn_situation_monitor;
mod classifier;
pub mod contracts;
pub mod platform;
pub mod repository;
mod tick;

use crate::persistence::{SqliteReaders, SqliteWriter};
#[cfg(test)]
use classifier::classify;
use classifier::{classify_with_parameters, shadow_policy, Hysteresis};
use contracts::{
    initial_decision, initial_signals, initial_state, AudioSignal, AudioState, CalendarSignal,
    CalendarState, CalibrationParameters, ConversationSignal, ConversationState,
    ForegroundCategory, ForegroundSignal, InputActivitySignal, InputActivityState,
    MicrophoneSignal, MicrophoneState, OwnedSignalInput, QualityWindowCounters, ShadowDecision,
    SignalHealth, SignalHealthEntry, SignalSnapshot, SituationEvent, SituationLedgerEntry,
    SituationRuntimeFailure, SituationRuntimeSettings, SituationSnapshot, SituationState,
    TimeBucket,
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
    input_activity: contracts::InputActivitySignal,
    observed_at: String,
    observed_ms: u128,
}

struct RuntimeInner {
    settings: SituationRuntimeSettings,
    calibration_parameters: CalibrationParameters,
    calibration_rule_version: String,
    quality: QualityWindowCounters,
    quality_started_ms: u128,
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
                calibration_rule_version: contracts::RULE_VERSION.to_string(),
                quality: QualityWindowCounters::default(),
                quality_started_ms: 0,
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

    #[cfg(test)]
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

    pub fn set_monitoring(&self, connection: &SqliteWriter, enabled: bool) -> Result<(), String> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| "Situation runtime lock unavailable".to_string())?;
        connection.write(|database| {
            if inner.settings.enabled && !enabled && inner.quality.sample_count > 0 {
                repository::persist_quality_window(
                    database,
                    inner.quality_started_ms,
                    epoch_millis(),
                    &inner.calibration_rule_version,
                    &inner.quality,
                    &inner.settings,
                )?;
                inner.quality = QualityWindowCounters::default();
                inner.quality_started_ms = 0;
            }
            let settings = repository::save_enabled(database, enabled)?;
            let stopped = inner.settings.enabled && !settings.enabled;
            inner.settings = settings.clone();
            if stopped {
                push_event(
                    &mut inner,
                    SituationEvent::MonitoringStopped {
                        reason: "Paused by user".to_string(),
                    },
                );
            }
            Ok(())
        })?;
        drop(inner);
        self.worker_wake.notify_one();
        Ok(())
    }

    pub fn configure_and_persist<T>(
        &self,
        connection: &SqliteWriter,
        settings: SituationRuntimeSettings,
        persist: impl FnOnce(&mut Connection) -> Result<T, String>,
    ) -> Result<T, String> {
        validate_settings(&settings)?;
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| "Situation runtime lock unavailable".to_string())?;
        let result = connection.write(|database| {
            if inner.settings.enabled && inner.quality.sample_count > 0 {
                repository::persist_quality_window(
                    database,
                    inner.quality_started_ms,
                    epoch_millis(),
                    &inner.calibration_rule_version,
                    &inner.quality,
                    &inner.settings,
                )?;
                inner.quality = QualityWindowCounters::default();
                inner.quality_started_ms = 0;
            }
            let result = persist(database)?;
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
            Ok(result)
        })?;
        drop(inner);
        self.worker_wake.notify_one();
        Ok(result)
    }

    pub fn clear_history(&self, connection: &SqliteWriter) -> Result<(), String> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| "Situation runtime lock unavailable".to_string())?;
        connection.write(|database| repository::clear_history(database))?;
        inner.quality = QualityWindowCounters::default();
        inner.quality_started_ms = 0;
        Ok(())
    }

    pub fn flush_quality(&self, connection: &SqliteWriter) -> Result<(), String> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| "Situation runtime lock unavailable".to_string())?;
        if inner.quality.sample_count == 0 {
            return Ok(());
        }
        let ended_at_ms = epoch_millis();
        connection.write(|database| {
            repository::persist_quality_window(
                database,
                inner.quality_started_ms,
                ended_at_ms,
                &inner.calibration_rule_version,
                &inner.quality,
                &inner.settings,
            )
        })?;
        inner.quality = QualityWindowCounters::default();
        inner.quality_started_ms = 0;
        Ok(())
    }

    pub fn set_calibration_profile(
        &self,
        profile: calibration::CalibrationProfile,
    ) -> Result<(), String> {
        contracts::validate_calibration_parameters(&profile.parameters)?;
        validate_rule_version(&profile.rule_version)?;
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| "Situation runtime lock unavailable".to_string())?;
        inner.calibration_parameters = profile.parameters;
        inner.calibration_rule_version = profile.rule_version;
        Ok(())
    }

    pub fn decide_calibration(
        &self,
        connection: &SqliteWriter,
        profile_id: &str,
        decision: &str,
        reason_code: &str,
    ) -> Result<calibration::CalibrationProfile, String> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| "Situation runtime lock unavailable".to_string())?;
        let active = connection.write(|database| {
            if inner.quality.sample_count > 0 {
                let ended_at_ms = epoch_millis();
                repository::persist_quality_window(
                    database,
                    inner.quality_started_ms,
                    ended_at_ms,
                    &inner.calibration_rule_version,
                    &inner.quality,
                    &inner.settings,
                )?;
                inner.quality = QualityWindowCounters::default();
                inner.quality_started_ms = 0;
            }
            calibration::decide(database, profile_id, decision, reason_code)
        })?;
        inner.calibration_parameters = active.parameters.clone();
        inner.calibration_rule_version = active.rule_version.clone();
        Ok(active)
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
        let (enabled, calendar_enabled, calibration_parameters) = {
            let inner = self
                .inner
                .lock()
                .map_err(|_| "Situation runtime lock unavailable".to_string())?;
            (
                inner.settings.enabled,
                inner.settings.calendar_enabled,
                inner.calibration_parameters.clone(),
            )
        };
        if !enabled {
            return Ok(SituationSample {
                foreground: ForegroundSignal {
                    category: ForegroundCategory::Unknown,
                    health: SignalHealth::Disabled,
                },
                calendar: CalendarSignal {
                    state: CalendarState::Unavailable,
                    time_bucket: TimeBucket::None,
                    health: SignalHealth::Disabled,
                },
                input_activity: InputActivitySignal {
                    state: InputActivityState::Unknown,
                    health: SignalHealth::Disabled,
                },
                observed_at: crate::now_iso(),
                observed_ms: epoch_millis(),
            });
        }
        Ok(SituationSample {
            foreground: platform::foreground_signal(),
            calendar: platform::calendar_signal(calendar_enabled),
            input_activity: platform::input_activity_signal(&calibration_parameters),
            observed_at: crate::now_iso(),
            observed_ms: epoch_millis(),
        })
    }

    #[cfg(test)]
    pub fn tick(&self, connection: &SqliteWriter) -> Result<(), String> {
        let sample = self.sample_platform()?;
        self.tick_sampled(connection, sample)
    }
}

fn accumulate_quality(
    counters: &mut QualityWindowCounters,
    candidate_changed: bool,
    transitioned: bool,
    unknown: bool,
    owned_is_stale: bool,
    proposed_attention: &str,
    health: &[SignalHealthEntry],
) {
    counters.sample_count = counters.sample_count.saturating_add(1);
    counters.candidate_change_count = counters
        .candidate_change_count
        .saturating_add(u64::from(candidate_changed));
    counters.stable_transition_count = counters
        .stable_transition_count
        .saturating_add(u64::from(transitioned));
    counters.unknown_sample_count = counters
        .unknown_sample_count
        .saturating_add(u64::from(unknown));
    counters.stale_owned_signal_count = counters
        .stale_owned_signal_count
        .saturating_add(u64::from(owned_is_stale));
    match proposed_attention {
        "IGNORE" => {
            counters.decision_ignore_count = counters.decision_ignore_count.saturating_add(1)
        }
        "OBSERVE" => {
            counters.decision_observe_count = counters.decision_observe_count.saturating_add(1)
        }
        "SUGGEST" => {
            counters.decision_suggest_count = counters.decision_suggest_count.saturating_add(1)
        }
        "RESPOND" => {
            counters.decision_respond_count = counters.decision_respond_count.saturating_add(1)
        }
        _ => {}
    }
    for item in health {
        match item.health {
            SignalHealth::Ready => {
                counters.health_ready_count = counters.health_ready_count.saturating_add(1)
            }
            SignalHealth::Disabled => {
                counters.health_disabled_count = counters.health_disabled_count.saturating_add(1)
            }
            SignalHealth::PermissionDenied => {
                counters.health_permission_denied_count =
                    counters.health_permission_denied_count.saturating_add(1)
            }
            SignalHealth::Unsupported => {
                counters.health_unsupported_count =
                    counters.health_unsupported_count.saturating_add(1)
            }
            SignalHealth::Degraded => {
                counters.health_degraded_count = counters.health_degraded_count.saturating_add(1)
            }
        }
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

fn validate_rule_version(version: &str) -> Result<(), String> {
    if version.is_empty()
        || version.len() > 160
        || !version.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        })
    {
        return Err("Invalid Situation rule version".to_string());
    }
    Ok(())
}

fn signal_health(signals: &SignalSnapshot) -> Vec<SignalHealthEntry> {
    vec![
        SignalHealthEntry {
            source: "foreground".to_string(),
            health: signals.foreground.health,
        },
        SignalHealthEntry {
            source: "microphone".to_string(),
            health: signals.microphone.health,
        },
        SignalHealthEntry {
            source: "audio".to_string(),
            health: signals.audio.health,
        },
        SignalHealthEntry {
            source: "calendar".to_string(),
            health: signals.calendar.health,
        },
        SignalHealthEntry {
            source: "input-activity".to_string(),
            health: signals.input_activity.health,
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
    use contracts::{
        CalendarState, ForegroundCategory, ForegroundSignal, InputActivitySignal,
        InputActivityState, TimeBucket,
    };

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
            input_activity: InputActivitySignal {
                state: InputActivityState::Unknown,
                health: SignalHealth::Unsupported,
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
    fn idle_activity_suppresses_only_suggestions_and_never_explicit_or_sensitive_safety() {
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
    fn idle_unknown_and_non_ready_activity_preserve_existing_policy() {
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
    fn disabled_runtime_returns_disabled_signals_without_platform_sampling() {
        let runtime = SituationRuntime::new(SituationRuntimeSettings::default(), None)
            .expect("runtime initializes");
        let sample = runtime.sample_platform().expect("disabled sample projects");
        assert_eq!(sample.foreground.health, SignalHealth::Disabled);
        assert_eq!(sample.calendar.health, SignalHealth::Disabled);
        assert_eq!(sample.input_activity.health, SignalHealth::Disabled);
        assert_eq!(sample.input_activity.state, InputActivityState::Unknown);
    }

    #[test]
    fn input_activity_health_change_is_emitted_once() {
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
    fn failed_ledger_write_does_not_advance_runtime_state() {
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
    fn failed_quality_window_is_bounded_to_two_intervals() {
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
            include_str!("calibration.rs"),
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
