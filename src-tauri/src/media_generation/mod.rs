//! Image/music requests use LARM's public service API, independently of Warm connections.
use crate::providers::service_registry::{AdapterKind, Purpose, ResolvedRoute};
use crate::{persistence, AppState};
use serde_json::{json, Value};
mod artifacts;
mod generation;
mod ledger;
mod recovery;
pub(crate) mod replicate;
#[cfg(test)]
mod replicate_tests;
use saaa_larm_session::media::{
    MediaArtifact, MediaClient, MediaError, MediaKind, MediaProgress, MediaResult,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::{Arc, OnceLock},
};
use tokio::sync::{watch, Mutex};
#[cfg(all(test, feature = "conversation-queue-e2e"))]
mod ipc_tests;

/// Keep media IPC registration with the feature instead of growing the global registry.
pub(crate) fn with_handler<R: tauri::Runtime>(
    fallback: impl Fn(tauri::ipc::Invoke<R>) -> bool + Send + Sync + 'static,
) -> impl Fn(tauri::ipc::Invoke<R>) -> bool + Send + Sync + 'static {
    let media: fn(tauri::ipc::Invoke<R>) -> bool = tauri::generate_handler![
        generation::generate_media,
        cancel_media_generation,
        artifacts::read_generated_media,
        recovery::list_media_generations,
        recovery::reconcile_media_generation
    ];
    move |invoke: tauri::ipc::Invoke<R>| match invoke.message.command() {
        "generate_media"
        | "cancel_media_generation"
        | "read_generated_media"
        | "list_media_generations"
        | "reconcile_media_generation" => media(invoke),
        _ => fallback(invoke),
    }
}

struct Entry {
    cancel: watch::Sender<bool>,
    client: Option<Arc<MediaClient>>,
    result: Option<MediaResult>,
    finished: bool,
    created: std::time::Instant,
}
static RUNS: OnceLock<Mutex<HashMap<String, Entry>>> = OnceLock::new();
fn runs() -> &'static Mutex<HashMap<String, Entry>> {
    RUNS.get_or_init(Default::default)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct GenerateInput {
    run_id: String,
    kind: MediaKind,
    prompt: String,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GenerateOutput {
    run_id: String,
    result: Option<MediaResult>,
    error: Option<MediaError>,
}

#[tauri::command]
pub(crate) async fn cancel_media_generation(
    state: tauri::State<'_, AppState>,
    run_id: String,
) -> Result<(), String> {
    cancel_registered(&run_id).await?;
    recovery::cancel_stored(&state, &run_id).await
}

async fn cancel_registered(run_id: &str) -> Result<(), String> {
    let run_id = run_id.to_string();
    validate_id(&run_id)?;
    let mut entries = runs().lock().await;
    if let Some(entry) = entries.get(&run_id) {
        entry.cancel.send_replace(true);
    } else {
        trim(&mut entries);
        let (cancel, _) = watch::channel(true);
        entries.insert(
            run_id,
            Entry {
                cancel,
                client: None,
                result: None,
                finished: true,
                created: std::time::Instant::now(),
            },
        );
    }
    Ok(())
}

fn validate_id(id: &str) -> Result<(), String> {
    if uuid::Uuid::parse_str(id).is_err() {
        Err("生成要求の識別子が不正です。".into())
    } else {
        Ok(())
    }
}

fn trim(entries: &mut HashMap<String, Entry>) {
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

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn early_cancel_is_retained_before_generation_has_registered() {
        let id = uuid::Uuid::new_v4().to_string();
        cancel_registered(&id).await.unwrap();
        let mut entries = runs().lock().await;
        let entry = entries.remove(&id).unwrap();
        assert!(*entry.cancel.borrow());
        assert!(entry.finished && entry.client.is_none());
    }
    #[test]
    fn media_ids_cannot_address_other_resources() {
        assert!(validate_id(&uuid::Uuid::new_v4().to_string()).is_ok());
        for id in ["", "../other", "http://other"] {
            assert!(validate_id(id).is_err());
        }
    }
}
