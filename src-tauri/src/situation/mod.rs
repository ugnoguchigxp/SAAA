//! Ownership, invariants, and code lookup: README.md in this directory.
pub mod calibration;
mod monitor;
mod world_snapshot;
pub(crate) use monitor::spawn_situation_monitor;
mod classifier;
pub mod contracts;
pub mod platform;
pub mod repository;
mod speech;
mod tick;
use crate::persistence::{SqliteReaders, SqliteWriter};
#[cfg(test)]
pub(super) use classifier::classify;
use classifier::{classify_with_parameters, shadow_policy, Hysteresis};
pub(super) use contracts::{
    initial_decision, initial_signals, initial_state, AudioSignal, AudioState, CalendarSignal,
    CalendarState, CalibrationParameters, ConversationSignal, ConversationState,
    ForegroundCategory, ForegroundSignal, InputActivitySignal, InputActivityState,
    MicrophoneSignal, MicrophoneState, OwnedSignalInput, QualityWindowCounters, ShadowDecision,
    SignalHealth, SignalHealthEntry, SignalSnapshot, SituationEvent, SituationLedgerEntry,
    SituationRuntimeFailure, SituationRuntimeSettings, SituationSnapshot, SituationState,
    TimeBucket,
};
use rusqlite::Connection;
pub(crate) use speech::{apply_tts_hold, speech_holds_runtime, speech_holds_tts};
use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::Notify;
mod situation_runtime;
use situation_runtime::accumulate_quality;
mod health_signals;
use health_signals::{epoch_millis, fresh_owned, push_event, signal_health};
pub(crate) use situation_runtime::SituationSample;
pub(crate) use situation_runtime::{validate_scene, validate_settings, SituationRuntime};
use situation_runtime::{RuntimeInner, MAX_EVENTS};
mod tests;
