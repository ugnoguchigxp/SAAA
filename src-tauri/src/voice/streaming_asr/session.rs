use super::{
    batch_engine::{BatchEngine, DecodeKind, DecodeRequest},
    batch_runtime::{BatchDecode, BatchDecodeOutcome},
    contracts::{CommitReason, VoiceAsrFailureCode, VoiceAsrStreamEvent},
    speaker_gate_runtime::SpeakerGate,
};
use crate::{persistence::audit::VoiceAsrAuditChannel, RunCancellation};
use std::{
    collections::VecDeque,
    sync::{Arc, Weak},
    time::Duration,
};
use tokio::sync::{mpsc, oneshot, Semaphore};
use zeroize::Zeroizing;
include!("session.d/01.rs");
include!("session.d/02.rs");
