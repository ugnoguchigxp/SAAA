//! Strict conversation commands for the restricted generation/inspection flow (plan 12.7).
//!
//! Only the exact saved user input is a command. Leading/trailing whitespace is trimmed, but a
//! newline, an extra argument, or a command inside a quote is not a command. Ordinary model or
//! tool output can never start one.

pub const COMMAND_PREFIX: &str = "/capability";
#[path = "capability_inspect.rs"]
mod capability_inspect;
#[path = "capability_inspect_run.rs"]
mod capability_inspect_run;
#[path = "capability_turn.rs"]
mod capability_turn;
#[cfg(test)]
pub(crate) use capability_inspect::load_stored_inspection;
#[cfg(test)]
pub use capability_inspect::{render_inspection, MAX_DISPLAY_BYTES};
pub(crate) use capability_turn::handle_user_turn;

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generated_capabilities::inspection::contracts::InspectionReceipt;
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
