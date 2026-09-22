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
include!("mod.d/01.rs");
include!("mod.d/02.rs");
#[cfg(test)]
mod tests {
    include!("mod.d/03.rs");
}
