#[cfg(test)]
use serde_json::Value;

pub(crate) const DEFAULT_MAX_OUTPUT_TOKENS: u32 = 2_048;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg(test)]
pub(crate) enum CompletionFinish {
    Stop,
    ToolCalls,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg(test)]
pub(crate) enum CompletionTerminalError {
    PartialOutput,
    Policy,
    Protocol,
}

#[derive(Debug, Default)]
#[cfg(test)]
pub(crate) struct CompletionTerminal {
    finish: Option<CompletionFinish>,
}

#[cfg(test)]
impl CompletionTerminal {
    pub(crate) fn observe(&mut self, value: &Value) -> Result<(), CompletionTerminalError> {
        if self.finish.is_some() {
            return Err(CompletionTerminalError::Protocol);
        }
        let choice = exact_choice(value)?;
        let Some(reason) = choice.get("finish_reason") else {
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
        self.finish = Some(finish);
        Ok(())
    }

    pub(crate) fn complete(&self) -> Result<CompletionFinish, CompletionTerminalError> {
        self.finish.ok_or(CompletionTerminalError::Protocol)
    }
}

#[cfg(test)]
fn exact_choice(value: &Value) -> Result<&serde_json::Map<String, Value>, CompletionTerminalError> {
    let choices = value
        .get("choices")
        .and_then(Value::as_array)
        .filter(|choices| choices.len() == 1)
        .ok_or(CompletionTerminalError::Protocol)?;
    let choice = choices[0]
        .as_object()
        .ok_or(CompletionTerminalError::Protocol)?;
    if choice
        .get("index")
        .is_some_and(|index| index.as_u64() != Some(0))
    {
        return Err(CompletionTerminalError::Protocol);
    }
    Ok(choice)
}

#[cfg(test)]
pub(crate) fn validate_non_stream_completion(
    value: &Value,
) -> Result<CompletionFinish, CompletionTerminalError> {
    let mut terminal = CompletionTerminal::default();
    terminal.observe(value)?;
    terminal.complete()
}

#[cfg(test)]
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

    #[test]
    fn rejects_missing_ambiguous_or_non_primary_choices() {
        for value in [
            json!({}),
            json!({ "choices": [] }),
            json!({ "choices": [{ "finish_reason": "stop" }, { "finish_reason": "stop" }] }),
            json!({ "choices": [{ "index": 1, "finish_reason": "stop" }] }),
            json!({ "choices": [null] }),
        ] {
            assert_eq!(
                validate_non_stream_completion(&value),
                Err(CompletionTerminalError::Protocol)
            );
        }
    }

    #[test]
    fn rejects_any_provider_event_after_a_terminal_choice() {
        let mut terminal = CompletionTerminal::default();
        terminal
            .observe(&json!({ "choices": [{ "index": 0, "finish_reason": "stop" }] }))
            .expect("first terminal is accepted");
        assert_eq!(
            terminal.observe(&json!({ "choices": [{ "index": 0, "delta": {} }] })),
            Err(CompletionTerminalError::Protocol)
        );
    }
}
