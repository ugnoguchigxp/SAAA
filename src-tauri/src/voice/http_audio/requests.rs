use super::*;
pub(crate) async fn play(
    provider: &CloudTtsProviderSettings,
    text: &str,
    timeout_ms: u64,
    cancellation: Arc<RunCancellation>,
    on_started: impl FnOnce() + Send + 'static,
) -> Result<(), String> {
    play_guarded(
        provider,
        text,
        timeout_ms,
        cancellation,
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
        on_started,
    )
    .await
}
pub(crate) async fn play_guarded(
    provider: &CloudTtsProviderSettings,
    text: &str,
    timeout_ms: u64,
    cancellation: Arc<RunCancellation>,
    output: Arc<std::sync::atomic::AtomicBool>,
    on_started: impl FnOnce() + Send + 'static,
) -> Result<(), String> {
    let started = std::time::Instant::now();
    let response =
        crate::voice::cloud_tts::request_audio(provider, text, timeout_ms, cancellation.clone())
            .await?;
    play_response(
        response,
        &provider.response_format,
        cancellation,
        on_started,
        started,
        None,
        output,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn play_larm(
    session: &Arc<saaa_larm_session::Session>,
    voice: Option<&str>,
    output: Arc<std::sync::atomic::AtomicBool>,
    text: &str,
    timeout_ms: u64,
    cancellation: Arc<RunCancellation>,
    on_started: impl FnOnce() + Send + 'static,
) -> Result<(), String> {
    let started = std::time::Instant::now();
    let lease = tokio::select! { biased;
        _ = cancellation.cancelled() => return Err("Speech cancelled".into()),
        result = tokio::time::timeout(std::time::Duration::from_millis(timeout_ms), session.acquire("tts")) => result.map_err(|_| "TTS lease acquisition timed out")?.map_err(str::to_string)?,
    };
    let provider = crate::larm_voice::audio::tts_settings(lease.provider(), voice)?;
    let remaining = timeout_ms.saturating_sub(started.elapsed().as_millis() as u64);
    if remaining == 0 {
        return Err("TTS request timed out".into());
    }
    let remaining = lease
        .request_budget(std::time::Duration::from_millis(remaining))
        .map_err(str::to_string)?
        .as_millis() as u64;
    let receive_budget = std::time::Duration::from_millis(remaining);
    let headers_started = std::time::Instant::now();
    let response = crate::voice::cloud_tts::request_audio_with_api_key(
        &provider,
        text,
        remaining,
        cancellation.clone(),
        Some(lease.provider().token()),
    )
    .await?;
    play_response(
        response,
        &provider.response_format,
        cancellation,
        on_started,
        started,
        Some((
            lease,
            receive_budget.saturating_sub(headers_started.elapsed()),
        )),
        output,
    )
    .await
}
