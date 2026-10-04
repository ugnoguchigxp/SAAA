use super::*;

pub(super) fn play_through_rodio(
    receiver: &mut tokio::sync::mpsc::Receiver<Packet>,
    barrier_receiver: &mut tokio::sync::mpsc::Receiver<tokio::sync::oneshot::Sender<()>>,
    cancellation: &RunCancellation,
    situation: Option<&Arc<crate::situation::SituationRuntime>>,
    stop: &AtomicBool,
    started: &mut Option<Box<dyn FnOnce() + Send>>,
    fallback: (Option<Packet>, Vec<tokio::sync::oneshot::Sender<()>>),
) -> Result<(), String> {
    let (mut first_packet, mut pending_barriers) = fallback;
    let (_stream, handle) = rodio::OutputStream::try_default()
        .map_err(|_| "Could not open the audio output device".to_string())?;
    let sink =
        rodio::Sink::try_new(&handle).map_err(|_| "Could not create audio playback".to_string())?;
    let mut closed = false;
    loop {
        if cancellation.is_cancelled()
            || stop.load(Ordering::Acquire)
            || situation.is_some_and(|s| crate::situation::speech_holds_runtime(s))
        {
            sink.stop();
            return Err("Speech cancelled".into());
        }
        if sink.len() < 3 && !closed {
            let packet = first_packet
                .take()
                .map(Ok)
                .unwrap_or_else(|| receiver.try_recv());
            match packet {
                Ok((format, samples)) => {
                    let callback = started.take();
                    sink.append(Started {
                        source: rodio::buffer::SamplesBuffer::new(
                            format.channels,
                            format.rate,
                            samples,
                        ),
                        callback: Some(Box::new(move || {
                            super::super::set_speaking(true);
                            if let Some(callback) = callback {
                                callback();
                            }
                        })),
                    });
                }
                Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => closed = true,
                Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {}
            }
        }
        while let Ok(barrier) = barrier_receiver.try_recv() {
            pending_barriers.push(barrier);
        }
        if sink.empty() {
            super::super::set_speaking(false);
        }
        if receiver.is_empty() && sink.empty() {
            for barrier in pending_barriers.drain(..) {
                let _ = barrier.send(());
            }
            if closed {
                return Ok(());
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}
