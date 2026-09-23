use super::speaker::SpeakerExtractor;
use crate::persistence::{SqliteReaders, SqliteWriter};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::Mutex,
};
use zeroize::Zeroizing;
mod codec;
mod enrollment;
pub(crate) mod streaming_verifier;
use codec::*;
pub use codec::{migrate_v10_to_v11, migrate_v14_to_v15, reconcile_voice_profile_storage};
mod voice_profile_snapshot;
pub(super) use voice_profile_snapshot::MODEL_SHA256;
use voice_profile_snapshot::{
    enrollment_uses_input_device, CANONICAL_SAMPLE_RATE, DEFAULT_THRESHOLD,
    ENROLLMENT_CONSISTENCY_THRESHOLD, MAX_SAMPLE_SECONDS, MIN_READY_DURATION_MS, MIN_READY_SAMPLES,
    MIN_SAMPLE_SECONDS, PROFILE_ID, TARGET_SAMPLE_COUNT,
};
pub use voice_profile_snapshot::{
    SaveVoiceEnrollmentSampleInput, SetTargetSpeakerFilterInput, VoiceProfileRuntime,
    VoiceProfileSnapshot, VoiceSampleSummary,
};
#[cfg(test)]
mod tests;
