use super::contracts::{ActivityKind, TerminalStatus};
use serde_json::Value;
use std::collections::HashSet;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectedCodexEvent {
    AssistantDelta(String),
    Activity {
        kind: ActivityKind,
        label: String,
        summary: String,
        started: bool,
        meaningful: bool,
        arms_terminal_gap: bool,
    },
    AssistantOutputCompleted {
        text: Option<String>,
        arms_terminal_gap: bool,
    },
    Progress(ActivityKind),
    Terminal(TerminalStatus),
    PolicyViolation,
    ProviderError,
    Ignore,
}

pub struct CodexEventProjector {
    thread_id: String,
    turn_id: String,
    active_item_ids: HashSet<String>,
    assistant_output_completed: bool,
}

impl CodexEventProjector {
    pub fn new(thread_id: &str, turn_id: &str) -> Self {
        Self {
            thread_id: thread_id.to_string(),
            turn_id: turn_id.to_string(),
            active_item_ids: HashSet::new(),
            assistant_output_completed: false,
        }
    }

    pub fn project(&mut self, message: &Value) -> Result<ProjectedCodexEvent, ()> {
        if message.get("id").is_some() && message.get("method").is_some() {
            return Ok(ProjectedCodexEvent::PolicyViolation);
        }
        let method = message.get("method").and_then(Value::as_str);
        if method == Some("error") {
            return Ok(
                if has_explicit_foreign_scope(message, &self.thread_id, &self.turn_id) {
                    ProjectedCodexEvent::Ignore
                } else {
                    ProjectedCodexEvent::ProviderError
                },
            );
        }
        if !has_current_scope(message, &self.thread_id, &self.turn_id, method) {
            return Ok(ProjectedCodexEvent::Ignore);
        }
        match method {
            Some("turn/started") => Ok(ProjectedCodexEvent::Progress(ActivityKind::Other)),
            Some("item/agentMessage/delta") => {
                let Some(delta) = message
                    .pointer("/params/delta")
                    .and_then(Value::as_str)
                    .filter(|delta| !delta.is_empty())
                else {
                    return Ok(ProjectedCodexEvent::Ignore);
                };
                self.assistant_output_completed = false;
                Ok(ProjectedCodexEvent::AssistantDelta(delta.to_string()))
            }
            Some("item/started") | Some("item/completed") => self.project_item(message),
            Some("turn/completed") => {
                let status = match message
                    .pointer("/params/turn/status")
                    .and_then(Value::as_str)
                {
                    Some("completed") => TerminalStatus::Completed,
                    Some("interrupted") => TerminalStatus::Interrupted,
                    _ => TerminalStatus::Failed,
                };
                Ok(ProjectedCodexEvent::Terminal(status))
            }
            Some(_) | None => Ok(ProjectedCodexEvent::Ignore),
        }
    }

