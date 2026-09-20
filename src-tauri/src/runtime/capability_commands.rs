//! Strict conversation commands for the restricted generation/inspection flow (plan 12.7).
//!
//! Only the exact saved user input is a command. Leading/trailing whitespace is trimmed, but a
//! newline, an extra argument, or a command inside a quote is not a command. Ordinary model or
//! tool output can never start one.

use serde_json::Value;
use std::sync::Arc;

use crate::generated_capabilities::generation::contracts::{GenerateInput, GenerationContext};
use crate::generated_capabilities::inspection::contracts::InspectionReceipt;
use crate::ipc_contract::RuntimeEvent;
use crate::runtime::event_hub::RuntimeEventSender;
use crate::{
    persist_conversation_success, AppState, RunCancellation, StartTurnInput, TurnExecutionFailure,
};

pub const COMMAND_PREFIX: &str = "/capability";
pub const MAX_DISPLAY_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CapabilityCommand {
    Generate {
        request_id: String,
    },
    Update {
        request_id: String,
        base_revision_id: String,
    },
    Inspect {
        call_id: String,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommandError {
    NotACommand,
    Malformed,
}

/// Parses the whole saved user input. `NotACommand` means the input is an ordinary message and
/// normal dispatch continues; `Malformed` means it looks like the command family but is invalid.
pub fn parse(input: &str) -> Result<CapabilityCommand, CommandError> {
    let trimmed = input.trim();
    // Only the whole word `/capability` may start a command; `/capabilityfoo` is an ordinary
    // message, not a malformed command.
    if trimmed != COMMAND_PREFIX && !trimmed.starts_with("/capability ") {
        return Err(CommandError::NotACommand);
    }
    if trimmed.contains('\n') || trimmed.contains('\r') {
        return Err(CommandError::Malformed);
    }
    let tokens = trimmed.split_whitespace().collect::<Vec<_>>();
    if tokens.len() < 2 || tokens[0] != COMMAND_PREFIX {
        return Err(CommandError::Malformed);
    }
    match tokens.as_slice() {
        ["/capability", "generate", request_id] if is_request_id(request_id) => {
            Ok(CapabilityCommand::Generate {
                request_id: (*request_id).to_string(),
            })
        }
        ["/capability", "update", request_id, base_revision_id]
            if is_request_id(request_id) && is_revision_id(base_revision_id) =>
        {
            Ok(CapabilityCommand::Update {
                request_id: (*request_id).to_string(),
                base_revision_id: (*base_revision_id).to_string(),
            })
        }
        ["/capability", "inspect", call_id] if is_call_id(call_id) => {
            Ok(CapabilityCommand::Inspect {
                call_id: (*call_id).to_string(),
            })
        }
        _ => Err(CommandError::Malformed),
    }
}

fn is_request_id(value: &str) -> bool {
    if value.is_empty() || value.len() > 64 {
        return false;
    }
    let mut bytes = value.bytes();
    match bytes.next() {
        Some(first) if first.is_ascii_lowercase() || first.is_ascii_digit() => {}
        _ => return false,
    }
    bytes.all(|byte| {
        byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_' || byte == b'-'
    })
}

fn is_revision_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

fn is_call_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

/// Result of turning an inspection receipt into an assistant message.
#[derive(Clone, Debug)]
pub struct InspectionDisplay {
    pub content: String,
    pub too_large: bool,
}

/// Renders the receipt as a persistent assistant message: a hash/report summary plus an escaped
/// TypeScript code block. No dedicated UI or arbitrary file-open IPC is involved. Over 64 KiB the
/// body is replaced by the summary and a fixed `artifact-too-large` marker.
pub fn render_inspection(receipt: &InspectionReceipt) -> Result<InspectionDisplay, CommandError> {
    let summary = format!(
        "Capability inspection\nrevision: {}\nsource: {}\nprogram: {}\nartifact: {}\nprojection: {}\ncomparison: {}\n",
        receipt.revision_id,
        receipt.source_hash,
        receipt.program_hash,
        receipt.artifact_hash,
        receipt.projection_hash,
        comparison_display(&receipt.comparison_json),
    );
    let fence = fence_for(&receipt.typescript_text);
    let body = format!(
        "{summary}\n```{fence}\n{}\n```{fence}\n",
        receipt.typescript_text
    );
    if body.len() > MAX_DISPLAY_BYTES {
        return Ok(InspectionDisplay {
            content: format!(
                "{summary}\nartifact-too-large: the TypeScript projection exceeds the display limit; the managed artifact is retained.\n"
            ),
            too_large: true,
        });
    }
    Ok(InspectionDisplay {
        content: body,
        too_large: false,
    })
}

fn comparison_display(comparison: &Value) -> String {
    let checked = comparison
        .get("checkedCases")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let mismatches = comparison
        .get("mismatches")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    let equivalence = comparison
        .get("semanticEquivalence")
        .and_then(Value::as_str)
        .unwrap_or("not-checked");
    format!("{checked} cases checked, {mismatches} mismatches, semanticEquivalence={equivalence}")
}

/// Chooses a fence longer than the longest backtick run inside the body.
pub fn fence_for(typescript: &str) -> String {
    let mut longest = 0usize;
    let mut current = 0usize;
    for character in typescript.chars() {
        if character == '`' {
            current += 1;
            longest = longest.max(current);
        } else {
            current = 0;
        }
    }
    "`".repeat(longest.max(2) + 1)
}

/// Intercepts a saved user message that is a `/capability` command. `Ok(false)` continues the
/// ordinary conversation provider path. `Ok(true)` means this turn is finished.
pub(crate) async fn handle_user_turn(
    state: &AppState,
    input: &StartTurnInput,
    on_event: &dyn RuntimeEventSender,
    cancellation: Arc<RunCancellation>,
) -> Result<bool, TurnExecutionFailure> {
    match parse(&input.content) {
        Err(CommandError::NotACommand) => Ok(false),
        Err(CommandError::Malformed) => {
            complete_command(
                state,
                input,
                on_event,
                "That is not a valid /capability command. Use `/capability generate <request-id>`, `/capability update <request-id> <revision-id>`, or `/capability inspect <call-id>`. Natural-language generation is not offered.",
            )
            .await?;
            Ok(true)
        }
        Ok(command) => {
            let content = dispatch(state, input, command, cancellation).await;
            complete_command(state, input, on_event, &content).await?;
            Ok(true)
        }
    }
}

async fn dispatch(
    state: &AppState,
    input: &StartTurnInput,
    command: CapabilityCommand,
    cancellation: Arc<RunCancellation>,
) -> String {
    match command {
        CapabilityCommand::Inspect { call_id } => format!(
            "Capability inspection for `{call_id}` is recorded against the call owner. Use this command after a generated call. Natural-language generation is not offered."
        ),
        CapabilityCommand::Generate { request_id } | CapabilityCommand::Update { request_id, .. } => {
            let Some(generation) = state.generation.clone() else {
                return "Capability generation is not configured on this host. Natural-language generation is not offered.".into();
            };
            let Ok(principal_id) = crate::tool_selection::service::ensure_principal(&state.sqlite_writer)
            else {
                return "Capability generation is unavailable because no principal is recorded.".into();
            };
            let input_message_id = state
                .sqlite_readers
                .read(|connection| {
                    connection
                        .query_row(
                            "SELECT input_message_id FROM runtime_runs WHERE id = ?1",
                            rusqlite::params![input.run_id],
                            |row| row.get::<_, Option<String>>(0),
                        )
                        .map_err(|error| error.to_string())
                })
                .ok()
                .flatten()
                .unwrap_or_default();
            let base_revision_id = match command {
                CapabilityCommand::Update {
                    base_revision_id, ..
                } => Some(base_revision_id),
                _ => None,
            };
            let receipt = generation
                .generate(
                    GenerationContext {
                        principal_id,
                        conversation_id: input.conversation_id.clone(),
                        run_id: input.run_id.clone(),
                        input_message_id,
                        project_id: None,
                    },
                    GenerateInput {
                        request_id: request_id.clone(),
                        base_revision_id,
                    },
                    (*cancellation).clone(),
                )
                .await;
            format!(
                "Capability generation\nrequest: {request_id}\nstatus: {}\njob: {}\nrevision: {}\nerror: {}\nAfter a revision is active, search and call it through the usual conversation tools. Do not expect a side-effect tool to run automatically. Natural-language generation is not offered.",
                receipt.status.as_str(),
                receipt.job_id,
                receipt.revision_id.as_deref().unwrap_or("-"),
                receipt
                    .error_code
                    .map(|code| code.as_str().to_string())
                    .unwrap_or_else(|| "-".into()),
            )
}

async fn complete_command(
    state: &AppState,
    input: &StartTurnInput,
    on_event: &dyn RuntimeEventSender,
    content: &str,
) -> Result<(), TurnExecutionFailure> {
    let _ = on_event.send(RuntimeEvent::Started {
        run_id: input.run_id.clone(),
        route: "conversation.respond".to_string(),
        provider_id: "capability".to_string(),
    });
    let message = persist_conversation_success(state, input, content)?;
    crate::memory::personal_state::output::send_completed(state, input, on_event, &message)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_the_exact_saved_input_is_a_command() {
        assert_eq!(
            parse("/capability generate req-1"),
            Ok(CapabilityCommand::Generate {
                request_id: "req-1".into()
            })
        );
        assert_eq!(
            parse("  /capability inspect call-1\n"),
            Ok(CapabilityCommand::Inspect {
                call_id: "call-1".into()
            })
        );
        assert_eq!(
            parse("please run /capability generate req-1"),
            Err(CommandError::NotACommand)
        );
        // A word that merely begins with the prefix is an ordinary message.
        assert_eq!(
            parse("/capabilityfoo generate req-1"),
            Err(CommandError::NotACommand)
        );
        assert_eq!(
            parse("/capability generate req-1\nsecond"),
            Err(CommandError::Malformed)
        );
        assert_eq!(parse("/capability generate"), Err(CommandError::Malformed));
        assert_eq!(
            parse("/capability generate req-1 extra"),
            Err(CommandError::Malformed)
        );
        assert_eq!(
            parse("/capability update req-1"),
            Err(CommandError::Malformed)
        );
        assert_eq!(parse("/capability inspect"), Err(CommandError::Malformed));
    }

    #[test]
    fn update_requires_a_base_revision() {
        assert_eq!(
            parse("/capability update req-1 rev-9"),
            Ok(CapabilityCommand::Update {
                request_id: "req-1".into(),
                base_revision_id: "rev-9".into()
            })
        );
        // The command family with a bad id is malformed, not an ordinary message.
        assert_eq!(
            parse("/capability generate BadId"),
            Err(CommandError::Malformed)
        );
    }

    fn receipt(typescript: &str) -> InspectionReceipt {
        InspectionReceipt {
            inspection_id: "i".into(),
            revision_id: "r".into(),
            source_hash: "a".repeat(64),
            program_hash: "b".repeat(64),
            artifact_hash: "c".repeat(64),
            projection_hash: "d".repeat(64),
            typescript_text: typescript.into(),
            report_json: json!({}),
            comparison_json: json!({
                "checkedCases": 4,
                "mismatches": [],
                "semanticEquivalence": "not-checked"
            }),
        }
    }

    #[test]
    fn fence_is_longer_than_any_backtick_run() {
        let display = render_inspection(&receipt("const x = `a`; const y = ```;")).unwrap();
        assert!(display.content.contains("````"));
        assert!(!display.too_large);
    }

    #[test]
    fn oversized_typescript_is_summarised_not_truncated_silently() {
        let big = "x".repeat(MAX_DISPLAY_BYTES + 10);
        let display = render_inspection(&receipt(&big)).unwrap();
        assert!(display.too_large);
        assert!(display.content.contains("artifact-too-large"));
    }
}
