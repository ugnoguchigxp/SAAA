use super::decode::Format;
use crate::RunCancellation;
use rodio::Source;
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

pub(super) type Packet = (Format, Vec<i16>);

pub(super) struct Playback {
    pub(super) sender: tokio::sync::mpsc::Sender<Packet>,
    done: tokio::sync::oneshot::Receiver<Result<(), String>>,
    stopped: Arc<AtomicBool>,
}
impl Playback {
    pub(super) fn start(
        cancellation: Arc<RunCancellation>,
        on_started: impl FnOnce() + Send + 'static,
    ) -> Self {
        let (sender, mut receiver) = tokio::sync::mpsc::channel::<Packet>(4);
        let (done_sender, done) = tokio::sync::oneshot::channel();
        let stopped = Arc::new(AtomicBool::new(false));
        let stop = stopped.clone();
        std::thread::spawn(move || {
            let result = (|| {
                let (_stream, handle) = rodio::OutputStream::try_default()
                    .map_err(|_| "Could not open the audio output device".to_string())?;
                let sink = rodio::Sink::try_new(&handle)
                    .map_err(|_| "Could not create audio playback".to_string())?;
                let mut started = Some(Box::new(on_started) as Box<dyn FnOnce() + Send>);
                let mut closed = false;
                loop {
                    if cancellation.is_cancelled() || stop.load(Ordering::Acquire) {
                        sink.stop();
                        return Err("Speech cancelled".into());
                    }
                    if sink.len() < 3 && !closed {
                        match receiver.try_recv() {
                            Ok((format, samples)) => sink.append(Started {
                                source: rodio::buffer::SamplesBuffer::new(
                                    format.channels,
                                    format.rate,
                                    samples,
                                ),
                                callback: started.take(),
                            }),
                            Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                                closed = true
                            }
                            Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {}
                        }
                    }
                    if closed && sink.empty() {
                        return Ok(());
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
            })();
            let _ = done_sender.send(result);
        });
        Self {
            sender,
            done,
            stopped,
        }
    }
    pub(super) async fn finish(mut self) -> Result<(), String> {
        let (replacement, _) = tokio::sync::mpsc::channel(1);
        drop(std::mem::replace(&mut self.sender, replacement));
        (&mut self.done)
            .await
            .map_err(|_| "Audio playback worker disconnected".to_string())?
    }
}
impl Drop for Playback {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
    }
}
struct Started {
    source: rodio::buffer::SamplesBuffer<i16>,
    callback: Option<Box<dyn FnOnce() + Send>>,
}
impl Iterator for Started {
    type Item = i16;
    fn next(&mut self) -> Option<i16> {
        let sample = self.source.next();
        if sample.is_some() {
            if let Some(callback) = self.callback.take() {
                callback();
            }
        }
        sample
    }
}
impl Source for Started {
    fn current_frame_len(&self) -> Option<usize> {
        self.source.current_frame_len()
    }
    fn channels(&self) -> u16 {
        self.source.channels()
    }
    fn sample_rate(&self) -> u32 {
        self.source.sample_rate()
    }
    fn total_duration(&self) -> Option<Duration> {
        self.source.total_duration()
    }
}
