use super::playback::Packet;
use crate::RunCancellation;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Duration;

pub(super) fn play_through_vpio(
    receiver: &mut tokio::sync::mpsc::Receiver<Packet>,
    cancellation: &Arc<RunCancellation>,
    situation: Option<&Arc<crate::situation::SituationRuntime>>,
    stop: &Arc<AtomicBool>,
    on_started: impl FnOnce() + Send + 'static,
) -> Result<(), String> {
    let mut started = Some(on_started);
    let mut closed = false;
    loop {
        if cancellation.is_cancelled()
            || stop.load(Ordering::Acquire)
            || situation.is_some_and(|s| crate::situation::speech_holds_runtime(s))
        {
            crate::voice::audio_backend::global().interrupt_playback();
            return Err("Speech cancelled".into());
        }
        if !closed {
            match receiver.try_recv() {
                Ok((format, samples)) => {
                    if let Some(callback) = started.take() {
                        callback();
                    }
                    if !crate::voice::audio_backend::queue_tts_packet(
                        format.rate,
                        format.channels,
                        &samples,
                    ) {
                        crate::voice::audio_backend::global().interrupt_playback();
                        if !crate::voice::audio_backend::global().is_capturing() {
                            return Err("Speech cancelled".into());
                        }
                        return Err("VoiceProcessing playback is unavailable".into());
                    }
                }
                Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => closed = true,
                Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {}
            }
        }
        if closed {
            crate::voice::audio_backend::global().wait_playback_drained(cancellation)?;
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}
