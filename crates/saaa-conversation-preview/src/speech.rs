use crate::config::TtsSelection;
use futures_util::StreamExt;
use rodio::{Decoder, OutputStream, Sink, Source};
use saaa_larm_session::Session;
use std::{
    fs::File,
    io::{BufReader, Write},
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use tempfile::{Builder, TempPath};
use tokio::sync::watch;
use tokio::time::Instant;

const MAX_ARTIFACT_BYTES: usize = 16 * 1024 * 1024;

pub struct Synthesis {
    pub artifact: TempPath,
    pub allocation_id: Option<String>,
    pub model: Option<String>,
}

pub async fn synthesize(
    selection: &TtsSelection,
    session: &Arc<Session>,
    text: &str,
    directory: &Path,
    cancellation: &mut watch::Receiver<bool>,
    speech_deadline: Instant,
    request_id: &str,
) -> Result<Synthesis, String> {
    let deadline = std::cmp::min(speech_deadline, Instant::now() + Duration::from_secs(15));
    match tokio::time::timeout_at(
        deadline,
        synthesize_inner(
            selection,
            session,
            text,
            directory,
            cancellation,
            request_id,
        ),
    )
    .await
    {
        Ok(result) => result,
        Err(_) => Err(match selection {
            TtsSelection::System { .. } => "tts_timeout",
            TtsSelection::Larm { .. } => "tts_request_outcome_unknown",
        }
        .into()),
    }
}

async fn synthesize_inner(
    selection: &TtsSelection,
    session: &Arc<Session>,
    text: &str,
    directory: &Path,
    cancellation: &mut watch::Receiver<bool>,
    request_id: &str,
) -> Result<Synthesis, String> {
    if *cancellation.borrow() {
        return Err("speech_cancelled".into());
    }
    let suffix = match selection {
        TtsSelection::System { .. } => ".aiff",
        TtsSelection::Larm { .. } => ".wav",
    };
    let mut artifact = Builder::new()
        .prefix("speech-")
        .suffix(suffix)
        .tempfile_in(directory)
        .map_err(|_| "speech_artifact_unavailable".to_string())?;
    let mut allocation_id = None;
    let mut model = None;
    match selection {
        TtsSelection::System { voice } => {
            let mut command = tokio::process::Command::new("say");
            command.kill_on_drop(true).arg("-o").arg(artifact.path());
            if voice != "default" {
                command.arg("-v").arg(voice);
            }
            command.arg("--").arg(text);
            let mut child = command
                .spawn()
                .map_err(|_| "system_tts_unavailable".to_string())?;
            let status = tokio::select! {
                biased;
                _ = cancelled(cancellation) => {
                    let _ = child.kill().await;
                    return Err("speech_cancelled".into());
                }
                result = tokio::time::timeout(Duration::from_secs(15), child.wait()) => {
                    result.map_err(|_| "tts_timeout")?
                        .map_err(|_| "system_tts_failed")?
                }
            };
            if !status.success() {
                return Err("system_tts_failed".into());
            }
        }
        TtsSelection::Larm { voice } => {
            let lease = tokio::select! {
                biased;
                _ = cancelled(cancellation) => return Err("speech_cancelled".into()),
                result = tokio::time::timeout(Duration::from_secs(15), session.acquire("tts")) => {
                    result.map_err(|_| "tts_timeout")?.map_err(str::to_string)?
                }
            };
            let budget = lease
                .request_budget(Duration::from_secs(15))
                .map_err(str::to_string)?;
            let provider = lease.provider();
            if provider.protocol != "openai.audio-speech.v1" {
                return Err("tts_protocol_unsupported".into());
            }
            allocation_id = Some(lease.allocation_id().to_string());
            model = Some(provider.model.clone());
            let url = provider.endpoint("audio/speech").map_err(str::to_string)?;
            let client = reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(|_| "tts_client_unavailable")?;
            let response = tokio::select! {
                biased;
                _ = cancelled(cancellation) => return Err("tts_request_outcome_unknown".into()),
                result = client.post(url)
                    .bearer_auth(provider.token())
                    .header("X-Request-ID", request_id)
                    .timeout(budget)
                    .json(&serde_json::json!({
                        "model": provider.model,
                        "voice": voice,
                        "input": text,
                        "response_format": "wav"
                    }))
                    .send() => result.map_err(|_| "tts_request_outcome_unknown")?
            };
            if !response.status().is_success() {
                return Err("tts_provider_rejected".into());
            }
            if response
                .content_length()
                .is_some_and(|len| len > MAX_ARTIFACT_BYTES as u64)
            {
                return Err("tts_artifact_too_large".into());
            }
            let mut stream = response.bytes_stream();
            let mut size = 0;
            loop {
                let chunk = tokio::select! {
                    biased;
                    _ = cancelled(cancellation) => return Err("tts_request_outcome_unknown".into()),
                    chunk = stream.next() => chunk,
                };
                let Some(chunk) = chunk else {
                    break;
                };
                let chunk = chunk.map_err(|_| "tts_request_outcome_unknown")?;
                size += chunk.len();
                if size > MAX_ARTIFACT_BYTES {
                    return Err("tts_artifact_too_large".into());
                }
                artifact
                    .write_all(&chunk)
                    .map_err(|_| "tts_artifact_write_failed")?;
            }
        }
    }
    let size = artifact
        .as_file()
        .metadata()
        .map_err(|_| "tts_artifact_unavailable")?
        .len() as usize;
    if size == 0 || size > MAX_ARTIFACT_BYTES {
        return Err("tts_artifact_invalid".into());
    }
    Ok(Synthesis {
        artifact: artifact.into_temp_path(),
        allocation_id,
        model,
    })
}

pub async fn play(
    artifact: &Path,
    cancellation: &mut watch::Receiver<bool>,
    speech_deadline: Instant,
    on_started: impl FnOnce() -> Result<(), String> + Send + 'static,
) -> Result<(), String> {
    let deadline = std::cmp::min(speech_deadline, Instant::now() + Duration::from_secs(120));
    let path = artifact.to_path_buf();
    let stop = Arc::new(AtomicBool::new(false));
    let _stop_on_drop = StopOnDrop(stop.clone());
    let worker_stop = stop.clone();
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let mut drain = tokio::task::spawn_blocking(move || -> Result<(), String> {
        let file = File::open(path).map_err(|_| "playback_artifact_unavailable")?;
        let decoder = Decoder::new(BufReader::new(file)).map_err(|_| "playback_decode_failed")?;
        let duration = decoder
            .total_duration()
            .ok_or("playback_duration_unknown")?;
        if duration > Duration::from_secs(30) {
            return Err("playback_too_long".into());
        }
        let (_stream, handle) = OutputStream::try_default().map_err(|_| "player_unavailable")?;
        let sink = Sink::try_new(&handle).map_err(|_| "player_unavailable")?;
        sink.append(StartedSource {
            inner: decoder,
            started: Some(started_tx),
        });
        loop {
            if worker_stop.load(Ordering::SeqCst) {
                sink.stop();
                return Err("speech_cancelled".into());
            }
            if sink.empty() {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    });
    tokio::select! {
        biased;
        _ = cancelled(cancellation) => {
            stop.store(true, Ordering::SeqCst);
            return match wait_stopped(&mut drain).await {
                Err(code) if code == "player_stop_unconfirmed" || code == "player_join_failed" => Err(code),
                _ => Err("speech_cancelled".into()),
            };
        }
        _ = tokio::time::sleep_until(deadline) => {
            stop.store(true, Ordering::SeqCst);
            return match wait_stopped(&mut drain).await {
                Err(code) if code == "player_stop_unconfirmed" || code == "player_join_failed" => Err(code),
                _ => Err("player_timeout".into()),
            };
        }
        started = tokio::time::timeout(Duration::from_secs(15), started_rx) => {
            if started.is_err() || started.as_ref().is_ok_and(Result::is_err) {
                stop.store(true, Ordering::SeqCst);
                return match wait_stopped(&mut drain).await {
                    Err(code) if code == "player_stop_unconfirmed" || code == "player_join_failed" => Err(code),
                    _ => Err("player_start_failed".into()),
                };
            }
        }
    }
    if *cancellation.borrow() {
        stop.store(true, Ordering::SeqCst);
        return match wait_stopped(&mut drain).await {
            Err(code) if code == "player_stop_unconfirmed" || code == "player_join_failed" => {
                Err(code)
            }
            _ => Err("speech_cancelled".into()),
        };
    }
    if let Err(error) = on_started() {
        stop.store(true, Ordering::SeqCst);
        if let Err(code) = wait_stopped(&mut drain).await {
            if code == "player_stop_unconfirmed" || code == "player_join_failed" {
                return Err(code);
            }
        }
        return Err(error);
    }
    tokio::select! {
        biased;
        _ = cancelled(cancellation) => {
            stop.store(true, Ordering::SeqCst);
            match wait_stopped(&mut drain).await {
                Err(code) if code == "player_stop_unconfirmed" || code == "player_join_failed" => Err(code),
                _ => Err("speech_cancelled".into()),
            }
        }
        _ = tokio::time::sleep_until(deadline) => {
            stop.store(true, Ordering::SeqCst);
            match wait_stopped(&mut drain).await {
                Err(code) if code == "player_stop_unconfirmed" || code == "player_join_failed" => Err(code),
                _ => Err("player_timeout".into()),
            }
        }
        result = &mut drain => {
            result.map_err(|_| "player_join_failed".to_string())?
        }
    }
}

struct StopOnDrop(Arc<AtomicBool>);

impl Drop for StopOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

async fn wait_stopped(
    drain: &mut tokio::task::JoinHandle<Result<(), String>>,
) -> Result<(), String> {
    match tokio::time::timeout(Duration::from_secs(5), drain).await {
        Ok(Ok(Err(code))) if code == "speech_cancelled" => Err(code),
        Ok(Ok(result)) => result,
        Ok(Err(_)) => Err("player_join_failed".into()),
        Err(_) => Err("player_stop_unconfirmed".into()),
    }
}

async fn cancelled(receiver: &mut watch::Receiver<bool>) {
    while !*receiver.borrow_and_update() {
        if receiver.changed().await.is_err() {
            break;
        }
    }
}

struct StartedSource<S> {
    inner: S,
    started: Option<tokio::sync::oneshot::Sender<()>>,
}

impl<S> Iterator for StartedSource<S>
where
    S: Source,
    S::Item: rodio::Sample,
{
    type Item = S::Item;
    fn next(&mut self) -> Option<Self::Item> {
        let sample = self.inner.next();
        if sample.is_some() {
            if let Some(sender) = self.started.take() {
                let _ = sender.send(());
            }
        }
        sample
    }
}

impl<S> Source for StartedSource<S>
where
    S: Source,
    S::Item: rodio::Sample,
{
    fn current_frame_len(&self) -> Option<usize> {
        self.inner.current_frame_len()
    }
    fn channels(&self) -> u16 {
        self.inner.channels()
    }
    fn sample_rate(&self) -> u32 {
        self.inner.sample_rate()
    }
    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }
}