    fn project_item(&mut self, message: &Value) -> Result<ProjectedCodexEvent, ()> {
        let item = message.pointer("/params/item").ok_or(())?;
        let item_type = item.get("type").and_then(Value::as_str).ok_or(())?;
        if matches!(
            item_type,
            "fileChange" | "mcpToolCall" | "dynamicToolCall" | "webSearch"
        ) {
            return Ok(ProjectedCodexEvent::PolicyViolation);
        }
        let completed = message.get("method").and_then(Value::as_str) == Some("item/completed");
        let trackable = matches!(
            item_type,
            "agentMessage" | "commandExecution" | "reasoning" | "plan"
        );
        let item_id = trackable
            .then(|| item.get("id").and_then(Value::as_str))
            .flatten()
            .filter(|id| valid_item_id(id));
        let meaningful = match (completed, item_id) {
            (false, Some(id)) => self.active_item_ids.insert(id.to_string()),
            (true, Some(id)) => self.active_item_ids.remove(id),
            _ => false,
        };

        if item_type == "agentMessage" {
            if completed {
                if meaningful {
                    self.assistant_output_completed = true;
                }
                return Ok(ProjectedCodexEvent::AssistantOutputCompleted {
                    text: item.get("text").and_then(Value::as_str).map(str::to_string),
                    arms_terminal_gap: meaningful
                        && self.assistant_output_completed
                        && self.active_item_ids.is_empty(),
                });
            }
            if meaningful {
                self.assistant_output_completed = false;
            }
            return Ok(if meaningful {
                ProjectedCodexEvent::Progress(ActivityKind::Other)
            } else {
                ProjectedCodexEvent::Ignore
            });
        }
        if item_type == "userMessage" {
            return Ok(ProjectedCodexEvent::Ignore);
        }

        let (kind, label, summary) = match item_type {
            "commandExecution" => (
                ActivityKind::Command,
                "command".to_string(),
                crate::redact_runtime_text(
                    item.get("command")
                        .and_then(Value::as_str)
                        .unwrap_or("Command"),
                ),
            ),
            "reasoning" => (
                ActivityKind::Reasoning,
                "reasoning".to_string(),
                "Codex is reasoning".to_string(),
            ),
            "plan" => (
                ActivityKind::Plan,
                "plan".to_string(),
                "Codex updated its plan".to_string(),
            ),
            other if !other.is_empty() => (
                ActivityKind::Other,
                "activity".to_string(),
                crate::bounded_text(other, 80),
            ),
            _ => return Err(()),
        };
        Ok(ProjectedCodexEvent::Activity {
            kind,
            label,
            summary,
            started: !completed,
            meaningful: meaningful && kind != ActivityKind::Other,
            arms_terminal_gap: completed
                && meaningful
                && self.assistant_output_completed
                && self.active_item_ids.is_empty(),
        })
    }
}

fn valid_item_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 160
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
}

fn has_current_scope(
    message: &Value,
    thread_id: &str,
    turn_id: &str,
    method: Option<&str>,
) -> bool {
    let mut current_thread = message.pointer("/params/threadId").and_then(Value::as_str);
    let mut current_turn = message.pointer("/params/turnId").and_then(Value::as_str);
    if method == Some("turn/completed") {
        current_thread = current_thread.or_else(|| {
            message
                .pointer("/params/turn/threadId")
                .and_then(Value::as_str)
        });
        current_turn =
            current_turn.or_else(|| message.pointer("/params/turn/id").and_then(Value::as_str));
    }
    current_thread == Some(thread_id) && current_turn == Some(turn_id)
}

