//! Shared wire compatibility for standard HTTP providers. No lease dependency.
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LlmOptions {
    #[serde(default)]
    pub token_limit: TokenLimit,
    #[serde(default)]
    pub reasoning: Reasoning,
    #[serde(default = "yes")]
    pub tools: bool,
    #[serde(default = "yes")]
    pub streaming: bool,
    /// Optional explicit sampling temperature. `None` keeps the provider default.
    #[serde(default)]
    pub temperature: Option<f32>,
    #[serde(default)]
    pub thinking: Thinking,
}
impl Default for LlmOptions {
    fn default() -> Self {
        Self {
            token_limit: TokenLimit::Auto,
            reasoning: Reasoning::Auto,
            tools: true,
            streaming: true,
            temperature: None,
            thinking: Thinking::Auto,
        }
    }
}
#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Thinking {
    #[default]
    Auto,
    Disabled,
    Enabled,
}
fn yes() -> bool {
    true
}
impl LlmOptions {
    pub fn standard() -> Self {
        Self::default()
    }
    pub fn apply(&self, body: &mut Value, model: &str, limit: u32, effort: &str) {
        body.as_object_mut()
            .expect("request object")
            .remove("max_tokens");
        body.as_object_mut()
            .expect("request object")
            .remove("max_completion_tokens");
        let modern = matches!(self.token_limit, TokenLimit::Completion)
            || (matches!(self.token_limit, TokenLimit::Auto) && reasoning_model(model));
        body[if modern {
            "max_completion_tokens"
        } else {
            "max_tokens"
        }] = json!(limit);
        body.as_object_mut().unwrap().remove("reasoning_effort");
        body.as_object_mut().unwrap().remove("chat_template_kwargs");
        if let Some(temperature) = self.temperature {
            body["temperature"] = json!(temperature);
        }
        if effort != "provider-default"
            && (matches!(self.reasoning, Reasoning::Supported)
                || (matches!(self.reasoning, Reasoning::Auto)
                    && reasoning_model(model)
                    && matches!(effort, "low" | "medium" | "high")
                    && !model.contains("-pro")
                    && !model.contains("-chat")
                    && !model.contains("-preview")
                    && !model.contains("o1-mini")))
        {
            body["reasoning_effort"] = json!(effort);
        } else if effort != "provider-default"
            && !matches!(self.reasoning, Reasoning::Unsupported)
            && qwen_model(model)
            && matches!(effort, "low" | "medium" | "xhigh")
        {
            // llama.cpp passes custom template variables through chat_template_kwargs. Qwen's
            // template defaults to xhigh when this value is absent, which makes every tool
            // follow-up perform a full deep-reasoning pass even when SAAA is configured for low.
            body["chat_template_kwargs"] = json!({"reasoning_effort": effort});
        }
        match self.thinking {
            Thinking::Disabled => {
                body["chat_template_kwargs"] = json!({"enable_thinking": false});
            }
            Thinking::Enabled => {
                body["chat_template_kwargs"] = json!({"enable_thinking": true});
            }
            Thinking::Auto => {}
        }
    }
}
#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum TokenLimit {
    #[default]
    Auto,
    Legacy,
    Completion,
}
#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Reasoning {
    #[default]
    Auto,
    Supported,
    Unsupported,
}
fn reasoning_model(model: &str) -> bool {
    let model = model.strip_prefix("openai/").unwrap_or(model);
    ["o1", "o3", "o4", "gpt-5"]
        .iter()
        .any(|prefix| model == *prefix || model.starts_with(&format!("{prefix}-")))
}

fn qwen_model(model: &str) -> bool {
    model
        .strip_prefix("openai/")
        .unwrap_or(model)
        .to_ascii_lowercase()
        .contains("qwen")
}
pub fn operation_url(base: &str, operation: &str) -> Result<String, String> {
    let mut url = url::Url::parse(base).map_err(|_| "Provider endpoint is invalid")?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(
            "Provider base URL must be HTTP(S) without credentials, query or fragment".into(),
        );
    }
    let mut path = url.path().trim_end_matches('/').to_owned();
    for suffix in [
        "/chat/completions",
        "/audio/transcriptions",
        "/audio/speech",
        "/models",
    ] {
        if path.ends_with(suffix) {
            path.truncate(path.len() - suffix.len());
            break;
        }
    }
    // A bare origin is the sole shorthand. Every explicit API prefix is preserved.
    if path.is_empty() {
        path = "/v1".into();
    }
    url.set_path(&format!("{path}/{operation}"));
    Ok(url.to_string())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preserves_vendor_base_urls() {
        for (base, expected) in [
            (
                "https://api.openai.com",
                "https://api.openai.com/v1/chat/completions",
            ),
            (
                "https://generativelanguage.googleapis.com/v1beta/openai/",
                "https://generativelanguage.googleapis.com/v1beta/openai/chat/completions",
            ),
            (
                "http://localhost/proxy",
                "http://localhost/proxy/chat/completions",
            ),
        ] {
            assert_eq!(operation_url(base, "chat/completions").unwrap(), expected);
        }
    }
    #[test]
    fn model_capabilities_choose_only_supported_parameters() {
        let mut body = json!({});
        let options = LlmOptions::standard();
        options.apply(&mut body, "o3", 123, "medium");
        assert_eq!(
            body,
            json!({"max_completion_tokens":123,"reasoning_effort":"medium"})
        );
        options.apply(&mut body, "unknown-local-model", 456, "medium");
        assert_eq!(body, json!({"max_tokens":456}));
        options.apply(&mut body, "Qwen3.8-27B-ROCmFP4-FAST.gguf", 456, "low");
        assert_eq!(
            body,
            json!({
                "max_tokens":456,
                "chat_template_kwargs":{"reasoning_effort":"low"}
            })
        );
        let options = LlmOptions {
            token_limit: TokenLimit::Completion,
            reasoning: Reasoning::Unsupported,
            ..LlmOptions::standard()
        };
        options.apply(&mut body, "alias", 456, "high");
        assert_eq!(body, json!({"max_completion_tokens":456}));
    }

    #[test]
    fn thinking_disabled_overrides_qwen_reasoning_kwargs() {
        let mut body = json!({});
        let options = LlmOptions {
            thinking: Thinking::Disabled,
            ..LlmOptions::standard()
        };
        options.apply(&mut body, "qwen3.5-2b-fast-response", 32, "low");
        assert_eq!(
            body["chat_template_kwargs"],
            json!({"enable_thinking": false})
        );
    }

    #[test]
    fn thinking_auto_keeps_existing_behavior() {
        let mut body = json!({});
        let options = LlmOptions {
            thinking: Thinking::Auto,
            ..LlmOptions::standard()
        };
        options.apply(&mut body, "Qwen3.8-27B-ROCmFP4-FAST.gguf", 456, "low");
        assert_eq!(
            body["chat_template_kwargs"],
            json!({"reasoning_effort": "low"})
        );
    }

    #[test]
    fn llm_options_without_thinking_key_deserializes() {
        let options: LlmOptions = serde_json::from_str(r#"{"tokenLimit":"auto"}"#).unwrap();
        assert_eq!(options.thinking, Thinking::Auto);
    }
}
