use super::*;

impl Playback {
    pub(super) fn spawn(
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
                    match super::super::playback_vpio::play_through_vpio(
                        &mut receiver,
                        &mut barrier_receiver,
                        &cancellation,
                        situation.as_ref(),
                        &stop,
                        &mut started,
                    )? {
                        super::super::playback_vpio::VpioExit::Completed => return Ok(()),
                        super::super::playback_vpio::VpioExit::CaptureEnded(packet, barriers) => {
                            return play_through_rodio(
                                &mut receiver,
                                &mut barrier_receiver,
                                &cancellation,
                                situation.as_ref(),
                                &stop,
                                &mut started,
                                (packet, barriers),
                            );
                        }
                    }
                }
                play_through_rodio(
                    &mut receiver,
                    &mut barrier_receiver,
                    &cancellation,
                    situation.as_ref(),
                    &stop,
                    &mut started,
                    (None, Vec::new()),
                )
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
}
