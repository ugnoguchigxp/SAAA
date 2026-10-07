//! Desktop binding. The HTTP client lives in `saaa-media`.
use super::*;
use std::sync::Arc;

fn io(state: &AppState, run: Option<&str>) -> saaa_media::replicate::ReplicateIo {
    let readers = state.sqlite_readers.clone();
    let phase_writer = Arc::clone(&state.sqlite_writer);
    let cache_writer = Arc::clone(&state.sqlite_writer);
    let phase_run = run.map(str::to_string);
    let cache_run = phase_run.clone();
    saaa_media::replicate::ReplicateIo {
        validate: Arc::new(move |route| {
            readers.read(|db| crate::providers::service_registry::validate_active(db, route))
        }),
        load_secret: Arc::new(|service, account| {
            crate::credentials::load_named_secret(service, account)
        }),
        phase: Arc::new(move |phase, job| {
            let Some(run) = phase_run.as_deref() else {
                return Err("取消の通信では台帳を更新しません".into());
            };
            phase_writer.write(|db| saaa_media::phase(db, &crate::now_iso(), run, phase, job))
        }),
        cache: Arc::new(move |index, bytes| {
            let Some(run) = cache_run.as_deref() else {
                return Err("取消の通信では成果物を保存しません".into());
            };
            cache_writer.write(|db| saaa_media::cache(db, run, index, bytes))
        }),
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn generate(
    state: &AppState,
    run: &str,
    kind: MediaKind,
    prompt: &str,
    route: &ResolvedRoute,
    cancel: watch::Receiver<bool>,
    progress: &tauri::ipc::Channel<MediaProgress>,
    resume: Option<&str>,
) -> Result<MediaResult, MediaError> {
    let bound = io(state, Some(run));
    saaa_media::replicate::generate(
        &bound,
        run,
        kind,
        prompt,
        route,
        cancel,
        &|event| {
            let _ = progress.send(event);
        },
        resume,
    )
    .await
}

pub(super) async fn download(
    route: &ResolvedRoute,
    raw: &str,
    deadline: tokio::time::Instant,
) -> Result<(Vec<u8>, String), String> {
    saaa_media::replicate::download(route, raw, deadline).await
}

pub(super) async fn cancel_existing(
    state: &AppState,
    route: &ResolvedRoute,
    id: &str,
) -> MediaError {
    // Cancellation confirms the remote prediction. It must not record that id as a run.
    saaa_media::replicate::cancel_existing(&io(state, None), route, id).await
}
