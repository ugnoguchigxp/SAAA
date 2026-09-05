use rusqlite::{params, Connection, Transaction};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};
use zeroize::Zeroizing;

use super::*;
use uuid::Uuid;

pub fn migrate_v10_to_v11(connection: &Connection) -> rusqlite::Result<()> {
    let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version >= 11 {
        return Ok(());
    }
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS voice_profiles (
           id TEXT PRIMARY KEY CHECK(id='default'),
           status TEXT NOT NULL CHECK(status IN ('collecting','ready')),
           filter_enabled INTEGER NOT NULL DEFAULT 0 CHECK(filter_enabled IN (0,1)),
           threshold REAL NOT NULL CHECK(threshold >= 0.0 AND threshold <= 1.0),
           model_sha256 TEXT NOT NULL CHECK(length(model_sha256)=64),
           embedding_dimension INTEGER NOT NULL CHECK(embedding_dimension BETWEEN 1 AND 4096),
           created_at TEXT NOT NULL,
           updated_at TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS voice_profile_samples (
           id TEXT PRIMARY KEY,
           profile_id TEXT NOT NULL,
           ordinal INTEGER NOT NULL CHECK(ordinal BETWEEN 1 AND 5),
           relative_path TEXT NOT NULL UNIQUE CHECK(length(relative_path) BETWEEN 1 AND 500),
           duration_ms INTEGER NOT NULL CHECK(duration_ms BETWEEN 3000 AND 12000),
           sample_rate INTEGER NOT NULL CHECK(sample_rate=16000),
           embedding_ciphertext BLOB NOT NULL CHECK(length(embedding_ciphertext) BETWEEN 64 AND 65536),
           input_device_id TEXT NOT NULL CHECK(length(input_device_id) BETWEEN 1 AND 300),
           effective_aec INTEGER NOT NULL CHECK(effective_aec IN (0,1)),
           created_at TEXT NOT NULL,
           FOREIGN KEY(profile_id) REFERENCES voice_profiles(id) ON DELETE CASCADE,
           UNIQUE(profile_id,ordinal)
         );
         CREATE INDEX IF NOT EXISTS idx_voice_profile_samples_profile_ordinal
           ON voice_profile_samples(profile_id,ordinal);",
    )?;
    Ok(())
}

pub fn migrate_v14_to_v15(transaction: &Transaction<'_>) -> rusqlite::Result<()> {
    let has_plain_embedding: bool = transaction.query_row(
        "SELECT EXISTS(
           SELECT 1 FROM pragma_table_info('voice_profile_samples') WHERE name='embedding'
         )",
        [],
        |row| row.get(0),
    )?;
    if has_plain_embedding {
        return Ok(());
    }
    transaction.execute_batch(
        "DROP INDEX IF EXISTS idx_voice_profile_samples_profile_ordinal;
         DROP TABLE IF EXISTS voice_profile_samples;
         DELETE FROM voice_profiles;
         CREATE TABLE voice_profile_samples (
           id TEXT PRIMARY KEY,
           profile_id TEXT NOT NULL,
           ordinal INTEGER NOT NULL CHECK(ordinal BETWEEN 1 AND 5),
           relative_path TEXT NOT NULL UNIQUE CHECK(length(relative_path) BETWEEN 1 AND 500),
           duration_ms INTEGER NOT NULL CHECK(duration_ms BETWEEN 3000 AND 12000),
           sample_rate INTEGER NOT NULL CHECK(sample_rate=16000),
           embedding BLOB NOT NULL CHECK(length(embedding) BETWEEN 8 AND 16388),
           input_device_id TEXT NOT NULL CHECK(length(input_device_id) BETWEEN 1 AND 300),
           effective_aec INTEGER NOT NULL CHECK(effective_aec IN (0,1)),
           created_at TEXT NOT NULL,
           FOREIGN KEY(profile_id) REFERENCES voice_profiles(id) ON DELETE CASCADE,
           UNIQUE(profile_id,ordinal)
         );
         CREATE INDEX idx_voice_profile_samples_profile_ordinal
           ON voice_profile_samples(profile_id,ordinal);",
    )?;
    Ok(())
}

