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
pub use voice_profile_snapshot::{VoiceProfileSnapshot, VoiceSampleSummary, SaveVoiceEnrollmentSampleInput, SetTargetSpeakerFilterInput, VoiceProfileRuntime};
use voice_profile_snapshot::{PROFILE_ID, MIN_READY_SAMPLES, MIN_READY_DURATION_MS, DEFAULT_THRESHOLD, CANONICAL_SAMPLE_RATE, TARGET_SAMPLE_COUNT, MIN_SAMPLE_SECONDS, MAX_SAMPLE_SECONDS, ENROLLMENT_CONSISTENCY_THRESHOLD, enrollment_uses_input_device};
pub(super) use voice_profile_snapshot::MODEL_SHA256;
#[cfg(test)]
mod tests;
