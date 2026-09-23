//! Internal event sink for a role-routed provider step.
//!
//! Provider deltas are draft material. They must not reach the WebView or speech runtime before
//! the final answer and routing ledger commit in the same transaction. Safe activity may cross
//! the boundary, but its summary is host-authored and contains no model text.

use crate::ipc_contract::RuntimeEvent;
use crate::runtime::event_hub::RuntimeEventSender;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub(super) struct BufferedRoleStepSink {
    run_id: String,
    parent: Arc<dyn RuntimeEventSender>,
    draft: Arc<Mutex<String>>,
}

impl BufferedRoleStepSink {
    pub(super) fn new(run_id: String, parent: Box<dyn RuntimeEventSender>) -> Self {
        Self {
            run_id,
            parent: Arc::from(parent),
            draft: Arc::new(Mutex::new(String::new())),
        }
    }

    #[cfg(test)]
    fn draft(&self) -> String {
        self.draft
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn contract_error(message: &'static str) -> tauri::Error {
        tauri::Error::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            message,
        ))
    }
}

impl RuntimeEventSender for BufferedRoleStepSink {
    fn allows_intermediate_messages(&self) -> bool {
        false
    }

    fn send(&self, event: RuntimeEvent) -> tauri::Result<()> {
        match event {
            RuntimeEvent::Delta { run_id, text } => {
                if run_id != self.run_id {
                    return Err(Self::contract_error(
                        "role step delta belongs to another run",
                    ));
                }
                self.draft
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push_str(&text);
                Ok(())
            }
            RuntimeEvent::Activity { run_id, kind, .. } => {
                if run_id != self.run_id {
                    return Err(Self::contract_error(
                        "role step activity belongs to another run",
                    ));
                }
                self.parent.send(RuntimeEvent::Activity {
                    run_id,
                    kind,
                    summary: "A role-routing step is running.".into(),
                })
            }
            RuntimeEvent::MessageCommitted { .. }
            | RuntimeEvent::MessageCompleted { .. }
            | RuntimeEvent::SpeechStarted { .. }
            | RuntimeEvent::SpeechEnded { .. }
            | RuntimeEvent::SpeechFailed { .. }
            | RuntimeEvent::Cancelled { .. }
            | RuntimeEvent::Failed { .. } => Err(Self::contract_error(
                "role step attempted to emit a terminal or speech event",
            )),
            event => self.parent.send(event),
        }
    }

    fn clone_box(&self) -> Box<dyn RuntimeEventSender> {
        Box::new(self.clone())
    }

    // Drafts never own speech. The final committed answer is delivered by the outer turn.
    fn voice_response_enabled(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Default)]
    struct Capture(Arc<Mutex<Vec<RuntimeEvent>>>);

    impl RuntimeEventSender for Capture {
        fn send(&self, event: RuntimeEvent) -> tauri::Result<()> {
            self.0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(event);
            Ok(())
        }

        fn clone_box(&self) -> Box<dyn RuntimeEventSender> {
            Box::new(self.clone())
        }
    }

    #[test]
    fn rr_09_child_delta_is_buffered_and_activity_is_bodyless() {
        let capture = Capture::default();
        let sink = BufferedRoleStepSink::new("run".into(), Box::new(capture.clone()));
        sink.send(RuntimeEvent::Delta {
            run_id: "run".into(),
            text: "private draft".into(),
        })
        .expect("delta buffers");
        sink.send(RuntimeEvent::Activity {
            run_id: "run".into(),
            kind: "provider-progress".into(),
            summary: "private model summary".into(),
        })
        .expect("activity forwards");
        assert_eq!(sink.draft(), "private draft");
        let events = capture
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            RuntimeEvent::Activity { summary, .. }
                if summary == "A role-routing step is running."
        ));
    }

    #[test]
    fn role_step_cannot_commit_intermediate_assistant_text() {
        let capture = Capture::default();
        let sink = BufferedRoleStepSink::new("run".into(), Box::new(capture.clone()));
        assert!(!sink.allows_intermediate_messages());
        assert!(sink
            .send(RuntimeEvent::MessageCommitted {
                run_id: "run".into(),
                message: crate::ipc_contract::ConversationMessage {
                    parts: None,
                    id: "draft".into(),
                    conversation_id: "conversation".into(),
                    role: "assistant".into(),
                    content: "unreviewed draft".into(),
                    created_at: "1".into(),
                },
            })
            .is_err());
        assert!(capture.0.lock().unwrap().is_empty());
    }

    #[test]
    fn rr_09_child_cannot_emit_completion_or_speech() {
        let sink = BufferedRoleStepSink::new("run".into(), Box::new(Capture::default()));
        assert!(sink
            .send(RuntimeEvent::SpeechStarted {
                run_id: "run".into()
            })
            .is_err());
        assert!(sink
            .send(RuntimeEvent::Cancelled {
                run_id: "run".into()
            })
            .is_err());
    }
}
