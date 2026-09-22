async fn render_session_inner(
    receiver: &mut mpsc::Receiver<SpeechWork>,
    context: &mut RenderSessionContext,
) -> Result<(), String> {
    if matches!(context.route, TtsRoute::Fallback(..)) {
        return fallback::render(receiver, context).await;
    }
    context.route = resolve_render_route(&context.route, &context.cancellation).await?;
    if matches!(&context.route, TtsRoute::Cloud(_) | TtsRoute::Larm(..)) {
        return render_http_session(receiver, context).await;
    }
    let mut queued = VecDeque::<(u64, String, Instant)>::new();
    let mut rendering = FuturesUnordered::<RenderFuture>::new();
    let mut ready = BTreeMap::<u64, RenderedChunk>::new();
    let mut playback: Option<PlaybackTask> = None;
    let mut next_sequence = 0_u64;
    let mut next_playback = 0_u64;
    let mut input_closed = false;
    let mut playback_started = false;
    let mut adaptive_concurrency = 2_usize;
    let mut timing_samples = VecDeque::<(u64, u64)>::new();
    let mut ready_bytes = 0_u64;
    let mut ready_audio_ms = 0_u64;

    loop {
        crate::runtime::event_hub::performance::record_tts_queue_depth(render_slots_used(
            rendering.len(),
            ready.len(),
            playback.is_some(),
        ));
        if context.cancellation.is_cancelled()
            || crate::situation::speech_holds_runtime(&context.situation)
        {
            break;
        }
        while rendering.len() < adaptive_concurrency
            && render_slots_used(rendering.len(), ready.len(), playback.is_some())
                < MAX_READY_CHUNKS
            && ready_bytes < MAX_READY_AUDIO_BYTES
            && ready_audio_ms < MAX_READY_AUDIO_MS
        {
            let Some((sequence, text, expression, boundary_at)) = queued.pop_front() else {
                break;
            };
            rendering.push(render_future(
                context.route.clone(),
                sequence,
                text,
                expression,
                boundary_at,
                context.timeout_ms,
                context.cancellation.clone(),
                context.cache_directory.clone(),
            ));
        }

        if playback.is_none() {
            if let Some(chunk) = ready.remove(&next_playback) {
                ready_bytes = ready_bytes.saturating_sub(chunk.bytes);
                ready_audio_ms = ready_audio_ms.saturating_sub(chunk.audio_ms);
                if crate::situation::speech_holds_runtime(&context.situation) {
                    break;
                }
                let mut child = crate::voice::cloud_tts::spawn_audio_player(&chunk.path)?;
                crate::runtime::event_hub::performance::record_tts_boundary_to_player_spawn(
                    chunk.boundary_at.elapsed(),
                );
                {
                    let mut slot = context
                        .child
                        .lock()
                        .map_err(|_| "Streaming speech child lock unavailable".to_string())?;
                    if context.cancellation.is_cancelled() {
                        let _ = child.kill();
                        let _ = child.wait();
                        let _ = fs::remove_file(&chunk.path);
                        break;
                    }
                    *slot = Some(child);
                }
                if !playback_started {
                    context
                        .situation
                        .set_audio_state(crate::situation::contracts::AudioState::SaaaSpeaking);
                    context
                        .on_event
                        .send(RuntimeEvent::SpeechStarted {
                            run_id: context.run_id.clone(),
                        })
                        .map_err(|_| "Speech event consumer disconnected".to_string())?;
                    playback_started = true;
                }
                let child_slot = context.child.clone();
                let cancellation = context.cancellation.clone();
                playback = Some(tauri::async_runtime::spawn_blocking(move || {
                    let status = wait_for_child(&child_slot, &cancellation);
                    (chunk, status)
                }));
            }
        }

        if input_closed
            && queued.is_empty()
            && rendering.is_empty()
            && ready.is_empty()
            && playback.is_none()
        {
            break;
        }

        tokio::select! {
            _ = context.cancellation.cancelled() => break,
            work = receiver.recv(), if !input_closed && queued.len() < MAX_QUEUED_CHUNKS => {
                match work {
                    Some(SpeechWork::Chunk {
                        text,
                        expression,
                        boundary_at,
                    }) => {
                        queued.push_back((next_sequence, text, expression, boundary_at));
                        next_sequence += 1;
                    }
                    Some(SpeechWork::Finish) | None => input_closed = true,
                }
            }
            rendered = rendering.next(), if !rendering.is_empty() => {
                let rendered = rendered.expect("rendering is non-empty")?;
                if rendered.bytes > MAX_READY_AUDIO_BYTES
                    || rendered.audio_ms > MAX_READY_AUDIO_MS
                    || ready_bytes.saturating_add(rendered.bytes) > MAX_READY_AUDIO_BYTES
                    || ready_audio_ms.saturating_add(rendered.audio_ms) > MAX_READY_AUDIO_MS
                {
                    let _ = fs::remove_file(&rendered.path);
                    return Err("Rendered speech exceeded the bounded ready-audio budget".to_string());
                }
                ready_bytes += rendered.bytes;
                ready_audio_ms += rendered.audio_ms;
                timing_samples.push_back((rendered.synthesis_ms, rendered.audio_ms.max(1)));
                if timing_samples.len() > 8 {
                    timing_samples.pop_front();
                }
                adaptive_concurrency = adaptive_render_concurrency(&timing_samples);
                ready.insert(rendered.sequence, rendered);
            }
            completed = async {
                playback.as_mut().expect("playback is present").await
            }, if playback.is_some() => {
                let (chunk, status) = completed
                    .map_err(|_| "Streaming speech playback worker stopped unexpectedly".to_string())?;
                if let Ok(mut slot) = context.child.lock() {
                    let _ = slot.take();
                }
                let _ = fs::remove_file(&chunk.path);
                playback = None;
                next_playback += 1;
                match status {
                    Ok(status) if status.success() => {}
                    Ok(status) if context.cancellation.is_cancelled() => break,
                    Ok(status) => return Err(format!("TTS playback exited with {status}")),
                    Err(_) if context.cancellation.is_cancelled() => break,
                    Err(error) => return Err(error),
                }
            }
        }
    }

    for chunk in ready.into_values() {
        let _ = fs::remove_file(&chunk.path);
    }
    Ok(())
}
async fn render_http_session(
    receiver: &mut mpsc::Receiver<SpeechWork>,
    context: &RenderSessionContext,
) -> Result<(), String> {
    let mut first_phrase = true;
    loop {
        let work = tokio::select! { biased;
            _ = context.cancellation.cancelled() => return Ok(()),
            work = receiver.recv() => work,
        };
        let Some(SpeechWork::Chunk {
            text,
            expression,
            boundary_at,
        }) = work
        else {
            return Ok(());
        };
        let situation = context.situation.clone();
        let on_event = context.on_event.clone();
        let run_id = context.run_id.clone();
        let cancellation = context.cancellation.clone();
        let is_first = first_phrase;
        let on_started = move || {
            if cancellation.is_cancelled() {
                return;
            }
            crate::providers::http_metrics::record(
                "ttsBoundaryToFirstMixerSample",
                boundary_at.elapsed(),
            );
            if is_first {
                situation.set_audio_state(crate::situation::contracts::AudioState::SaaaSpeaking);
                let _ = on_event.send(RuntimeEvent::SpeechStarted { run_id });
            }
        };
        match &context.route {
            TtsRoute::Cloud(provider) => {
                let provider = crate::voice::cloud_tts::speech_directive::apply_expression(
                    provider,
                    expression,
                );
                crate::voice::http_audio::play_with_situation(
                    &provider,
                    &text,
                    context.timeout_ms,
                    context.cancellation.clone(),
                    Arc::new(std::sync::atomic::AtomicBool::new(false)),
                    on_started,
                    Some(context.situation.clone()),
                )
                .await?
            }
            TtsRoute::Larm(conversation, settings) => {
                let ready = crate::larm_voice::current_at(conversation, settings).await?;
                crate::voice::http_audio::play_larm_with_situation(
                    &ready.session,
                    settings.tts_voice.as_deref(),
                    Some(settings),
                    expression,
                    Arc::new(std::sync::atomic::AtomicBool::new(false)),
                    &text,
                    context.timeout_ms,
                    context.cancellation.clone(),
                    on_started,
                    Some(context.situation.clone()),
                )
                .await?
            }
            _ => return Err("Invalid HTTP speech route".into()),
        }
        first_phrase = false;
    }
}
async fn resolve_render_route(
    route: &TtsRoute,
    cancellation: &RunCancellation,
) -> Result<TtsRoute, String> {
    match route {
        TtsRoute::Harness(settings) => {
            let service = crate::providers::service_harness::resolve_service_cancellable(
                &settings.address,
                "tts",
                cancellation,
            )
            .await?;
            let mut provider = crate::CloudTtsProviderSettings {
                response_format: "wav".to_string(),
                id: "provider-harness-tts".to_string(),
                enabled: true,
                label: "Provider Harness TTS".to_string(),
                location: "local".to_string(),
                endpoint: service.base_url,
                model: service.model,
                voice: settings
                    .tts_voice
                    .clone()
                    .or(service.voice)
                    .ok_or_else(|| {
                        "Provider Harness TTS descriptor does not include a voice".to_string()
                    })?,
                authentication: "none".to_string(),
                style: None,
                speed: None,
                pitch_scale: None,
                intonation_scale: None,
            };
            crate::voice::cloud_tts::speech_request::apply_harness_prosody(&mut provider, settings);
            Ok(TtsRoute::Cloud(provider))
        }
        route => Ok(route.clone()),
    }
}
fn render_future(
    route: TtsRoute,
    sequence: u64,
    text: String,
    expression: crate::voice::cloud_tts::speech_directive::SpeechExpression,
    boundary_at: Instant,
    timeout_ms: u64,
    cancellation: Arc<RunCancellation>,
    cache_directory: PathBuf,
) -> RenderFuture {
    Box::pin(async move {
        let started = Instant::now();
        let path = match route {
            TtsRoute::System(provider) => {
                crate::voice::system_tts::render_tts_artifact(
                    text,
                    provider.voice,
                    cache_directory,
                    cancellation,
                )
                .await?
            }
            TtsRoute::Cloud(provider) => {
                let provider = crate::voice::cloud_tts::speech_directive::apply_expression(
                    &provider,
                    expression,
                );
                crate::voice::cloud_tts::render_to_artifact(
                    &provider,
                    &text,
                    timeout_ms,
                    cancellation,
                    &cache_directory,
                )
                .await?
            }
            TtsRoute::Harness(..) | TtsRoute::Larm(..) | TtsRoute::Fallback(..) => {
                return Err("Unresolved Harness TTS render route".to_string());
            }
        };
        finalize_rendered_chunk(sequence, boundary_at, path, started)
    })
}
fn finalize_rendered_chunk(
    sequence: u64,
    boundary_at: Instant,
    path: PathBuf,
    started: Instant,
) -> Result<RenderedChunk, String> {
    // Take ownership before validating the artifact so every error path removes it.
    let mut rendered = RenderedChunk {
        sequence,
        boundary_at,
        path,
        bytes: 0,
        audio_ms: 0,
        synthesis_ms: 0,
    };
    rendered.bytes = fs::metadata(&rendered.path)
        .map_err(|_| "Could not inspect rendered speech audio".to_string())?
        .len();
    rendered.audio_ms = wave_duration_ms(&rendered.path)?;
    rendered.synthesis_ms = started.elapsed().as_millis().try_into().unwrap_or(u64::MAX);
    Ok(rendered)
}
fn adaptive_render_concurrency(samples: &VecDeque<(u64, u64)>) -> usize {
    if samples.is_empty() {
        return 2;
    }
    let mut synthesis = samples.iter().map(|sample| sample.0).collect::<Vec<_>>();
    let mut audio = samples.iter().map(|sample| sample.1).collect::<Vec<_>>();
    synthesis.sort_unstable();
    audio.sort_unstable();
    let p95_index = ((synthesis.len() * 95).div_ceil(100)).saturating_sub(1);
    let p95 = synthesis[p95_index];
    let median = audio[audio.len() / 2].max(1);
    p95.div_ceil(median).clamp(1, MAX_RENDER_CONCURRENCY as u64) as usize
}
fn render_slots_used(rendering: usize, ready: usize, playing: bool) -> usize {
    rendering
        .saturating_add(ready)
        .saturating_add(usize::from(playing))
}
fn wave_duration_ms(path: &Path) -> Result<u64, String> {
    let mut file =
        fs::File::open(path).map_err(|_| "Could not inspect rendered speech audio".to_string())?;
    let mut header = [0_u8; 4_096];
    let length = file
        .read(&mut header)
        .map_err(|_| "Could not inspect rendered speech audio".to_string())?;
    if length < 44 || &header[..4] != b"RIFF" || &header[8..12] != b"WAVE" {
        return Err("Rendered speech audio was not a valid WAVE file".to_string());
    }
    let byte_rate = u32::from_le_bytes(header[28..32].try_into().expect("byte rate slice"));
    if byte_rate == 0 {
        return Err("Rendered speech audio had an invalid byte rate".to_string());
    }
    let mut cursor = 12_usize;
    while cursor + 8 <= length {
        let chunk_size = u32::from_le_bytes(
            header[cursor + 4..cursor + 8]
                .try_into()
                .expect("chunk size slice"),
        ) as usize;
        if &header[cursor..cursor + 4] == b"data" {
            return Ok((chunk_size as u64 * 1_000).div_ceil(byte_rate as u64));
        }
        cursor = cursor
            .saturating_add(8)
            .saturating_add(chunk_size + (chunk_size % 2));
    }
    Err("Rendered speech audio did not contain a bounded data chunk".to_string())
}
fn wait_for_child(
    child_slot: &Arc<Mutex<Option<Child>>>,
    cancellation: &RunCancellation,
) -> Result<std::process::ExitStatus, String> {
    loop {
        if cancellation.is_cancelled() {
            return Err("Speech cancelled".to_string());
        }
        let status = child_slot
            .lock()
            .map_err(|_| "Streaming speech child lock unavailable".to_string())?
            .as_mut()
            .ok_or_else(|| "Streaming speech child ownership was lost".to_string())?
            .try_wait()
            .map_err(|_| "Could not inspect streamed speech playback".to_string())?;
        if let Some(status) = status {
            return Ok(status);
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}
