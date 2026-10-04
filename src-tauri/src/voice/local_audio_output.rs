//! Tauri-owned PCM output shared by HTTP and system speech synthesis.
use crate::RunCancellation;
use rodio::Source;
use serde::Serialize;
use std::{
    path::Path,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex, OnceLock,
    },
    thread,
    time::Duration,
};

#[path = "local_audio_output/playback.rs"]
mod playback;
#[path = "local_audio_output/playback_started.rs"]
mod playback_started;
#[path = "local_audio_output/playback_vpio.rs"]
mod playback_vpio;

pub(crate) use playback::{Packet, Playback};

struct IdleOutput {
    stop: Arc<AtomicBool>,
    speaking: Arc<AtomicBool>,
    running: Arc<AtomicBool>,
    rendered_samples: Arc<AtomicU64>,
    error: Arc<Mutex<Option<String>>>,
    worker: thread::JoinHandle<()>,
}

struct CountingZero(Arc<AtomicU64>);

impl Iterator for CountingZero {
    type Item = i16;

    fn next(&mut self) -> Option<i16> {
        self.0.fetch_add(1, Ordering::Relaxed);
        Some(0)
    }
}

impl Source for CountingZero {
    fn current_frame_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> u16 {
        1
    }
    fn sample_rate(&self) -> u32 {
        48_000
    }
    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct IdleOutputStatus {
    running: bool,
    speaking: bool,
    rendered_samples: u64,
    error: Option<String>,
}

static IDLE_OUTPUT: OnceLock<Mutex<Option<IdleOutput>>> = OnceLock::new();
static SPEECH_ACTIVE: AtomicBool = AtomicBool::new(false);

fn idle_output() -> &'static Mutex<Option<IdleOutput>> {
    IDLE_OUTPUT.get_or_init(|| Mutex::new(None))
}

/// Keeps the local output device rendering exact zero samples while the
/// conversation screen is open. The silent source pauses during speech.
pub(crate) fn start_idle_output() -> Result<(), String> {
    let mut slot = idle_output()
        .lock()
        .map_err(|_| "無音出力の状態を取得できません。")?;
    if slot.is_some() {
        drop(slot);
        return wait_until_idle_output_renders();
    }
    let stop = Arc::new(AtomicBool::new(false));
    let speaking = Arc::new(AtomicBool::new(SPEECH_ACTIVE.load(Ordering::Acquire)));
    let running = Arc::new(AtomicBool::new(false));
    let rendered_samples = Arc::new(AtomicU64::new(0));
    let error = Arc::new(Mutex::new(None));
    let worker_stop = stop.clone();
    let worker_speaking = speaking.clone();
    let worker_running = running.clone();
    let worker_samples = rendered_samples.clone();
    let worker_error = error.clone();
    let worker = thread::Builder::new()
        .name("saaa-idle-audio".into())
        .spawn(move || {
            while !worker_stop.load(Ordering::Acquire) {
                let (_stream, handle) = match rodio::OutputStream::try_default() {
                    Ok(output) => output,
                    Err(cause) => {
                        if let Ok(mut error) = worker_error.lock() {
                            *error = Some(format!("音声出力デバイスを開けません: {cause}"));
                        }
                        thread::sleep(Duration::from_millis(500));
                        continue;
                    }
                };
                let sink = match rodio::Sink::try_new(&handle) {
                    Ok(sink) => sink,
                    Err(cause) => {
                        if let Ok(mut error) = worker_error.lock() {
                            *error = Some(format!("無音出力を開始できません: {cause}"));
                        }
                        thread::sleep(Duration::from_millis(500));
                        continue;
                    }
                };
                if let Ok(mut error) = worker_error.lock() {
                    *error = None;
                }
                sink.append(CountingZero(worker_samples.clone()));
                worker_running.store(true, Ordering::Release);
                let mut playing = true;
                while !worker_stop.load(Ordering::Acquire) {
                    let should_play = !worker_speaking.load(Ordering::Acquire)
                        && !crate::voice::audio_backend::global().is_capturing();
                    if should_play != playing {
                        if should_play {
                            sink.play();
                        } else {
                            sink.pause();
                        }
                        playing = should_play;
                    }
                    thread::sleep(Duration::from_millis(20));
                }
                sink.stop();
                worker_running.store(false, Ordering::Release);
            }
        })
        .map_err(|_| "無音出力を開始できません。".to_string())?;
    *slot = Some(IdleOutput {
        stop,
        speaking,
        running,
        rendered_samples,
        error,
        worker,
    });
    drop(slot);
    wait_until_idle_output_renders()
}

