use super::TurnEventHub;
use crate::{ipc_contract::RuntimeEvent, AppState, RunCancellation};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};

fn pending_speech() -> &'static Mutex<HashMap<String, ()>> {
    static PENDING: OnceLock<Mutex<HashMap<String, ()>>> = OnceLock::new();
    PENDING.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(crate) fn mark_speech(speech_id: &str, playing: bool) {
    let mut pending = pending_speech()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if playing {
        pending.insert(speech_id.to_string(), ());
    } else {
        pending.remove(speech_id);
    }
}

fn spoken_ack() -> &'static Mutex<HashMap<String, String>> {
    static SPOKEN: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    SPOKEN.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(in crate::runtime) fn completion_already_spoken(run_id: &str, text: &str) -> bool {
    spoken_ack()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(run_id)
        .is_some_and(|spoken| spoken == text.trim())
}

fn remember_ack(run_id: &str, speech_id: &str, text: &str) {
    if speech_id != format!("{run_id}_ack") {
        return;
    }
    spoken_ack()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(run_id.to_string(), text.trim().to_string());
}

#[cfg(test)]
fn clear_after() -> &'static Mutex<(String, u32)> {
    static CLEAR: OnceLock<Mutex<(String, u32)>> = OnceLock::new();
    CLEAR.get_or_init(|| Mutex::new((String::new(), 0)))
}

#[cfg(test)]
pub(crate) fn test_clear_speech_at_tick(run_id: &str, tick: u32) {
    *clear_after()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = (run_id.to_string(), tick);
}

#[cfg(test)]
pub(crate) fn observe_filler_tick(run_id: &str, tick: u32) {
    let clear = clear_after()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    if clear.0 == run_id && clear.1 == tick {
        mark_speech(&format!("{run_id}_ack"), false);
    }
}

pub(crate) fn speech_still_playing(run_id: &str) -> bool {
    let pending = pending_speech()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let prefix = format!("{run_id}_");
    pending
        .keys()
        .any(|speech_id| speech_id == run_id || speech_id.starts_with(&prefix))
}

pub(in crate::runtime) async fn speak(
    hub: &TurnEventHub,
    state: &AppState,
    run_id: &str,
    conversation_id: &str,
    cancellation: Arc<RunCancellation>,
) {
    // Voice conversation acknowledgements belong to the LFM front desk, not Qwen's timer.
    let delegated_voice = state
        .sqlite_readers
        .read(|c| {
            c.query_row(
                "SELECT EXISTS(SELECT 1 FROM lfm_voice_utterances WHERE claimed_run_id=?1)",
                [run_id],
                |r| r.get::<_, bool>(0),
            )
            .map_err(crate::database_error)
        })
        .unwrap_or(false);
    if delegated_voice {
        return;
    }
    if !hub.streaming_speech || cancellation.is_cancelled() {
        return;
    }
    let (presentation, _) = crate::voice_behavior::completion_state(state, run_id, conversation_id);
    if presentation.decision != "speak" {
        return;
    }
    let language = state
        .sqlite_readers
        .read(|connection| {
            Ok(crate::persistence::settings::regional_preferences::load(connection)?.language)
        })
        .unwrap_or_else(|_| "ja".into());
    let acknowledgement = if language == "en" {
        "Let me check."
    } else {
        "確認します。"
    };
    speak_text(
        hub,
        state,
        run_id,
        conversation_id,
        format!("{run_id}_ack"),
        acknowledgement.to_string(),
        true,
        cancellation,
    )
    .await;
}

pub(in crate::runtime) async fn speak_text(
    hub: &TurnEventHub,
    state: &AppState,
    run_id: &str,
    conversation_id: &str,
    speech_id: String,
    text: String,
    wait_end: bool,
    cancellation: Arc<RunCancellation>,
) {
    if !hub.streaming_speech || cancellation.is_cancelled() || text.is_empty() {
        return;
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    let ack_id = speech_id;
    let (ended_tx, mut ended_rx) = tokio::sync::oneshot::channel();
    let ended_tx = Arc::new(Mutex::new(Some(ended_tx)));
    let parent_id = run_id.to_string();
    let ended_speech_id = ack_id.clone();
    let ui = hub.ui.clone();
    let parent_cancellation = cancellation.clone();
    let channel = tauri::ipc::Channel::<RuntimeEvent>::new(move |body| {
        if let tauri::ipc::InvokeResponseBody::Json(value) = body {
            let event: serde_json::Value = serde_json::from_str(&value)
                .map_err(|e| tauri::Error::Io(std::io::Error::other(e)))?;
            match event["type"].as_str() {
                Some("speechStarted") if !parent_cancellation.is_cancelled() => {
                    let _ = ui.send(RuntimeEvent::SpeechStarted {
                        run_id: parent_id.clone(),
                    });
                }
                Some("speechEnded") => {
                    mark_speech(&ended_speech_id, false);
                    let _ = ui.send(RuntimeEvent::SpeechEnded {
                        run_id: parent_id.clone(),
                    });
                    if let Ok(mut tx) = ended_tx.lock() {
                        if let Some(tx) = tx.take() {
                            let _ = tx.send(());
                        }
                    }
                }
                // A failed acknowledgement must not fail a valid reasoning answer.
                _ => {}
            }
        }
        Ok(())
    });
    let begin = hub
        .speech
        .begin(state, &ack_id, true, channel, Some(conversation_id));
    let ready = prepare_before_deadline(begin, &cancellation, deadline).await;
    if !ready {
        hub.speech.cancel(&ack_id);
        return;
    }
    if cancellation.is_cancelled() || hub.speech.finish(&ack_id, &text).is_err() {
        hub.speech.cancel(&ack_id);
        return;
    }
    mark_speech(&ack_id, true);
    remember_ack(run_id, &ack_id, &text);
    if !wait_end {
        let speech_id = ack_id.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(30)).await;
            mark_speech(&speech_id, false);
        });
        return;
    }
    let ended = tokio::select! { biased;
        _ = cancellation.cancelled() => false,
        _ = &mut ended_rx => true, // The channel closing also means no further child events can arrive.
        _ = tokio::time::sleep_until(deadline) => false,
    };
    if !ended {
        mark_speech(&ack_id, false);
        hub.speech.cancel(&ack_id);
        // Don't allow an old SpeechEnded event to race the answer's SpeechStarted.
        if tokio::time::timeout(Duration::from_millis(300), &mut ended_rx)
            .await
            .is_err()
        {
            hub.speech.cancel(run_id);
        }
    }
}

async fn prepare_before_deadline(
    begin: impl std::future::Future<Output = Result<(), String>>,
    cancellation: &RunCancellation,
    deadline: tokio::time::Instant,
) -> bool {
    tokio::select! { biased;
        _ = cancellation.cancelled() => false,
        result = tokio::time::timeout_at(deadline, begin) => matches!(result, Ok(Ok(()))),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn acknowledgement_setup_is_bounded_and_cancelled_with_its_parent() {
        let cancellation = RunCancellation::default();
        assert!(
            !prepare_before_deadline(
                std::future::pending(),
                &cancellation,
                tokio::time::Instant::now()
            )
            .await
        );
        cancellation.cancel();
        assert!(
            !prepare_before_deadline(
                std::future::pending(),
                &cancellation,
                tokio::time::Instant::now() + Duration::from_secs(60)
            )
            .await
        );
    }
}
