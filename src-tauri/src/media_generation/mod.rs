//! Image/music requests use LARM's public service API, independently of Warm connections.
#[cfg(test)]
use crate::persistence;
#[cfg(test)]
use crate::providers::service_registry::{AdapterKind, Purpose, ResolvedRoute};
use crate::AppState;
use saaa_larm_session::media::MediaProgress;
#[cfg(test)]
use saaa_larm_session::media::{MediaError, MediaKind, MediaResult};
#[cfg(test)]
use serde_json::json;
use serde_json::Value;
#[cfg(test)]
use std::sync::Arc;
#[cfg(test)]
use tokio::sync::watch;
mod artifacts;
mod generation;
mod host;
#[cfg(all(test, feature = "conversation-queue-e2e"))]
mod ipc_tests;
#[cfg(test)]
mod ledger;
mod recovery;
#[cfg(test)]
pub(crate) mod replicate;
#[cfg(test)]
mod replicate_tests;
#[cfg(test)]
mod writer_contract;

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

pub(crate) use host::assemble;
pub(crate) use saaa_media::{GenerateInput, GenerateOutput};

#[tauri::command]
pub(crate) async fn cancel_media_generation(
    state: tauri::State<'_, AppState>,
    run_id: String,
) -> Result<(), String> {
    match state.media.cancel(&run_id).await {
        Ok(outcome) => match outcome.remote_stop {
            saaa_media::RemoteStop::Unconfirmed => Err(
                "遠隔の停止を確認できませんでした。生成し直さず、進行状況を照会してください。"
                    .into(),
            ),
            _ => Ok(()),
        },
        Err(error) => Err(error.message),
    }
}
