//! Image/music requests use LARM's public service API, independently of Warm connections.
use crate::{persistence, AppState};
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
        generate_media,
        cancel_media_generation,
        read_generated_media
    ];
    move |invoke: tauri::ipc::Invoke<R>| match invoke.message.command() {
        "generate_media" | "cancel_media_generation" | "read_generated_media" => media(invoke),
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
pub(crate) async fn generate_media(
    state: tauri::State<'_, AppState>,
    input: GenerateInput,
    on_progress: tauri::ipc::Channel<MediaProgress>,
) -> Result<GenerateOutput, String> {
    validate_id(&input.run_id)?;
    if input.prompt.trim().is_empty() || input.prompt.len() > 16_384 {
        return Err("生成内容は1〜16384バイトで入力してください。".into());
    }
    let settings = state
        .sqlite_readers
        .read(persistence::load_model_providers)?;
    let credential = crate::providers::dynamic_lan::credential::load()
        .map_err(|error| error.code().to_string())?;
    let (cancel, receiver) = watch::channel(false);
    {
        let mut entries = runs().lock().await;
        if let Some(entry) = entries.get(&input.run_id) {
            if *entry.cancel.borrow() && entry.client.is_none() && entry.finished {
                return Ok(GenerateOutput {
                    run_id: input.run_id,
                    result: None,
                    error: Some(MediaError {
                        kind: saaa_larm_session::media::FailureKind::Cancelled,
                        code: "cancelled_before_submission".into(),
                        retryable: false,
                        may_have_generated: false,
                        job_id: None,
                    }),
                });
            }
            return Err("この生成要求は送信済みです。同じ要求を再送できません。".into());
        }
        if entries.values().filter(|entry| !entry.finished).count() >= 2 {
            return Err("生成要求を処理中です。完了までお待ちください。".into());
        }
        trim(&mut entries);
        entries.insert(
            input.run_id.clone(),
            Entry {
                cancel,
                client: None,
                result: None,
                finished: false,
                created: std::time::Instant::now(),
            },
        );
    }
    let _ = on_progress.send(MediaProgress {
        phase: "discovering".into(),
        job_id: None,
        progress: None,
    });
    let result = async {
        let client = Arc::new(
            MediaClient::discover(
                &settings.harness.address,
                credential.token().into(),
                input.kind,
            )
            .await?,
        );
        runs()
            .lock()
            .await
            .get_mut(&input.run_id)
            .expect("registered media run")
            .client = Some(client.clone());
        client
            .generate(&input.prompt, receiver, &|event| {
                let _ = on_progress.send(event);
            })
            .await
    }
    .await;
    let mut entries = runs().lock().await;
    let entry = entries
        .get_mut(&input.run_id)
        .expect("registered media run");
    entry.finished = true;
    match result {
        Ok(result) => {
            entry.result = Some(result.clone());
            Ok(GenerateOutput {
                run_id: input.run_id,
                result: Some(result),
                error: None,
            })
        }
        Err(error) => Ok(GenerateOutput {
            run_id: input.run_id,
            result: None,
            error: Some(error),
        }),
    }
}

#[tauri::command]
pub(crate) async fn cancel_media_generation(run_id: String) -> Result<(), String> {
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

#[tauri::command]
pub(crate) async fn read_generated_media(
    run_id: String,
    artifact_index: usize,
) -> Result<tauri::ipc::Response, String> {
    validate_id(&run_id)?;
    let (client, artifact): (Arc<MediaClient>, MediaArtifact) = {
        let entries = runs().lock().await;
        let entry = entries
            .get(&run_id)
            .ok_or("成果物の参照期限が切れました。")?;
        let artifact = entry
            .result
            .as_ref()
            .and_then(|result| result.artifacts.get(artifact_index))
            .ok_or("成果物が見つかりません。")?
            .clone();
        (
            entry
                .client
                .as_ref()
                .ok_or("生成サービスを確認できません。")?
                .clone(),
            artifact,
        )
    };
    static DOWNLOADS: OnceLock<tokio::sync::Semaphore> = OnceLock::new();
    let _permit = DOWNLOADS
        .get_or_init(|| tokio::sync::Semaphore::new(2))
        .acquire()
        .await
        .map_err(|_| "成果物の取得を開始できません。")?;
    client
        .artifact_bytes(&artifact)
        .await
        .map(tauri::ipc::Response::new)
        .map_err(|error| {
            format!(
                "成果物を取得できませんでした（{}）。生成し直さず、取得を再試行できます。",
                error.code
            )
        })
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
        cancel_media_generation(id.clone()).await.unwrap();
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
