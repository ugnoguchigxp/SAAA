use super::TurnEventHub;
use crate::{ipc_contract::RuntimeEvent, AppState, RunCancellation};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

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
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    let ack_id = format!("{run_id}_ack");
    let (ended_tx, mut ended_rx) = tokio::sync::oneshot::channel();
    let ended_tx = Arc::new(Mutex::new(Some(ended_tx)));
    let parent_id = run_id.to_string();
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
    if cancellation.is_cancelled() || hub.speech.finish(&ack_id, acknowledgement).is_err() {
        hub.speech.cancel(&ack_id);
    }
    let ended = tokio::select! { biased;
        _ = cancellation.cancelled() => false,
        _ = &mut ended_rx => true, // The channel closing also means no further child events can arrive.
        _ = tokio::time::sleep_until(deadline) => false,
    };
    if !ended {
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
