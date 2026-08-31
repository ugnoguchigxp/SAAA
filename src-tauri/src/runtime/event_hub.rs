#[path = "performance.rs"]
pub(crate) mod performance;

use std::time::{Duration, Instant};
use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, Weak,
    },
};
use tokio::sync::Notify;

use crate::{ipc_contract::RuntimeEvent, voice::streaming_tts::runtime::StreamingSpeechRuntime};

const UI_DELTA_FLUSH_INTERVAL: Duration = Duration::from_millis(16);

#[derive(Default)]
struct UiQueueState {
    events: VecDeque<RuntimeEvent>,
}

struct UiQueue {
    state: Mutex<UiQueueState>,
    notify: Arc<Notify>,
    failed: AtomicBool,
}

impl Drop for UiQueue {
    fn drop(&mut self) {
        self.notify.notify_waiters();
    }
}

pub(crate) trait RuntimeEventSender: Send + Sync {
    fn send(&self, event: RuntimeEvent) -> tauri::Result<()>;
    fn send_received(&self, event: RuntimeEvent, _received_at: Instant) -> tauri::Result<()> {
        self.send(event)
    }
    fn clone_box(&self) -> Box<dyn RuntimeEventSender>;
}

impl RuntimeEventSender for tauri::ipc::Channel<RuntimeEvent> {
    fn send(&self, event: RuntimeEvent) -> tauri::Result<()> {
        tauri::ipc::Channel::send(self, event)
    }

    fn clone_box(&self) -> Box<dyn RuntimeEventSender> {
        Box::new(self.clone())
    }
}

/// The process-local fan-out for one conversation turn. Speech receives typed
/// deltas before UI delivery, so WebView stalls cannot delay sentence scanning.
#[derive(Clone)]
pub(crate) struct TurnEventHub {
    speech: StreamingSpeechRuntime,
    streaming_speech: bool,
    ui_queue: Arc<UiQueue>,
}

impl TurnEventHub {
    pub(crate) fn new(
        ui: tauri::ipc::Channel<RuntimeEvent>,
        speech: StreamingSpeechRuntime,
        streaming_speech: bool,
    ) -> Self {
        let ui_queue = Arc::new(UiQueue {
            state: Mutex::new(UiQueueState::default()),
            notify: Arc::new(Notify::new()),
            failed: AtomicBool::new(false),
        });
        Self::spawn_ui_delivery(ui, Arc::downgrade(&ui_queue));
        Self {
            speech,
            streaming_speech,
            ui_queue,
        }
    }

    fn spawn_ui_delivery(ui: tauri::ipc::Channel<RuntimeEvent>, queue: Weak<UiQueue>) {
        let Some(notify) = queue.upgrade().map(|queue| queue.notify.clone()) else {
            return;
        };
        tauri::async_runtime::spawn(async move {
            loop {
                let notified = notify.notified();
                let Some(shared) = queue.upgrade() else { break };
                let empty = shared
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .events
                    .is_empty();
                drop(shared);
                if empty {
                    notified.await;
                }
                loop {
                    let Some(shared) = queue.upgrade() else {
                        return;
                    };
                    let delta_pending = matches!(
                        shared
                            .state
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .events
                            .front(),
                        Some(RuntimeEvent::Delta { .. })
                    );
                    drop(shared);
                    if delta_pending {
                        tokio::time::sleep(UI_DELTA_FLUSH_INTERVAL).await;
                    }
                    let Some(shared) = queue.upgrade() else {
                        return;
                    };
                    let event = shared
                        .state
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .events
                        .pop_front();
                    let Some(event) = event else { break };
                    let is_delta = matches!(event, RuntimeEvent::Delta { .. });
                    if ui.send(event).is_err() {
                        shared.failed.store(true, Ordering::Release);
                        shared
                            .state
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .events
                            .clear();
                        return;
                    }
                    if is_delta {
                        performance::record_ui_batch();
                    }
                }
            }
        });
    }

    fn enqueue_ui(&self, event: RuntimeEvent) {
        let mut state = self
            .ui_queue
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let RuntimeEvent::Delta { run_id, text } = &event {
            if let Some(RuntimeEvent::Delta {
                run_id: pending_run,
                text: pending_text,
            }) = state.events.back_mut()
            {
                if pending_run == run_id {
                    pending_text.push_str(text);
                    return;
                }
            }
        }
        state.events.push_back(event);
        drop(state);
        self.ui_queue.notify.notify_one();
    }

    fn stop_speech_with_error(&self, run_id: &str, error: String) {
        self.speech.cancel(run_id);
        self.enqueue_ui(RuntimeEvent::SpeechFailed {
            run_id: run_id.to_string(),
            message: crate::redact_runtime_text(&error),
            recovery: "Check the speech provider and try another response.".to_string(),
        });
    }

    fn dispatch(&self, event: RuntimeEvent, received_at: Option<Instant>) -> tauri::Result<()> {
        if self.ui_queue.failed.load(Ordering::Acquire) {
            return Err(tauri::Error::Io(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "runtime event consumer disconnected",
            )));
        }
        let hub_accepted_at = Instant::now();
        if let Some(received_at) = received_at {
            performance::record_socket_to_hub(
                hub_accepted_at.saturating_duration_since(received_at),
            );
        }
        if self.streaming_speech {
            match &event {
                RuntimeEvent::Delta { run_id, text } => {
                    let tts_started_at = Instant::now();
                    match self.speech.append(run_id, text) {
                        Ok(outcome) => self.speech.schedule_idle(run_id, outcome.idle_generation),
                        Err(error) => self.stop_speech_with_error(run_id, error),
                    }
                    performance::record_hub_to_tts(tts_started_at.elapsed());
                }
                RuntimeEvent::MessageCompleted {
                    run_id,
                    message,
                    presentation,
                    ..
                } if presentation.decision == "speak" => {
                    if let Err(error) = self.speech.finish(run_id, &message.content) {
                        self.stop_speech_with_error(run_id, error);
                    }
                }
                RuntimeEvent::MessageCompleted { run_id, .. }
                | RuntimeEvent::Cancelled { run_id }
                | RuntimeEvent::Failed { run_id, .. } => self.speech.cancel(run_id),
                _ => {}
            }
        }
        self.enqueue_ui(event);
        Ok(())
    }
}

