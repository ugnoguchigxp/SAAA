use super::super::stream::ProviderFailureKind as Failure;
use crate::runtime::agent_tools::AgentToolCall;
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};

#[derive(Default)]
struct Tool {
    id: String,
    name: String,
    arguments: String,
}

#[derive(Default)]
pub(super) struct Completion {
    pub(super) content: String,
    pub(super) finish: Option<String>,
    pub(super) done: bool,
    tools: BTreeMap<u64, Tool>,
    pub(super) usage: Option<crate::runtime::context::usage::ProviderUsage>,
    response_model: Option<String>,
    reasoning_started: bool,
    tool_started: bool,
}

impl Completion {
    pub(super) fn absorb(&mut self, data: &str, _requested_model: &str) -> Result<String, Failure> {
        if self.done {
            return Err(Failure::Protocol);
        }
        if data == "[DONE]" {
            if self.finish.is_none() {
                return Err(Failure::Protocol);
            }
            self.done = true;
            return Ok(String::new());
        }
        let value: Value = serde_json::from_str(data).map_err(|_| Failure::Protocol)?;
        if value.get("error").is_some() {
            return Err(Failure::Upstream);
        }
        // OpenAI-compatible servers may return a concrete model for a public alias.
        // Keep that response identity stable across this completion, rather than
        // inventing aliases or requiring the request spelling in every chunk.
        if let Some(value) = value.get("model") {
            let actual = value
                .as_str()
                .filter(|m| !m.is_empty() && m.len() <= 256)
                .ok_or(Failure::Contract)?;
            if self
                .response_model
                .as_deref()
                .is_some_and(|prior| prior != actual)
            {
                return Err(Failure::Contract);
            }
            self.response_model = Some(actual.into());
        }
        let choices = value
            .get("choices")
            .and_then(Value::as_array)
            .ok_or(Failure::Protocol)?;
        if choices.is_empty() && value.get("usage").is_some_and(Value::is_object) {
            self.usage = Some(crate::runtime::context::usage::parse_openai_usage(&value));
            return Ok(String::new());
        }
        if choices.len() != 1 || self.finish.is_some() {
            return Err(Failure::Protocol);
        }
        let choice = &choices[0];
        if choice.get("index").and_then(Value::as_u64) != Some(0) {
            return Err(Failure::Protocol);
        }
        let delta = choice
            .get("delta")
            .and_then(Value::as_object)
            .ok_or(Failure::Protocol)?;
        if delta
            .get("role")
            .is_some_and(|v| v.as_str() != Some("assistant"))
        {
            return Err(Failure::Protocol);
        }
        let content = match delta.get("content") {
            None | Some(Value::Null) => "",
            Some(Value::String(content)) => content,
            _ => return Err(Failure::Protocol),
        };
        match delta.get("reasoning_content") {
            None | Some(Value::Null) => {}
            Some(Value::String(reasoning)) => {
                self.reasoning_started |= !reasoning.is_empty();
            }
            _ => return Err(Failure::Protocol),
        }
        self.content.push_str(content);
        if self.content.len() > 1_048_576 {
            return Err(Failure::RequestTooLarge);
        }
        if let Some(calls) = delta.get("tool_calls").filter(|v| !v.is_null()) {
            self.tool_started = true;
            for call in calls.as_array().ok_or(Failure::Protocol)? {
                let index = call
                    .get("index")
                    .and_then(Value::as_u64)
                    .ok_or(Failure::Protocol)?;
                if index >= 4 {
                    return Err(Failure::Protocol);
                }
                if call
                    .get("type")
                    .is_some_and(|v| v.as_str() != Some("function"))
                {
                    return Err(Failure::Protocol);
                }
                let tool = self.tools.entry(index).or_default();
                if let Some(id) = call.get("id") {
                    let id = id.as_str().ok_or(Failure::Protocol)?;
                    if id.is_empty() || id.len() > 160 || (!tool.id.is_empty() && tool.id != id) {
                        return Err(Failure::Protocol);
                    }
                    tool.id = id.to_string();
                }
                if let Some(function) = call.get("function") {
                    let function = function.as_object().ok_or(Failure::Protocol)?;
                    for (field, target, limit) in [
                        ("name", &mut tool.name, 160),
                        ("arguments", &mut tool.arguments, 16_384),
                    ] {
                        if let Some(value) = function.get(field) {
                            target.push_str(value.as_str().ok_or(Failure::Protocol)?);
                            if target.len() > limit {
                                return Err(Failure::RequestTooLarge);
                            }
                        }
                    }
                }
            }
        }
        if let Some(reason) = choice.get("finish_reason").filter(|v| !v.is_null()) {
            let reason = reason.as_str().ok_or(Failure::Protocol)?;
            if !matches!(reason, "stop" | "tool_calls" | "length" | "content_filter") {
                return Err(Failure::Protocol);
            }
            self.finish = Some(reason.to_string());
        }
        Ok(content.to_string())
    }

    pub(super) fn provider_progressed(&self) -> bool {
        self.reasoning_started
            || self.tool_started
            || !self.content.is_empty()
            || self.finish.is_some()
            || self.done
    }

    pub(super) fn complete(&self) -> Result<Vec<AgentToolCall>, Failure> {
        if !self.done {
            return Err(Failure::Protocol);
        }
        match self.finish.as_deref() {
            Some("length") => return Err(Failure::PartialOutput),
            Some("content_filter") => return Err(Failure::Policy),
            Some("stop") if self.tools.is_empty() => return Ok(Vec::new()),
            Some("tool_calls") if !self.tools.is_empty() => {}
            _ => return Err(Failure::Protocol),
        }
        let mut ids = HashSet::new();
        self.tools
            .values()
            .map(|tool| {
                if tool.id.is_empty()
                    || tool.name.is_empty()
                    || !ids.insert(&tool.id)
                    || !serde_json::from_str::<Value>(&tool.arguments).is_ok_and(|v| v.is_object())
                {
                    return Err(Failure::Protocol);
                }
                Ok(AgentToolCall {
                    id: tool.id.clone(),
                    name: tool.name.clone(),
                    arguments: tool.arguments.clone(),
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod progress_tests {
    use super::*;

    #[test]
    fn reasoning_tool_and_finish_are_distinct_provider_progress() {
        let mut reasoning = Completion::default();
        assert_eq!(
            reasoning.absorb(
                r#"{"choices":[{"index":0,"delta":{"reasoning_content":"thinking"},"finish_reason":null}]}"#,
                "model"
            ),
            Ok(String::new())
        );
        assert!(reasoning.provider_progressed());
        assert!(reasoning.content.is_empty());

        let mut empty = Completion::default();
        assert!(!empty.provider_progressed());
        empty
            .absorb(
                r#"{"choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}"#,
                "model",
            )
            .unwrap();
        assert!(empty.provider_progressed());
        assert!(empty.content.is_empty());
    }

    #[test]
    fn cw_12_chunk_with_usage_only_is_retained() {
        let mut completion = Completion::default();
        assert_eq!(
            completion.absorb(
                r#"{"choices":[],"usage":{"prompt_tokens":10,"prompt_tokens_details":{"cached_tokens":4},"completion_tokens":2}}"#,
                "model"
            ),
            Ok(String::new())
        );
        let usage = completion.usage.expect("usage retained");
        assert_eq!(usage.input_tokens, Some(10));
        assert_eq!(usage.cache_read_tokens, Some(4));
        assert_eq!(usage.output_tokens, Some(2));
    }
}
