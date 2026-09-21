//! Model-independent, bounded contract shared by the desktop and MCP service.
use serde::{Deserialize, Serialize};

pub mod world;

pub const VERSION: &str = "reasoning-answer-v2";
pub const PROTOCOL: &str = "2025-06-18";
pub const TOOL: &str = "reasoning.answer";
pub const MAX_INPUT_BYTES: usize = 64 * 1024;
pub const MAX_MODEL_INPUT_BYTES: usize = 24 * 1024;
pub const MAX_OUTPUT_BYTES: usize = 16 * 1024;
pub const TIMEOUT_MS: u64 = 15_000;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Request {
    pub schema_version: String,
    pub conversation_id: String,
    pub turn_id: String,
    pub request_id: String,
    pub context_revision: u64,
    pub request: String,
    pub context: Context,
    pub constraints: Constraints,
    pub budget: Budget,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Context {
    pub messages: Vec<Message>,
    pub evidence: Vec<Evidence>,
    pub truncated: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Message {
    pub role: Role,
    pub content: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Evidence {
    pub world: Option<world::WorldEvidence>,
    pub id: String,
    pub source: String,
    pub content: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Constraints {
    pub language: String,
    pub local_only: bool,
    pub max_speech_chars: usize,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Budget {
    pub timeout_ms: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Intent {
    Answer,
    Clarify,
    InsufficientContext,
}

/// The model generates only this payload. The service binds correlation IDs.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Answer {
    pub intent: Intent,
    pub speech_text: String,
    pub key_points: Vec<String>,
    pub evidence_ids: Vec<String>,
    pub limitations: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Response {
    pub schema_version: String,
    pub conversation_id: String,
    pub turn_id: String,
    pub request_id: String,
    pub context_revision: u64,
    pub intent: Intent,
    pub speech_text: String,
    pub key_points: Vec<String>,
    pub evidence_ids: Vec<String>,
    pub limitations: Vec<String>,
}

fn text(value: &str, max: usize) -> bool {
    !value.trim().is_empty() && value.chars().count() <= max
}
fn id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 160
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_.-".contains(&c))
}
fn bytes<T: Serialize>(value: &T, max: usize) -> bool {
    serde_json::to_vec(value).is_ok_and(|v| v.len() <= max)
}
impl Request {
    pub fn model_input(&self) -> serde_json::Value {
        serde_json::json!({"request":self.request,"context":self.context,"constraints":self.constraints})
    }
    pub fn model_input_fits(&self) -> bool {
        bytes(&self.model_input(), MAX_MODEL_INPUT_BYTES)
    }
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.schema_version != VERSION
            || !id(&self.conversation_id)
            || !id(&self.turn_id)
            || !id(&self.request_id)
            || self.context_revision == 0
            || !text(&self.request, 16_000)
            || !bytes(self, MAX_INPUT_BYTES)
            || self.context.messages.len() > 32
            || self.context.evidence.len() > 8
            || self
                .context
                .messages
                .iter()
                .any(|m| !text(&m.content, 16_000))
            || self.context.evidence.iter().any(|e| {
                !id(&e.id)
                    || !text(&e.source, 512)
                    || !text(&e.content, 16_000)
                    || (e.source.starts_with("world-model:") != e.world.is_some())
                    || e.world
                        .as_ref()
                        .is_some_and(|world| !world.matches_content(&e.content))
            })
            || !matches!(self.constraints.language.as_str(), "ja" | "en" | "auto")
            || !self.constraints.local_only
            || !(1..=240).contains(&self.constraints.max_speech_chars)
            || !(1..=TIMEOUT_MS).contains(&self.budget.timeout_ms)
        {
            return Err("invalid_request");
        }
        let unique: std::collections::HashSet<_> =
            self.context.evidence.iter().map(|e| &e.id).collect();
        if unique.len() != self.context.evidence.len() {
            return Err("invalid_request");
        }
        Ok(())
    }
}
impl Answer {
    pub fn validate(&self, request: &Request) -> Result<(), &'static str> {
        if !text(&self.speech_text, request.constraints.max_speech_chars)
            || self.key_points.len() > 5
            || self.limitations.len() > 5
            || self
                .key_points
                .iter()
                .chain(&self.limitations)
                .any(|s| !text(s, 160))
            || self.evidence_ids.len() > 8
            || self
                .evidence_ids
                .iter()
                .any(|id| !request.context.evidence.iter().any(|e| &e.id == id))
            || !bytes(self, MAX_OUTPUT_BYTES)
        {
            return Err("invalid_response");
        }
        Ok(())
    }
}
impl Response {
    pub fn bind(request: &Request, answer: Answer) -> Result<Self, &'static str> {
        request.validate()?;
        answer.validate(request)?;
        Ok(Self {
            schema_version: VERSION.into(),
            conversation_id: request.conversation_id.clone(),
            turn_id: request.turn_id.clone(),
            request_id: request.request_id.clone(),
            context_revision: request.context_revision,
            intent: answer.intent,
            speech_text: answer.speech_text,
            key_points: answer.key_points,
            evidence_ids: answer.evidence_ids,
            limitations: answer.limitations,
        })
    }
    pub fn validate(&self, request: &Request) -> Result<(), &'static str> {
        if self.schema_version != VERSION
            || self.conversation_id != request.conversation_id
            || self.turn_id != request.turn_id
            || self.request_id != request.request_id
            || self.context_revision != request.context_revision
            || !bytes(self, MAX_OUTPUT_BYTES)
        {
            return Err("stale_or_invalid_response");
        }
        Answer {
            intent: self.intent.clone(),
            speech_text: self.speech_text.clone(),
            key_points: self.key_points.clone(),
            evidence_ids: self.evidence_ids.clone(),
            limitations: self.limitations.clone(),
        }
        .validate(request)
    }
}

pub mod schema;
#[cfg(test)]
mod tests;