fn wait_until_idle_output_renders() -> Result<(), String> {
    for _ in 0..200 {
        let status = idle_output_status();
        if status.running
            && (status.rendered_samples > 0 || crate::voice::audio_backend::global().is_capturing())
        {
            return Ok(());
        }
        if let Some(error) = status.error {
            return Err(error);
        }
        thread::sleep(Duration::from_millis(50));
    }
    let status = idle_output_status();
    Err(format!(
        "無音出力の開始を確認できませんでした (running={}, speaking={}, capture={}, samples={}, error={:?})",
        status.running,
        status.speaking,
        crate::voice::audio_backend::global().is_capturing(),
        status.rendered_samples,
        status.error
    ))
}

pub(crate) fn stop_idle_output() {
    let output = idle_output().lock().ok().and_then(|mut slot| slot.take());
    if let Some(output) = output {
        output.stop.store(true, Ordering::Release);
        // Device initialization may be slow; closing the page must not wait for it.
        drop(output.worker);
    }
}

pub(crate) fn set_speaking(active: bool) {
    SPEECH_ACTIVE.store(active, Ordering::Release);
    if let Ok(slot) = idle_output().lock() {
        if let Some(output) = slot.as_ref() {
            output.speaking.store(active, Ordering::Release);
        }
    }
}

pub(crate) fn idle_output_status() -> IdleOutputStatus {
    if let Ok(slot) = idle_output().lock() {
        if let Some(output) = slot.as_ref() {
            return IdleOutputStatus {
                running: output.running.load(Ordering::Acquire),
                speaking: output.speaking.load(Ordering::Acquire),
                rendered_samples: output.rendered_samples.load(Ordering::Relaxed),
                error: output.error.lock().ok().and_then(|error| error.clone()),
            };
        }
    }
    IdleOutputStatus {
        running: false,
        speaking: false,
        rendered_samples: 0,
        error: None,
    }
}

/// Keeps one local output stream open through every chunk of an answer.
pub(crate) struct ContinuousPlayback(Playback);

impl ContinuousPlayback {
    pub(crate) fn start(
        cancellation: Arc<RunCancellation>,
        on_started: impl FnOnce() + Send + 'static,
    ) -> Self {
        Self(Playback::start_continuous(cancellation, on_started))
    }

    #[cfg(feature = "conversation-queue-e2e")]
    pub(crate) fn start_vpio_handoff_probe(
        cancellation: Arc<RunCancellation>,
        on_started: impl FnOnce() + Send + 'static,
    ) -> Self {
        Self(Playback::start_vpio_handoff_probe(cancellation, on_started))
    }

    pub(crate) fn sender(&self) -> &tokio::sync::mpsc::Sender<Packet> {
        &self.0.sender
    }

    pub(crate) async fn drain(&self, cancellation: &RunCancellation) -> Result<(), String> {
        self.0.drain(cancellation).await
    }

    pub(crate) async fn play_wav_file(
        &self,
        path: &Path,
        cancellation: &RunCancellation,
    ) -> Result<(), String> {
        let metadata = tokio::fs::metadata(path)
            .await
            .map_err(|_| "TTS音声ファイルを確認できませんでした。")?;
        if !metadata.is_file() || metadata.len() > 16 * 1024 * 1024 {
            return Err("TTS音声ファイルのサイズが不正です。".into());
        }
        let bytes = tokio::fs::read(path)
            .await
            .map_err(|_| "TTS音声ファイルを読み込めませんでした。")?;
        if bytes.len() > 16 * 1024 * 1024 {
            return Err("TTS音声ファイルが大きすぎます。".into());
        }
        let mut decoder = crate::voice::http_audio::decode::Decoder::new("wav")?;
        let samples = decoder.push(&bytes)?;
        decoder.finish()?;
        let format = decoder.format.ok_or("TTS音声の形式がありません。")?;
        let packet_samples = (format.rate as usize / 20) * format.channels as usize;
        for packet in samples.chunks(packet_samples) {
            tokio::select! { biased;
                _ = cancellation.cancelled() => return Err("Speech cancelled".into()),
                result = self.0.sender.send((format, packet.to_vec())) =>
                    result.map_err(|_| "Audio output worker disconnected".to_string())?,
            }
        }
        self.0.drain(cancellation).await
    }

    pub(crate) async fn finish(self) -> Result<(), String> {
        self.0.finish().await
    }
}
