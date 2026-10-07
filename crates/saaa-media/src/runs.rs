//! In-memory run lifetime for one media service instance.
use saaa_larm_session::media::{MediaClient, MediaResult};
use std::{collections::HashMap, sync::Arc, time::Instant};
use tokio::sync::{watch, Mutex, Semaphore};

use crate::validate_run_id;

pub struct RunEntry {
    pub cancel: watch::Sender<bool>,
    pub client: Option<Arc<MediaClient>>,
    pub result: Option<MediaResult>,
    pub finished: bool,
    pub created: Instant,
}

pub struct MediaService {
    runs: Mutex<HashMap<String, RunEntry>>,
    downloads: Semaphore,
}

impl Default for MediaService {
    fn default() -> Self {
        Self {
            runs: Mutex::new(HashMap::new()),
            downloads: Semaphore::new(2),
        }
    }
}

impl MediaService {
    pub fn runs(&self) -> &Mutex<HashMap<String, RunEntry>> {
        &self.runs
    }

    pub fn downloads(&self) -> &Semaphore {
        &self.downloads
    }

    pub async fn cancel_registered(&self, run_id: &str) -> Result<(), String> {
        validate_run_id(run_id)?;
        let mut entries = self.runs.lock().await;
        if let Some(entry) = entries.get(run_id) {
            entry.cancel.send_replace(true);
        } else {
            trim(&mut entries);
            let (cancel, _) = watch::channel(true);
            entries.insert(
                run_id.to_string(),
                RunEntry {
                    cancel,
                    client: None,
                    result: None,
                    finished: true,
                    created: Instant::now(),
                },
            );
        }
        Ok(())
    }
}

pub fn trim(entries: &mut HashMap<String, RunEntry>) {
    if entries.len() >= 16 {
        let oldest = entries
            .iter()
            .filter(|(_, entry)| entry.finished)
            .min_by_key(|(_, entry)| entry.created)
            .map(|(id, _)| id.clone());
        if let Some(oldest) = oldest {
            entries.remove(&oldest);
        }
    }
}
