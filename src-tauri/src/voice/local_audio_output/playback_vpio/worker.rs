use super::*;

pub(in crate::voice::output::local_audio_output) fn play_through_vpio(
    receiver: &mut tokio::sync::mpsc::Receiver<Packet>,
    barriers: &mut tokio::sync::mpsc::Receiver<tokio::sync::oneshot::Sender<()>>,
    cancellation: &Arc<RunCancellation>,
    situation: Option<&Arc<crate::situation::SituationRuntime>>,
    stop: &Arc<AtomicBool>,
    started: &mut Option<Box<dyn FnOnce() + Send>>,
) -> Result<VpioExit, String> {
    let mut closed = false;
    let mut pending_barriers = Vec::new();
    loop {
        if cancellation.is_cancelled()
            || stop.load(Ordering::Acquire)
            || situation.is_some_and(|s| crate::situation::speech_holds_runtime(s))
        {
            crate::voice::audio_backend::global().interrupt_playback();
            return Err("Speech cancelled".into());
        }
        if !crate::voice::audio_backend::global().is_capturing() {
            crate::voice::audio_backend::global().wait_playback_drained(cancellation)?;
            return Ok(VpioExit::CaptureEnded(None, pending_barriers));
        }
        if !closed {
            match receiver.try_recv() {
                Ok((format, samples)) => {
                    if !crate::voice::audio_backend::queue_tts_packet(
                        format.rate,
                        format.channels,
                        &samples,
                    ) {
                        crate::voice::audio_backend::global().interrupt_playback();
                        if !crate::voice::audio_backend::global().is_capturing() {
                            return Ok(VpioExit::CaptureEnded(
                                Some((format, samples)),
                                pending_barriers,
                            ));
                        }
                        return Err("VoiceProcessing playback is unavailable".into());
                    }
                    super::super::set_speaking(true);
                    if let Some(callback) = started.take() {
                        callback();
                    }
                }
                Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => closed = true,
                Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {}
            }
        }
        while let Ok(barrier) = barriers.try_recv() {
            pending_barriers.push(barrier);
        }
        if receiver.is_empty() && !pending_barriers.is_empty() {
            crate::voice::audio_backend::global().wait_playback_drained(cancellation)?;
            if !crate::voice::audio_backend::global().is_capturing() {
                return Ok(VpioExit::CaptureEnded(None, pending_barriers));
            }
            super::super::set_speaking(false);
            for barrier in pending_barriers.drain(..) {
                let _ = barrier.send(());
            }
        }
        if closed {
            crate::voice::audio_backend::global().wait_playback_drained(cancellation)?;
            super::super::set_speaking(false);
            if !crate::voice::audio_backend::global().is_capturing() {
                return Ok(VpioExit::CaptureEnded(None, pending_barriers));
            }
            return Ok(VpioExit::Completed);
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}