fn has_explicit_foreign_scope(message: &Value, thread_id: &str, turn_id: &str) -> bool {
    message
        .pointer("/params/threadId")
        .and_then(Value::as_str)
        .is_some_and(|value| value != thread_id)
        || message
            .pointer("/params/turnId")
            .and_then(Value::as_str)
            .is_some_and(|value| value != turn_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn foreign_and_unscoped_events_are_ignored_and_policy_items_are_rejected() {
        let mut projector = CodexEventProjector::new("thread", "turn");
        for message in [
            json!({"method":"item/agentMessage/delta","params":{"threadId":"thread","turnId":"other","delta":"late"}}),
            json!({"method":"item/agentMessage/delta","params":{"delta":"unscoped"}}),
        ] {
            assert_eq!(projector.project(&message), Ok(ProjectedCodexEvent::Ignore));
        }
        let policy = json!({"method":"item/started","params":{"threadId":"thread","turnId":"turn","item":{"id":"item_1","type":"fileChange"}}});
        assert_eq!(
            projector.project(&policy),
            Ok(ProjectedCodexEvent::PolicyViolation)
        );
    }

    #[test]
    fn duplicate_items_do_not_count_as_progress_or_arm_the_terminal_gap() {
        let mut projector = CodexEventProjector::new("thread", "turn");
        let started = json!({"method":"item/started","params":{"threadId":"thread","turnId":"turn","item":{"id":"message_1","type":"agentMessage"}}});
        assert_eq!(
            projector.project(&started),
            Ok(ProjectedCodexEvent::Progress(ActivityKind::Other))
        );
        assert_eq!(projector.project(&started), Ok(ProjectedCodexEvent::Ignore));
        let completed = json!({"method":"item/completed","params":{"threadId":"thread","turnId":"turn","item":{"id":"message_1","type":"agentMessage","text":"done"}}});
        assert_eq!(
            projector.project(&completed),
            Ok(ProjectedCodexEvent::AssistantOutputCompleted {
                text: Some("done".to_string()),
                arms_terminal_gap: true,
            })
        );
        assert_eq!(
            projector.project(&completed),
            Ok(ProjectedCodexEvent::AssistantOutputCompleted {
                text: Some("done".to_string()),
                arms_terminal_gap: false,
            })
        );
    }

    #[test]
    fn assistant_completion_is_not_a_terminal_event() {
        let mut projector = CodexEventProjector::new("thread", "turn");
        let started = json!({"method":"item/started","params":{"threadId":"thread","turnId":"turn","item":{"id":"message_1","type":"agentMessage"}}});
        projector.project(&started).expect("start projects");
        let message = json!({"method":"item/completed","params":{"threadId":"thread","turnId":"turn","item":{"id":"message_1","type":"agentMessage","text":"done"}}});
        assert!(matches!(
            projector.project(&message),
            Ok(ProjectedCodexEvent::AssistantOutputCompleted { .. })
        ));
    }

    #[test]
    fn assistant_completion_arms_gap_after_the_last_active_item_finishes() {
        let mut projector = CodexEventProjector::new("thread", "turn");
        for (id, item_type) in [
            ("message_1", "agentMessage"),
            ("command_1", "commandExecution"),
        ] {
            projector
                .project(&json!({"method":"item/started","params":{"threadId":"thread","turnId":"turn","item":{"id":id,"type":item_type}}}))
                .expect("item starts");
        }
        let assistant = projector
            .project(&json!({"method":"item/completed","params":{"threadId":"thread","turnId":"turn","item":{"id":"message_1","type":"agentMessage","text":"done"}}}))
            .expect("assistant completes");
        assert!(matches!(
            assistant,
            ProjectedCodexEvent::AssistantOutputCompleted {
                arms_terminal_gap: false,
                ..
            }
        ));
        let command = projector
            .project(&json!({"method":"item/completed","params":{"threadId":"thread","turnId":"turn","item":{"id":"command_1","type":"commandExecution"}}}))
            .expect("command completes");
        assert!(matches!(
            command,
            ProjectedCodexEvent::Activity {
                arms_terminal_gap: true,
                ..
            }
        ));
    }

    #[test]
    fn renewed_assistant_delta_disarms_a_previous_completion() {
        let mut projector = CodexEventProjector::new("thread", "turn");
        for message in [
            json!({"method":"item/started","params":{"threadId":"thread","turnId":"turn","item":{"id":"message_1","type":"agentMessage"}}}),
            json!({"method":"item/completed","params":{"threadId":"thread","turnId":"turn","item":{"id":"message_1","type":"agentMessage","text":"first"}}}),
            json!({"method":"item/agentMessage/delta","params":{"threadId":"thread","turnId":"turn","delta":"more"}}),
            json!({"method":"item/started","params":{"threadId":"thread","turnId":"turn","item":{"id":"command_1","type":"commandExecution"}}}),
        ] {
            projector.project(&message).expect("event projects");
        }
        let command = projector
            .project(&json!({"method":"item/completed","params":{"threadId":"thread","turnId":"turn","item":{"id":"command_1","type":"commandExecution"}}}))
            .expect("command completes");
        assert!(matches!(
            command,
            ProjectedCodexEvent::Activity {
                arms_terminal_gap: false,
                ..
            }
        ));
    }
}