impl RuntimeEventSender for TurnEventHub {
    fn send(&self, event: RuntimeEvent) -> tauri::Result<()> {
        self.dispatch(event, None)
    }

    fn send_received(&self, event: RuntimeEvent, received_at: Instant) -> tauri::Result<()> {
        self.dispatch(event, Some(received_at))
    }

    fn clone_box(&self) -> Box<dyn RuntimeEventSender> {
        Box::new(self.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[tokio::test]
    async fn ten_thousand_deltas_coalesce_without_loss_or_terminal_reordering() {
        let events = Arc::new(Mutex::new(Vec::<String>::new()));
        let captured = events.clone();
        let channel = tauri::ipc::Channel::new(move |body| {
            if let tauri::ipc::InvokeResponseBody::Json(value) = body {
                captured.lock().expect("event lock").push(value);
            }
            Ok(())
        });
        let hub = TurnEventHub::new(channel, StreamingSpeechRuntime::default(), false);
        for index in 0..10_000 {
            hub.send(RuntimeEvent::Delta {
                run_id: "run_hub_fixture".to_string(),
                text: char::from(b'a' + (index % 26) as u8).to_string(),
            })
            .expect("delta queues");
        }
        hub.send(RuntimeEvent::Cancelled {
            run_id: "run_hub_fixture".to_string(),
        })
        .expect("terminal sends");

        tokio::time::sleep(Duration::from_millis(40)).await;
        let events = events.lock().expect("event lock");
        assert_eq!(events.len(), 2);
        assert!(events[0].contains("\"type\":\"delta\""));
        assert!(events[1].contains("\"type\":\"cancelled\""));
        let delta: serde_json::Value = serde_json::from_str(&events[0]).expect("delta JSON");
        let text = delta["text"].as_str().expect("delta text");
        assert_eq!(text.len(), 10_000);
        assert_eq!(text.as_bytes()[0], b'a');
        assert_eq!(text.as_bytes()[9_999], b'p');
    }

    #[tokio::test]
    async fn blocked_ui_keeps_only_an_in_flight_and_one_pending_delta_batch() {
        let (blocked_tx, blocked_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let release_rx = Arc::new(Mutex::new(release_rx));
        let delivered = Arc::new(Mutex::new(Vec::<String>::new()));
        let captured = delivered.clone();
        let channel = tauri::ipc::Channel::new(move |body| {
            if let tauri::ipc::InvokeResponseBody::Json(value) = body {
                let is_first = captured.lock().expect("delivery lock").is_empty();
                if is_first {
                    blocked_tx.send(()).expect("block signal");
                    release_rx
                        .lock()
                        .expect("release lock")
                        .recv()
                        .expect("release signal");
                }
                captured.lock().expect("delivery lock").push(value);
            }
            Ok(())
        });
        let hub = TurnEventHub::new(channel, StreamingSpeechRuntime::default(), false);
        hub.send(RuntimeEvent::Delta {
            run_id: "run_stall".into(),
            text: "a".into(),
        })
        .expect("first delta queues");
        blocked_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("UI delivery blocks");

        for _ in 0..9_999 {
            hub.send(RuntimeEvent::Delta {
                run_id: "run_stall".into(),
                text: "b".into(),
            })
            .expect("pending delta coalesces");
        }
        let started = Instant::now();
        hub.send(RuntimeEvent::Cancelled {
            run_id: "run_stall".into(),
        })
        .expect("terminal queues");
        assert!(started.elapsed() < Duration::from_millis(50));
        assert_eq!(
            hub.ui_queue
                .state
                .lock()
                .expect("UI queue lock")
                .events
                .len(),
            2
        );

        release_tx.send(()).expect("release UI");
        tokio::time::sleep(Duration::from_millis(40)).await;
        let delivered = delivered.lock().expect("delivery lock");
        assert_eq!(delivered.len(), 3);
        assert!(delivered[0].contains("\"type\":\"delta\""));
        assert!(delivered[1].contains("\"type\":\"delta\""));
        assert!(delivered[2].contains("\"type\":\"cancelled\""));
    }

    #[tokio::test]
    async fn failed_ui_delivery_is_reported_to_the_next_provider_event() {
        let channel = tauri::ipc::Channel::new(|_| {
            Err(tauri::Error::Io(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "closed fixture channel",
            )))
        });
        let hub = TurnEventHub::new(channel, StreamingSpeechRuntime::default(), false);
        hub.send(RuntimeEvent::Delta {
            run_id: "run_closed_ui".into(),
            text: "first".into(),
        })
        .expect("first event queues before asynchronous failure");

        tokio::time::timeout(Duration::from_secs(1), async {
            while !hub.ui_queue.failed.load(Ordering::Acquire) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("delivery failure becomes observable");

        let error = hub
            .send(RuntimeEvent::Delta {
                run_id: "run_closed_ui".into(),
                text: "second".into(),
            })
            .expect_err("later provider delivery fails closed");
        assert!(matches!(error, tauri::Error::Io(_)));
        assert!(hub
            .ui_queue
            .state
            .lock()
            .expect("UI queue lock")
            .events
            .is_empty());
    }
}
