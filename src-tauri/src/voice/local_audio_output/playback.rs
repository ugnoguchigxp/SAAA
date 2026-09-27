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

    fn spawn(
        cancellation: Arc<RunCancellation>,
        situation: Option<Arc<crate::situation::SituationRuntime>>,
        on_started: impl FnOnce() + Send + 'static,
        force_vpio: bool,
    ) -> Self {
        let (sender, mut receiver) = tokio::sync::mpsc::channel::<Packet>(4);
        let (barriers, mut barrier_receiver) =
            tokio::sync::mpsc::channel::<tokio::sync::oneshot::Sender<()>>(4);
        let (done_sender, done) = tokio::sync::oneshot::channel();
        let stopped = Arc::new(AtomicBool::new(false));
        let stop = stopped.clone();
        std::thread::spawn(move || {
            let _speech_activity = SpeechActivityGuard;
            let mut started: Option<Box<dyn FnOnce() + Send>> = Some(Box::new(on_started));
            let result = (|| {
                if force_vpio || crate::voice::audio_backend::global().is_capturing() {
                    match super::playback_vpio::play_through_vpio(
                        &mut receiver,
                        &mut barrier_receiver,
                        &cancellation,
                        situation.as_ref(),
                        &stop,
                        &mut started,
                    )? {
                        super::playback_vpio::VpioExit::Completed => return Ok(()),
                        super::playback_vpio::VpioExit::CaptureEnded(packet, barriers) => {
                            return play_through_rodio(
                                &mut receiver, &mut barrier_receiver, &cancellation,
                                situation.as_ref(), &stop, &mut started, packet, barriers,
                            );
                        }
                    }
                }
                play_through_rodio(&mut receiver, &mut barrier_receiver, &cancellation,
                    situation.as_ref(), &stop, &mut started, None, Vec::new())
            })();
            let _ = done_sender.send(result);
        });
        Self {
            sender,
            barriers,
            done,
            stopped,
        }
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

fn play_through_rodio(
    receiver: &mut tokio::sync::mpsc::Receiver<Packet>,
    barrier_receiver: &mut tokio::sync::mpsc::Receiver<tokio::sync::oneshot::Sender<()>>,
    cancellation: &RunCancellation,
    situation: Option<&Arc<crate::situation::SituationRuntime>>,
    stop: &AtomicBool,
    started: &mut Option<Box<dyn FnOnce() + Send>>,
    mut first_packet: Option<Packet>,
    mut pending_barriers: Vec<tokio::sync::oneshot::Sender<()>>,
) -> Result<(), String> {
    let (_stream, handle) = rodio::OutputStream::try_default()
        .map_err(|_| "Could not open the audio output device".to_string())?;
    let sink = rodio::Sink::try_new(&handle)
        .map_err(|_| "Could not create audio playback".to_string())?;
    let mut closed = false;
    loop {
        if cancellation.is_cancelled() || stop.load(Ordering::Acquire)
            || situation.is_some_and(|s| crate::situation::speech_holds_runtime(s)) {
            sink.stop();
            return Err("Speech cancelled".into());
        }
        if sink.len() < 3 && !closed {
            let packet = first_packet.take().map(Ok).unwrap_or_else(|| receiver.try_recv());
            match packet {
                Ok((format, samples)) => {
                    let callback = started.take();
                    sink.append(Started {
                        source: rodio::buffer::SamplesBuffer::new(format.channels, format.rate, samples),
                        callback: Some(Box::new(move || {
                            super::set_speaking(true);
                            if let Some(callback) = callback { callback(); }
                        })),
                    });
                }
                Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => closed = true,
                Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {}
            }
        }
        while let Ok(barrier) = barrier_receiver.try_recv() { pending_barriers.push(barrier); }
        if sink.empty() { super::set_speaking(false); }
        if receiver.is_empty() && sink.empty() {
            for barrier in pending_barriers.drain(..) { let _ = barrier.send(()); }
            if closed { return Ok(()); }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}
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
