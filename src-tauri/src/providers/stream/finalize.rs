use super::{CompletionFinish, ModelProviderCompletion, ProviderAttemptError, ProviderFailureKind};
use crate::runtime::agent_tools::AgentToolCall;

pub(super) fn finalize_stream_completion(
    content: String,
    tool_call: Option<AgentToolCall>,
    finish: CompletionFinish,
    output_started: bool,
) -> Result<ModelProviderCompletion, ProviderAttemptError> {
    if let Some(call) = tool_call {
        if finish != CompletionFinish::ToolCalls || !content.trim().is_empty() {
            return Err(ProviderAttemptError::failed(
                ProviderFailureKind::Protocol,
                output_started,
            ));
        }
        return Ok(ModelProviderCompletion::ToolCall(call));
    }
    if finish != CompletionFinish::Stop || content.trim().is_empty() {
        return Err(ProviderAttemptError::failed(
            ProviderFailureKind::Protocol,
            output_started,
        ));
    }
    Ok(ModelProviderCompletion::Content(content))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call() -> AgentToolCall {
        AgentToolCall {
            id: "call_1".to_string(),
            name: "recall_conversation".to_string(),
            arguments: "{}".to_string(),
        }
    }

    #[test]
    fn terminal_reason_must_match_the_stream_payload() {
        assert!(matches!(
            finalize_stream_completion(
                "answer".to_string(),
                None,
                CompletionFinish::Stop,
                true
            ),
            Ok(ModelProviderCompletion::Content(content)) if content == "answer"
        ));
        assert!(matches!(
            finalize_stream_completion(
                String::new(),
                Some(call()),
                CompletionFinish::ToolCalls,
                false
            ),
            Ok(ModelProviderCompletion::ToolCall(_))
        ));
        for result in [
            finalize_stream_completion(
                "answer".to_string(),
                None,
                CompletionFinish::ToolCalls,
                true,
            ),
            finalize_stream_completion(String::new(), Some(call()), CompletionFinish::Stop, false),
            finalize_stream_completion(
                "mixed".to_string(),
                Some(call()),
                CompletionFinish::ToolCalls,
                true,
            ),
        ] {
            assert!(matches!(
                result,
                Err(ProviderAttemptError::Failed {
                    kind: ProviderFailureKind::Protocol,
                    ..
                })
            ));
        }
    }
}
