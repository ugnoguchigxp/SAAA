use rodio::{Decoder, OutputStream, OutputStreamBuilder, Sink};
use std::{
    fs::File,
    path::{Path, PathBuf},
    sync::{
        mpsc::{self, Receiver, RecvTimeoutError, Sender},
        Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use tempfile::TempDir;

pub(crate) const AUDIO_OUTPUT_IDLE_TIMEOUT: Duration = Duration::from_millis(5_000);
const ACTIVE_POLL_INTERVAL: Duration = Duration::from_millis(10);
const OUTPUT_PREPARE_TIMEOUT: Duration = Duration::from_secs(10);

pub(crate) struct RenderedTtsAudio {
    directory: TempDir,
    path: PathBuf,
}

impl RenderedTtsAudio {
    pub(crate) fn new() -> Result<Self, String> {
        let directory = tempfile::Builder::new()
            .prefix("saaa-tts-")
            .tempdir()
            .map_err(|error| format!("Could not create temporary TTS storage: {error}"))?;
        let path = directory.path().join("speech.wav");
        Ok(Self { directory, path })
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

pub(crate) struct TtsAudioOutput {
    commands: Sender<AudioOutputCommand>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl TtsAudioOutput {
    pub(crate) fn new() -> Result<Self, String> {
        let (commands, receiver) = mpsc::channel();
        let worker = thread::Builder::new()
            .name("tts-audio-output".to_string())
            .spawn(move || run_audio_output_worker(receiver))
            .map_err(|error| format!("Could not start the TTS audio output worker: {error}"))?;
        Ok(Self {
            commands,
            worker: Mutex::new(Some(worker)),
        })
    }

    pub(crate) fn prepare(&self, run_id: &str) -> Result<(), String> {
        let (reply, response) = mpsc::channel();
        self.commands
            .send(AudioOutputCommand::Prepare {
                run_id: run_id.to_string(),
                reply,
            })
            .map_err(|_| "TTS audio output worker is unavailable".to_string())?;
        match response.recv_timeout(OUTPUT_PREPARE_TIMEOUT) {
            Ok(result) => result,
            Err(RecvTimeoutError::Timeout) => {
                self.cancel(run_id);
                Err("TTS audio output preparation timed out".to_string())
            }
            Err(RecvTimeoutError::Disconnected) => {
                Err("TTS audio output worker stopped during preparation".to_string())
            }
        }
    }

    pub(crate) fn play(
        &self,
        run_id: &str,
        audio: RenderedTtsAudio,
    ) -> Result<Receiver<Result<(), String>>, String> {
        let (reply, response) = mpsc::channel();
        self.commands
            .send(AudioOutputCommand::Play {
                run_id: run_id.to_string(),
                audio,
                reply,
            })
            .map_err(|_| "TTS audio output worker is unavailable".to_string())?;
        Ok(response)
    }

    pub(crate) fn cancel(&self, run_id: &str) {
        let _ = self.commands.send(AudioOutputCommand::Cancel {
            run_id: run_id.to_string(),
        });
    }

    pub(crate) fn stop(&self) {
        let _ = self.commands.send(AudioOutputCommand::Stop);
    }

    pub(crate) fn shutdown(&self) {
        let worker = self.worker.lock().ok().and_then(|mut value| value.take());
        if worker.is_some() {
            let _ = self.commands.send(AudioOutputCommand::Shutdown);
        }
    }
}

enum AudioOutputCommand {
    Prepare {
        run_id: String,
        reply: Sender<Result<(), String>>,
    },
    Play {
        run_id: String,
        audio: RenderedTtsAudio,
        reply: Sender<Result<(), String>>,
    },
    Cancel {
        run_id: String,
    },
    Stop,
    Shutdown,
}

struct OutputSession {
    _stream: OutputStream,
    sink: Sink,
}

impl OutputSession {
    fn open() -> Result<Self, String> {
        let mut stream = OutputStreamBuilder::open_default_stream()
            .map_err(|error| format!("Could not open the system audio output: {error}"))?;
        stream.log_on_drop(false);
        let sink = Sink::connect_new(stream.mixer());
        Ok(Self {
            _stream: stream,
            sink,
        })
    }
}

struct ActivePlayback {
    run_id: String,
    reply: Sender<Result<(), String>>,
}

fn run_audio_output_worker(receiver: Receiver<AudioOutputCommand>) {
    let mut session: Option<OutputSession> = None;
    let mut prepared_run_id: Option<String> = None;
    let mut keepalive_run_id: Option<String> = None;
    let mut active: Option<ActivePlayback> = None;
    let mut idle_deadline: Option<Instant> = None;

    loop {
        if active.is_some() && session.as_ref().is_some_and(|value| value.sink.empty()) {
            if let Some(completed) = active.take() {
                let _ = completed.reply.send(Ok(()));
                keepalive_run_id = Some(completed.run_id);
                idle_deadline = Some(Instant::now() + AUDIO_OUTPUT_IDLE_TIMEOUT);
            }
        }

        if idle_deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            session = None;
            keepalive_run_id = None;
            idle_deadline = None;
        }

        let wait = if active.is_some() {
            ACTIVE_POLL_INTERVAL
        } else if let Some(deadline) = idle_deadline {
            deadline.saturating_duration_since(Instant::now())
        } else {
            Duration::from_secs(60)
        };

        let command = match receiver.recv_timeout(wait) {
            Ok(command) => command,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => break,
        };

        match command {
            AudioOutputCommand::Prepare { run_id, reply } => {
                if prepared_run_id.is_some() || active.is_some() {
                    let _ = reply.send(Err("Another speech run is already active".to_string()));
                    continue;
                }
                if session.is_none() {
                    match OutputSession::open() {
                        Ok(opened) => session = Some(opened),
                        Err(error) => {
                            let _ = reply.send(Err(error));
                            continue;
                        }
                    }
                }
                prepared_run_id = Some(run_id.clone());
                keepalive_run_id = Some(run_id);
                idle_deadline = None;
                let _ = reply.send(Ok(()));
            }
            AudioOutputCommand::Play {
                run_id,
                audio,
                reply,
            } => {
                if prepared_run_id.as_deref() != Some(run_id.as_str()) {
                    let _ = reply.send(Err(
                        "TTS audio output was not prepared for this run".to_string()
                    ));
                    continue;
                }
                let file = match File::open(audio.path()) {
                    Ok(file) => file,
                    Err(error) => {
                        prepared_run_id = None;
                        idle_deadline = Some(Instant::now() + AUDIO_OUTPUT_IDLE_TIMEOUT);
                        let _ =
                            reply.send(Err(format!("Could not open rendered TTS audio: {error}")));
                        continue;
                    }
                };
                let decoded = match Decoder::try_from(file) {
                    Ok(decoded) => decoded,
                    Err(error) => {
                        prepared_run_id = None;
                        idle_deadline = Some(Instant::now() + AUDIO_OUTPUT_IDLE_TIMEOUT);
                        let _ = reply
                            .send(Err(format!("Could not decode rendered TTS audio: {error}")));
                        continue;
                    }
                };
                let Some(output) = session.as_ref() else {
                    prepared_run_id = None;
                    let _ = reply.send(Err("TTS audio output session was lost".to_string()));
                    continue;
                };
                output.sink.append(decoded);
                prepared_run_id = None;
                keepalive_run_id = Some(run_id.clone());
                active = Some(ActivePlayback { run_id, reply });
                idle_deadline = None;
                drop(audio.directory);
            }
            AudioOutputCommand::Cancel { run_id } => {
                let owns_output = prepared_run_id.as_deref() == Some(run_id.as_str())
                    || active.as_ref().is_some_and(|value| value.run_id == run_id)
                    || keepalive_run_id.as_deref() == Some(run_id.as_str());
                if !owns_output {
                    continue;
                }
                if let Some(output) = session.as_ref() {
                    output.sink.stop();
                }
                if let Some(cancelled) = active.take() {
                    let _ = cancelled.reply.send(Err("Speech cancelled".to_string()));
                }
                prepared_run_id = None;
                keepalive_run_id = None;
                idle_deadline = None;
                session = None;
            }
            AudioOutputCommand::Stop => {
                if let Some(output) = session.as_ref() {
                    output.sink.stop();
                }
                if let Some(cancelled) = active.take() {
                    let _ = cancelled.reply.send(Err("Speech cancelled".to_string()));
                }
                prepared_run_id = None;
                keepalive_run_id = None;
                idle_deadline = None;
                session = None;
            }
            AudioOutputCommand::Shutdown => break,
        }
    }

    if let Some(cancelled) = active {
        let _ = cancelled
            .reply
            .send(Err("TTS audio output stopped during playback".to_string()));
    }
}

#[cfg(test)]
mod tests {
    use super::AUDIO_OUTPUT_IDLE_TIMEOUT;
    use std::time::Duration;

    #[test]
    fn keeps_audio_output_open_for_five_seconds() {
        assert_eq!(AUDIO_OUTPUT_IDLE_TIMEOUT, Duration::from_millis(5_000));
    }
}
