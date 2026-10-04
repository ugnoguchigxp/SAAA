use super::*;
impl AudioUploadStore {
    pub(crate) fn stage_pcm_for_e2e(&self, samples: &[i16]) -> String {
        let id = crate::new_id("audio");
        let bytes = samples
            .iter()
            .flat_map(|sample| sample.to_le_bytes())
            .collect();
        self.uploads.lock().expect("fixture audio lock").insert(
            id.clone(),
            StagedAudio {
                purpose: "conversation-asr".into(),
                bytes: Zeroizing::new(bytes),
                created_at: Instant::now(),
            },
        );
        id
    }
}
