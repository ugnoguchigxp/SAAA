use serde::{Deserialize, Serialize};

pub const MAX_INPUT_BYTES: usize = 4 * 1024;
pub const MAX_PUBLIC_BYTES: usize = 8 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Reply,
    Clarify,
    Delegate,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextInput {
    pub session_id: String,
    pub input_id: String,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResponseRef {
    pub response_id: String,
    pub input_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublicDelta {
    pub response_id: String,
    pub seq: u64,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpeechRef {
    pub response_id: String,
    pub speech_id: String,
    pub generation: u64,
    pub clause_index: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Failure {
    pub stage: String,
    pub code: String,
    pub message: String,
    pub request_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InputError {
    InvalidId,
    Empty,
    TooLong,
}

pub fn validate_input(input: &TextInput) -> Result<(), InputError> {
    if !valid_id(&input.session_id) || !valid_id(&input.input_id) {
        return Err(InputError::InvalidId);
    }
    if input.text.trim().is_empty() {
        return Err(InputError::Empty);
    }
    if input.text.len() > MAX_INPUT_BYTES {
        return Err(InputError::TooLong);
    }
    Ok(())
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}
