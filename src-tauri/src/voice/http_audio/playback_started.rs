use rodio::Source;
use std::time::Duration;

pub(super) struct Started {
    pub(super) source: rodio::buffer::SamplesBuffer<i16>,
    pub(super) callback: Option<Box<dyn FnOnce() + Send>>,
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