pub fn reconcile_voice_profile_storage(
    connection: &Connection,
    data_directory: &Path,
) -> Result<(), String> {
    let stored_samples = connection
        .prepare("SELECT id,relative_path FROM voice_profile_samples WHERE profile_id=?1")
        .and_then(|mut statement| {
            statement
                .query_map([PROFILE_ID], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .map_err(database_error)?;
    let mut referenced_names = HashSet::new();
    let mut can_remove_orphaned_wav = true;
    for (sample_id, stored_path) in stored_samples {
        match expected_sample_relative_path(&sample_id) {
            Ok(path) if Path::new(&stored_path) == path => {
                if let Some(name) = path.file_name() {
                    referenced_names.insert(name.to_os_string());
                }
            }
            _ => can_remove_orphaned_wav = false,
        }
    }

    let directory = data_directory.join("voice-profiles").join(PROFILE_ID);
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(format!(
                "Could not inspect the voice-profile directory: {error}"
            ))
        }
    };
    for entry in entries {
        let entry =
            entry.map_err(|error| format!("Could not inspect a voice-profile file: {error}"))?;
        let file_type = entry
            .file_type()
            .map_err(|error| format!("Could not inspect a voice-profile file: {error}"))?;
        if !file_type.is_file() {
            continue;
        }
        let name = entry.file_name();
        let Some(name_text) = name.to_str() else {
            continue;
        };
        let legacy_encrypted = name_text
            .strip_suffix(".wav.enc")
            .is_some_and(is_valid_sample_id);
        let temporary = temporary_sample_id(name_text).is_some_and(is_valid_sample_id);
        let orphaned_wav = can_remove_orphaned_wav
            && name_text
                .strip_suffix(".wav")
                .is_some_and(is_valid_sample_id)
            && !referenced_names.contains(&name);
        if legacy_encrypted || temporary || orphaned_wav {
            fs::remove_file(entry.path()).map_err(|error| {
                format!("Could not reconcile an obsolete voice-profile file: {error}")
            })?;
        }
    }
    Ok(())
}

fn temporary_sample_id(name: &str) -> Option<&str> {
    for marker in [".wav.tmp-", ".tmp-"] {
        if let Some((sample_id, nonce)) = name.split_once(marker) {
            if nonce.len() == 32 && nonce.chars().all(|character| character.is_ascii_hexdigit()) {
                return Some(sample_id);
            }
        }
    }
    None
}

fn is_valid_sample_id(sample_id: &str) -> bool {
    validate_sample_id(sample_id).is_ok()
}

pub(super) fn snapshot_from_connection(
    connection: &Connection,
    runtime_available: bool,
    runtime_message: String,
) -> Result<VoiceProfileSnapshot, String> {
    let profile: Option<(String, bool, f32)> = connection
        .query_row(
            "SELECT status,filter_enabled,threshold FROM voice_profiles WHERE id=?1",
            [PROFILE_ID],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(database_error)?;
    let samples = connection
        .prepare(
            "SELECT id,ordinal,duration_ms,input_device_id,effective_aec,created_at
             FROM voice_profile_samples WHERE profile_id=?1 ORDER BY ordinal",
        )
        .and_then(|mut statement| {
            statement
                .query_map([PROFILE_ID], |row| {
                    Ok(VoiceSampleSummary {
                        id: row.get(0)?,
                        ordinal: row.get::<_, i64>(1)? as usize,
                        duration_ms: row.get::<_, i64>(2)? as u64,
                        input_device_id: row.get(3)?,
                        effective_aec: row.get(4)?,
                        created_at: row.get(5)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .map_err(database_error)?;
    let total_duration_ms = samples.iter().map(|sample| sample.duration_ms).sum();
    let (status, filter_enabled, threshold) =
        profile.unwrap_or_else(|| ("empty".to_string(), false, DEFAULT_THRESHOLD));
    Ok(VoiceProfileSnapshot {
        status,
        filter_enabled,
        runtime_available,
        runtime_message,
        sample_count: samples.len(),
        target_sample_count: TARGET_SAMPLE_COUNT,
        total_duration_ms,
        minimum_duration_ms: MIN_READY_DURATION_MS,
        threshold,
        samples,
    })
}

pub(super) fn update_profile_readiness(connection: &Connection) -> Result<(), String> {
    let (count, duration): (i64, i64) = connection
        .query_row(
            "SELECT COUNT(*),COALESCE(SUM(duration_ms),0) FROM voice_profile_samples WHERE profile_id=?1",
            [PROFILE_ID],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(database_error)?;
    let ready = count as usize >= MIN_READY_SAMPLES && duration as u64 >= MIN_READY_DURATION_MS;
    connection
        .execute(
            "UPDATE voice_profiles
             SET status=?1,filter_enabled=CASE WHEN ?1='ready' THEN filter_enabled ELSE 0 END,updated_at=?2
             WHERE id=?3",
            params![if ready { "ready" } else { "collecting" }, now_iso(), PROFILE_ID],
        )
        .map_err(database_error)?;
    Ok(())
}

pub(super) fn validate_enrollment_input(
    input: &SaveVoiceEnrollmentSampleInput,
    samples: &[f32],
) -> Result<(), String> {
    if !(8_000..=192_000).contains(&input.sample_rate)
        || samples.is_empty()
        || samples.iter().any(|sample| !sample.is_finite())
    {
        return Err("Enrollment audio is empty or invalid".to_string());
    }
    let duration = samples.len() as f32 / input.sample_rate as f32;
    if !(MIN_SAMPLE_SECONDS..=MAX_SAMPLE_SECONDS).contains(&duration) {
        return Err("Each voice sample must be between 10 and 12 seconds".to_string());
    }
    if input.input_device_id.trim().is_empty() || input.input_device_id.len() > 300 {
        return Err("Enrollment input device is invalid".to_string());
    }
    Ok(())
}

pub(super) fn validate_sample_quality(samples: &[f32]) -> Result<(), String> {
    let rms = root_mean_square(samples);
    let clipped = samples
        .iter()
        .filter(|sample| sample.abs() >= 0.985)
        .count() as f32
        / samples.len() as f32;
    if rms < 0.008 {
        return Err("The sample is too quiet. Move closer to the microphone and retry".to_string());
    }
    if clipped > 0.02 {
        return Err("The sample is clipped. Lower the input level and retry".to_string());
    }
    let frame_size = CANONICAL_SAMPLE_RATE as usize / 50;
    let voiced_threshold = (rms * 0.35).max(0.008);
    let voiced_frames = samples
        .chunks(frame_size)
        .filter(|frame| frame.len() == frame_size && root_mean_square(frame) >= voiced_threshold)
        .count();
    let total_frames = samples.len() / frame_size;
    if total_frames == 0 || voiced_frames as f32 / (total_frames as f32) < 0.4 {
        return Err(
            "発話として認識できた時間が短すぎます。全文を読み切る必要はないため、録音が自動停止するまで長い間を空けずに読み続けてください"
                .to_string(),
        );
    }
    Ok(())
}

pub(super) fn resample_mono(
    samples: &[f32],
    from_rate: u32,
    to_rate: u32,
) -> Result<Vec<f32>, String> {
    if from_rate == 0 || to_rate == 0 || samples.is_empty() {
        return Err("Audio sample rate is invalid".to_string());
    }
    if from_rate == to_rate {
        return Ok(samples.to_vec());
    }
    let output_len = ((samples.len() as u64 * to_rate as u64) / from_rate as u64) as usize;
    if output_len == 0 {
        return Err("Audio is too short to resample".to_string());
    }
    let ratio = from_rate as f64 / to_rate as f64;
    let mut output = Vec::with_capacity(output_len);
    for index in 0..output_len {
        let position = index as f64 * ratio;
        let lower = position.floor() as usize;
        let upper = (lower + 1).min(samples.len() - 1);
        let fraction = (position - lower as f64) as f32;
        output.push(samples[lower] * (1.0 - fraction) + samples[upper] * fraction);
    }
    Ok(output)
}

pub(super) fn encode_pcm16_wav(samples: &[f32], sample_rate: u32) -> Vec<u8> {
    let data_size = (samples.len() * 2) as u32;
    let mut wav = Vec::with_capacity(44 + data_size as usize);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_size).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16_u32.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&sample_rate.to_le_bytes());
    wav.extend_from_slice(&(sample_rate * 2).to_le_bytes());
    wav.extend_from_slice(&2_u16.to_le_bytes());
    wav.extend_from_slice(&16_u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_size.to_le_bytes());
    for sample in samples {
        let value = (sample.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16;
        wav.extend_from_slice(&value.to_le_bytes());
    }
    wav
}

pub(super) fn encode_embedding(embedding: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(4 + embedding.len() * 4);
    bytes.extend_from_slice(&(embedding.len() as u32).to_le_bytes());
    for value in embedding {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes
}

pub(super) fn decode_embedding(
    bytes: &[u8],
    expected_dimension: usize,
) -> Result<Zeroizing<Vec<f32>>, String> {
    if bytes.len() < 4 {
        return Err("Speaker embedding is truncated".to_string());
    }
    let dimension = u32::from_le_bytes(bytes[..4].try_into().expect("four-byte slice")) as usize;
    if dimension != expected_dimension || bytes.len() != 4 + dimension * 4 {
        return Err("Speaker embedding has an incompatible dimension".to_string());
    }
    let values = bytes[4..]
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes(chunk.try_into().expect("four-byte chunk")))
        .collect::<Vec<_>>();
    if values.iter().any(|value| !value.is_finite()) {
        return Err("Speaker embedding contains invalid values".to_string());
    }
    Ok(Zeroizing::new(values))
}

pub(super) fn cosine_similarity(left: &[f32], right: &[f32]) -> f32 {
    if left.len() != right.len() || left.is_empty() {
        return f32::NEG_INFINITY;
    }
    let mut dot = 0.0_f32;
    let mut left_norm = 0.0_f32;
    let mut right_norm = 0.0_f32;
    for (left, right) in left.iter().zip(right) {
        dot += left * right;
        left_norm += left * left;
        right_norm += right * right;
    }
    let denominator = left_norm.sqrt() * right_norm.sqrt();
    if denominator <= f32::EPSILON {
        f32::NEG_INFINITY
    } else {
        dot / denominator
    }
}

pub(super) fn root_mean_square(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples.iter().map(|sample| sample * sample).sum::<f32>() / samples.len() as f32).sqrt()
}

pub(super) fn verify_artifact(path: &Path, expected: &str, label: &str) -> Result<(), String> {
    let bytes = fs::read(path).map_err(|_| format!("The bundled {label} is missing"))?;
    let actual = format!("{:x}", Sha256::digest(&bytes));
    if actual != expected {
        return Err(format!("The bundled {label} failed its integrity check"));
    }
    Ok(())
}

pub(super) fn verify_bundled_library_exists(path: &Path, label: &str) -> Result<(), String> {
    let metadata = fs::metadata(path).map_err(|_| format!("The bundled {label} is missing"))?;
    if !metadata.is_file() || metadata.len() == 0 {
        return Err(format!("The bundled {label} is invalid"));
    }
    // The install script verifies the upstream archive. Production signing may
    // rewrite Mach-O signature bytes, so runtime verification uses the enclosing
    // app signature plus successful native symbol loading for dylibs.
    Ok(())
}

pub(super) fn write_private_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "Voice sample path has no parent directory".to_string())?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("Could not create the voice-profile directory: {error}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))
            .map_err(|error| format!("Could not protect the voice-profile directory: {error}"))?;
    }
    let temporary = path.with_extension(format!("tmp-{}", Uuid::new_v4().simple()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&temporary)
        .map_err(|error| format!("Could not create the voice sample: {error}"))?;
    if let Err(error) = file.write_all(bytes).and_then(|_| file.sync_all()) {
        let _ = fs::remove_file(&temporary);
        return Err(format!("Could not persist the voice sample: {error}"));
    }
    fs::rename(&temporary, path).map_err(|error| {
        let _ = fs::remove_file(&temporary);
        format!("Could not finalize the voice sample: {error}")
    })
}

pub(super) fn validate_sample_id(sample_id: &str) -> Result<(), String> {
    if !sample_id.starts_with("voice_sample_")
        || sample_id.len() > 80
        || !sample_id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
    {
        return Err("Voice sample id is invalid".to_string());
    }
    Ok(())
}

pub(super) fn expected_sample_relative_path(sample_id: &str) -> Result<PathBuf, String> {
    validate_sample_id(sample_id)?;
    Ok(PathBuf::from("voice-profiles")
        .join(PROFILE_ID)
        .join(format!("{sample_id}.wav")))
}

pub(super) fn database_error(error: rusqlite::Error) -> String {
    format!("Voice-profile database operation failed: {error}")
}

pub(super) fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339()
}
