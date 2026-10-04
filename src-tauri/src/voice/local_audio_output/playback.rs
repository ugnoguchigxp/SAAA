#[path = "playback/worker.rs"]
mod worker;
use super::playback_started::Started;
use crate::voice::http_audio::decode::Format;
use crate::RunCancellation;
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

pub(crate) type Packet = (Format, Vec<i16>);

pub(crate) struct Playback {
    pub(crate) sender: tokio::sync::mpsc::Sender<Packet>,
    barriers: tokio::sync::mpsc::Sender<tokio::sync::oneshot::Sender<()>>,
    done: tokio::sync::oneshot::Receiver<Result<(), String>>,
    stopped: Arc<AtomicBool>,
}

struct SpeechActivityGuard;

impl Drop for SpeechActivityGuard {
    fn drop(&mut self) {
        super::set_speaking(false);
    }
}

impl Playback {
    pub(crate) fn start_guarded(
        cancellation: Arc<RunCancellation>,
        situation: Option<Arc<crate::situation::SituationRuntime>>,
        on_started: impl FnOnce() + Send + 'static,
    ) -> Self {
        Self::spawn(cancellation, situation, on_started, false)
    }

    pub(crate) fn start_continuous(
        cancellation: Arc<RunCancellation>,
        on_started: impl FnOnce() + Send + 'static,
    ) -> Self {
        Self::spawn(cancellation, None, on_started, false)
    }

    #[cfg(feature = "conversation-queue-e2e")]
    pub(crate) fn start_vpio_handoff_probe(
        cancellation: Arc<RunCancellation>,
        on_started: impl FnOnce() + Send + 'static,
    ) -> Self {
        Self::spawn(cancellation, None, on_started, true)
    }

    pub(crate) async fn drain(&self, cancellation: &RunCancellation) -> Result<(), String> {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        tokio::select! { biased;
            _ = cancellation.cancelled() => return Err("Speech cancelled".into()),
            result = self.barriers.send(sender) => result.map_err(|_| "Audio playback worker disconnected".to_string())?,
        }
        tokio::select! { biased;
            _ = cancellation.cancelled() => Err("Speech cancelled".into()),
            result = receiver => result.map_err(|_| "Audio playback worker disconnected".to_string()),
        }
    }
    pub(crate) async fn finish(mut self) -> Result<(), String> {
        let (replacement, _) = tokio::sync::mpsc::channel(1);
        drop(std::mem::replace(&mut self.sender, replacement));
        (&mut self.done)
            .await
            .map_err(|_| "Audio playback worker disconnected".to_string())?
    }
}

#[path = "playback/rodio_output.rs"]
mod rodio_output;
use rodio_output::play_through_rodio;

impl Drop for Playback {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        crate::voice::audio_backend::global().interrupt_playback();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rodio::Source;
    use std::sync::atomic::AtomicBool;
    impl Playback {
        #[cfg(test)]
        pub(crate) fn start(
            cancellation: Arc<RunCancellation>,
            on_started: impl FnOnce() + Send + 'static,
        ) -> Self {
            Self::start_guarded(cancellation, None, on_started)
        }
    }

    #[test]
    fn started_source_forwards_samples_and_fires_the_callback_once() {
        let fired = Arc::new(AtomicBool::new(false));
        let flag = fired.clone();
        let source = rodio::buffer::SamplesBuffer::new(1, 24_000, vec![7_i16, 8]);
        let expected_frame_len = source.current_frame_len();
        let mut started = Started {
            source,
            callback: Some(Box::new(move || flag.store(true, Ordering::SeqCst))),
        };
        assert_eq!(started.channels(), 1);
        assert_eq!(started.sample_rate(), 24_000);
        assert_eq!(started.current_frame_len(), expected_frame_len);
        assert!(started.total_duration().is_some());
        assert_eq!(started.next(), Some(7));
        assert!(fired.load(Ordering::SeqCst));
        assert_eq!(started.next(), Some(8));
        assert_eq!(started.next(), None);
    }
}
