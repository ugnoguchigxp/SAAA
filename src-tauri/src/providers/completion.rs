use serde_json::Value;

pub(crate) const DEFAULT_MAX_OUTPUT_TOKENS: u32 = 2_048;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CompletionFinish {
    Stop,
    ToolCalls,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CompletionTerminalError {
    PartialOutput,
    Policy,
    Protocol,
}

#[derive(Debug, Default)]
pub(crate) struct CompletionTerminal {
    finish: Option<CompletionFinish>,
}

impl CompletionTerminal {
    pub(crate) fn observe(&mut self, value: &Value) -> Result<(), CompletionTerminalError> {
        let Some(reason) = value.pointer("/choices/0/finish_reason") else {
            return Ok(());
        };
        if reason.is_null() {
            return Ok(());
        }
        let finish = match reason.as_str() {
            Some("stop") => CompletionFinish::Stop,
            Some("tool_calls") => CompletionFinish::ToolCalls,
            Some("length") => return Err(CompletionTerminalError::PartialOutput),
            Some("content_filter") => return Err(CompletionTerminalError::Policy),
            _ => return Err(CompletionTerminalError::Protocol),
        };
        if self.finish.is_some_and(|previous| previous != finish) {
            return Err(CompletionTerminalError::Protocol);
        }
        self.finish = Some(finish);
        Ok(())
    }

    pub(crate) fn complete(&self) -> Result<CompletionFinish, CompletionTerminalError> {
        self.finish.ok_or(CompletionTerminalError::Protocol)
    }
}

pub(crate) fn validate_non_stream_completion(
    value: &Value,
) -> Result<CompletionFinish, CompletionTerminalError> {
    let mut terminal = CompletionTerminal::default();
    terminal.observe(value)?;
    terminal.complete()
}

pub(crate) fn thinking_enabled(reasoning_effort: &str) -> bool {
    reasoning_effort != "low"
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn requires_a_supported_terminal_reason() {
        for (reason, expected) in [
            ("stop", CompletionFinish::Stop),
            ("tool_calls", CompletionFinish::ToolCalls),
        ] {
            let value = json!({ "choices": [{ "finish_reason": reason }] });
            assert_eq!(validate_non_stream_completion(&value), Ok(expected));
        }
        assert_eq!(
            validate_non_stream_completion(&json!({ "choices": [{ "finish_reason": "length" }] })),
            Err(CompletionTerminalError::PartialOutput)
        );
        assert_eq!(
            validate_non_stream_completion(&json!({ "choices": [{ "finish_reason": null }] })),
            Err(CompletionTerminalError::Protocol)
        );
        assert_eq!(
            validate_non_stream_completion(&json!({ "choices": [{ "finish_reason": "other" }] })),
            Err(CompletionTerminalError::Protocol)
        );
    }

    #[test]
    fn reasoning_effort_controls_local_thinking_without_contradiction() {
        assert!(!thinking_enabled("low"));
        assert!(thinking_enabled("medium"));
        assert!(thinking_enabled("xhigh"));
    }
}
