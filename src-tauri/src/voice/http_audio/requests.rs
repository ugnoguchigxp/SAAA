use super::*;
#[allow(clippy::too_many_arguments)]
pub(crate) async fn play_with_situation(
    provider: &CloudTtsProviderSettings,
    text: &str,
    timeout_ms: u64,
    cancellation: Arc<RunCancellation>,
    output: Arc<std::sync::atomic::AtomicBool>,
    on_started: impl FnOnce() + Send + 'static,
    situation: Option<Arc<crate::situation::SituationRuntime>>,
) -> Result<(), String> {
    if held(&situation)
    {
        return Ok(());
    }
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
        situation,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn play_larm_with_situation(
    session: &Arc<saaa_larm_session::Session>,
    voice: Option<&str>,
    output: Arc<std::sync::atomic::AtomicBool>,
    text: &str,
    timeout_ms: u64,
    cancellation: Arc<RunCancellation>,
    on_started: impl FnOnce() + Send + 'static,
    situation: Option<Arc<crate::situation::SituationRuntime>>,
) -> Result<(), String> {
    if held(&situation)
    {
        return Ok(());
    }
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
    if held(&situation)
    {
        return Ok(());
    }
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
        situation,
    )
    .await
}

fn held(situation: &Option<Arc<crate::situation::SituationRuntime>>) -> bool {
    situation.as_deref().is_some_and(crate::situation::speech_holds_runtime)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn wr_t19_hold_during_synthesis_prevents_playback() {
        let situation =
            Arc::new(crate::situation::SituationRuntime::new(Default::default(), None).unwrap());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let observed = situation.clone();
        let router = axum::Router::new().route(
            "/v1/audio/speech",
            axum::routing::post(move || {
                let observed = observed.clone();
                async move {
                    observed.set_scene_attention_for_test("MEETING", "OBSERVE");
                    "not audio; must never be decoded or queued"
                }
            }),
        );
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let provider = crate::CloudTtsProviderSettings {
            id: "fixture".into(),
            label: "fixture".into(),
            enabled: true,
            location: "local".into(),
            authentication: "none".into(),
            endpoint: format!("http://{address}/v1/audio/speech"),
            model: "fixture".into(),
            voice: "voice".into(),
            response_format: "wav".into(),
        };
        let output = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let result = play_with_situation(
            &provider,
            "answer",
            1000,
            Arc::new(RunCancellation::default()),
            output.clone(),
            || panic!("held audio started"),
            Some(situation),
        )
        .await;
        server.abort();
        assert!(result.is_ok(), "{result:?}");
        assert!(!output.load(std::sync::atomic::Ordering::Acquire));
    }
}
