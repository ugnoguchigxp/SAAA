#[cfg(test)]
use super::classifier::classify;
use super::classifier::{classify_with_parameters, shadow_policy, Hysteresis};
use super::contracts::{
    initial_decision, initial_signals, initial_state, AudioSignal, AudioState, CalendarSignal,
    CalendarState, CalibrationParameters, ConversationSignal, ConversationState,
    ForegroundCategory, ForegroundSignal, InputActivitySignal, InputActivityState,
    MicrophoneSignal, MicrophoneState, OwnedSignalInput, QualityWindowCounters, ShadowDecision,
    SignalHealth, SignalHealthEntry, SignalSnapshot, SituationEvent, SituationLedgerEntry,
    SituationRuntimeFailure, SituationRuntimeSettings, SituationSnapshot, SituationState,
    TimeBucket,
};
use super::*;
use crate::persistence::{SqliteReaders, SqliteWriter};
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
pub(crate) const MAX_EVENTS: usize = 64;
pub struct SituationRuntime {
    pub(super) inner: Mutex<RuntimeInner>,
    pub(super) worker_running: AtomicBool,
    pub(super) worker_wake: Notify,
}
pub(crate) struct SituationSample {
    pub(super) foreground: contracts::ForegroundSignal,
    pub(super) calendar: CalendarSignal,
    pub(super) input_activity: contracts::InputActivitySignal,
    pub(super) observed_at: String,
    pub(super) observed_ms: u128,
}
pub(crate) struct RuntimeInner {
    pub(super) world_sequence: u64,
    pub(super) world_digest: String,
    pub(super) settings: SituationRuntimeSettings,
    pub(super) calibration_parameters: CalibrationParameters,
    pub(super) calibration_rule_version: String,
    pub(super) quality: QualityWindowCounters,
    pub(super) quality_started_ms: u128,
    pub(super) owned: OwnedSignalInput,
    pub(super) owned_updated_ms: u128,
    pub(super) signals: SignalSnapshot,
    pub(super) state: SituationState,
    pub(super) decision: ShadowDecision,
    pub(super) last_failure: Option<SituationRuntimeFailure>,
    pub(super) tts_hold_audit_run: Option<String>,
    pub(super) hysteresis: Hysteresis,
    pub(super) last_candidate_scene: String,
    pub(super) last_persisted_ms: u128,
    pub(super) next_revision: u64,
    pub(super) events: VecDeque<(u64, SituationEvent)>,
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
                world_sequence: 0,
                world_digest: String::new(),
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
                tts_hold_audit_run: None,
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
        world_snapshot::update_version(&mut inner)?;
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

    pub(crate) fn foreground_category(&self) -> ForegroundCategory {
        self.inner
            .lock()
            .map(|inner| inner.signals.foreground.category.clone())
            .unwrap_or(ForegroundCategory::Unknown)
    }

    #[cfg(test)]
    pub(crate) fn set_scene_attention_for_test(&self, scene: &str, attention: &str) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.state.scene = scene.to_string();
            inner.decision.proposed_attention = attention.to_string();
            inner.tts_hold_audit_run = None;
        }
    }

    #[cfg(test)]
    pub(crate) fn set_foreground_category_for_test(&self, category: ForegroundCategory) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.signals.foreground.category = category;
        }
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
            world_snapshot::update_version(&mut inner)?;
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
            world_snapshot::update_version(&mut inner)?;
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
            let _ = world_snapshot::update_version(&mut inner);
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
pub(super) fn accumulate_quality(
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
