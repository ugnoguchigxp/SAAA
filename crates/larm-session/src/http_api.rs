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
}
impl Default for LlmOptions {
    fn default() -> Self {
        Self {
            token_limit: TokenLimit::Auto,
            reasoning: Reasoning::Auto,
            tools: true,
            streaming: true,
        }
    }
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
        let options = LlmOptions {
            token_limit: TokenLimit::Completion,
            reasoning: Reasoning::Unsupported,
            ..LlmOptions::standard()
        };
        options.apply(&mut body, "alias", 456, "high");
        assert_eq!(body, json!({"max_completion_tokens":456}));
    }
}
