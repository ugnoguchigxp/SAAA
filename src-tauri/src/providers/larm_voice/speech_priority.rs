//! A completed answer invalidates pending LFM speech and preempts its active player.
use std::{collections::HashMap, sync::{LazyLock, Mutex}};
use crate::voice::streaming_tts::runtime::StreamingSpeechRuntime;

#[derive(Default)]
struct ConversationSpeech {
    epoch: u64,
    lfm_run: Option<String>,
    final_run: Option<String>,
}
static SPEECH: LazyLock<Mutex<HashMap<String, ConversationSpeech>>> = LazyLock::new(Mutex::default);

pub(crate) fn epoch(conversation: &str) -> u64 {
    SPEECH.lock().unwrap_or_else(|e|e.into_inner()).entry(conversation.into()).or_default().epoch
}

pub(crate) fn final_ready(speech: &StreamingSpeechRuntime, conversation: &str, run: &str, spoken: bool) {
    let mut all = SPEECH.lock().unwrap_or_else(|e|e.into_inner());
    let state = all.entry(conversation.into()).or_default();
    state.epoch += 1;
    let previous_final = state.final_run.take();
    state.final_run = spoken.then(||run.into());
    if let Some(previous) = previous_final.filter(|previous|previous != run) {speech.cancel(&previous);}
    if let Some(ack) = state.lfm_run.take() { speech.cancel(&ack); }
}

pub(crate) fn finished(run: &str) {
    let mut all = SPEECH.lock().unwrap_or_else(|e|e.into_inner());
    for state in all.values_mut() {
        if state.final_run.as_deref() == Some(run) { state.final_run = None; }
        if state.lfm_run.as_deref() == Some(run) { state.lfm_run = None; }
    }
}

pub(crate) fn queue_reply(speech: &StreamingSpeechRuntime, conversation: &str, run: &str, reply: &str, expected_epoch: u64) -> Result<bool,String> {
    let mut all = SPEECH.lock().unwrap_or_else(|e|e.into_inner());
    let state = all.entry(conversation.into()).or_default();
    if state.epoch != expected_epoch || state.final_run.is_some() {
        speech.cancel(run);
        return Ok(false);
    }
    if let Some(previous) = state.lfm_run.replace(run.into()) { speech.cancel(&previous); }
    speech.finish(run, reply)?;
    Ok(true)
}

pub(crate) fn stop_conversation(speech: &StreamingSpeechRuntime, conversation: &str) {
    let mut all = SPEECH.lock().unwrap_or_else(|e|e.into_inner());
    if let Some(state) = all.get_mut(conversation) {
        state.epoch += 1;
        if let Some(run) = state.lfm_run.take() { speech.cancel(&run); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn final_invalidates_inflight_reply_and_holds_priority_until_playback_ends() {
        let speech = StreamingSpeechRuntime::default();
        let conversation = "priority-test";
        let old = epoch(conversation);
        final_ready(&speech,conversation,"final",true);
        assert!(!queue_reply(&speech,conversation,"late","ack",old).unwrap());
        let next = epoch(conversation);
        assert!(!queue_reply(&speech,conversation,"new","ack",next).unwrap());
        finished("final");
        assert!(!queue_reply(&speech,conversation,"old","ack",old).unwrap());
        assert!(queue_reply(&speech,conversation,"fresh","ack",next).unwrap());
    }
}
