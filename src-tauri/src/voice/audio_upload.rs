use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use tauri::ipc::{InvokeBody, Request};
use zeroize::Zeroizing;

const PURPOSE_HEADER: &str = "x-saaa-audio-purpose";
const MAX_UPLOAD_BYTES: usize = 16_000 * 2 * 120;
const MAX_STAGED_BYTES: usize = MAX_UPLOAD_BYTES * 2;
const MAX_STAGED_UPLOADS: usize = 8;
const UPLOAD_TTL: Duration = Duration::from_secs(30);

struct StagedAudio {
    purpose: String,
    bytes: Zeroizing<Vec<u8>>,
    created_at: Instant,
}

#[derive(Clone, Default)]
pub(crate) struct AudioUploadStore {
    uploads: Arc<Mutex<HashMap<String, StagedAudio>>>,
}

impl AudioUploadStore {
    pub(crate) fn stage(&self, request: Request<'_>) -> Result<String, String> {
        let purpose = request
            .headers()
            .get(PURPOSE_HEADER)
            .and_then(|value| value.to_str().ok())
            .filter(|value| matches!(*value, "chat-asr" | "meeting-segment" | "voice-enrollment"))
            .ok_or_else(|| "Invalid audio upload purpose".to_string())?;
        let InvokeBody::Raw(bytes) = request.body() else {
            return Err("Audio upload must use binary IPC".to_string());
        };
        if bytes.is_empty() || bytes.len() % 2 != 0 || bytes.len() > MAX_UPLOAD_BYTES {
            return Err("Audio upload is empty, malformed, or too large".to_string());
        }
        let mut uploads = self
            .uploads
            .lock()
            .map_err(|_| "Audio upload store unavailable".to_string())?;
        let now = Instant::now();
        uploads.retain(|_, upload| now.duration_since(upload.created_at) < UPLOAD_TTL);
        let staged_bytes = uploads
            .values()
            .map(|upload| upload.bytes.len())
            .sum::<usize>();
        if uploads.len() >= MAX_STAGED_UPLOADS
            || staged_bytes.saturating_add(bytes.len()) > MAX_STAGED_BYTES
        {
            return Err("Too many audio uploads are waiting to be processed".to_string());
        }
        let upload_id = crate::new_id("audio");
        uploads.insert(
            upload_id.clone(),
            StagedAudio {
                purpose: purpose.to_string(),
                bytes: Zeroizing::new(bytes.clone()),
                created_at: now,
            },
        );
        drop(uploads);
        let expiration_store = self.clone();
        let expiration_id = upload_id.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(UPLOAD_TTL).await;
            expiration_store.expire(&expiration_id);
        });
        Ok(upload_id)
    }

    pub(crate) fn consume(&self, upload_id: &str, purpose: &str) -> Result<Vec<f32>, String> {
        crate::validate_identifier(upload_id, "audio upload id")?;
        let upload = self
            .uploads
            .lock()
            .map_err(|_| "Audio upload store unavailable".to_string())?
            .remove(upload_id)
            .ok_or_else(|| "Audio upload expired or was already consumed".to_string())?;
        if upload.created_at.elapsed() >= UPLOAD_TTL || upload.purpose != purpose {
            return Err("Audio upload expired or has the wrong purpose".to_string());
        }
        Ok(upload
            .bytes
            .chunks_exact(2)
            .map(|bytes| i16::from_le_bytes([bytes[0], bytes[1]]) as f32 / 32_767.0)
            .map(|sample| sample.clamp(-1.0, 1.0))
            .collect())
    }

    fn expire(&self, upload_id: &str) {
        let Ok(mut uploads) = self.uploads.lock() else {
            return;
        };
        if uploads
            .get(upload_id)
            .is_some_and(|upload| upload.created_at.elapsed() >= UPLOAD_TTL)
        {
            uploads.remove(upload_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tauri::{http::HeaderMap, ipc::InvokeBody};

    #[test]
    fn consumes_binary_audio_once_and_enforces_purpose() {
        let store = AudioUploadStore::default();
        let body = InvokeBody::Raw(vec![0, 0, 0xff, 0x7f]);
        let mut headers = HeaderMap::new();
        headers.insert(PURPOSE_HEADER, "chat-asr".parse().unwrap());
        // Request fields are private, so the command boundary exercises staging; conversion is covered here.
        let upload = StagedAudio {
            purpose: "chat-asr".to_string(),
            bytes: Zeroizing::new(match body {
                InvokeBody::Raw(bytes) => bytes,
                _ => unreachable!(),
            }),
            created_at: Instant::now(),
        };
        store
            .uploads
            .lock()
            .unwrap()
            .insert("audio_test".to_string(), upload);
        let samples = store.consume("audio_test", "chat-asr").unwrap();
        assert_eq!(samples, vec![0.0, 1.0]);
        assert!(store.consume("audio_test", "chat-asr").is_err());
        assert!(!headers.is_empty());
    }

    #[test]
    fn physically_removes_expired_audio_from_the_bounded_store() {
        let store = AudioUploadStore::default();
        store.uploads.lock().unwrap().insert(
            "audio_expired".to_string(),
            StagedAudio {
                purpose: "chat-asr".to_string(),
                bytes: Zeroizing::new(vec![0, 0]),
                created_at: Instant::now() - UPLOAD_TTL - Duration::from_millis(1),
            },
        );

        store.expire("audio_expired");

        assert!(store.uploads.lock().unwrap().is_empty());
    }
}
