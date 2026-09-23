use serde::Deserialize;
use ts_rs::TS;

pub const NAMES: [&str; 4] = [
    "coding_start",
    "coding_inspect",
    "coding_continue",
    "coding_cancel",
];
pub const MAX_REQUEST_CHARS: usize = 32_000;

pub use super::settings::{valid_implementation, valid_profile, CodingSettings};
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Start {
    pub workspace_id: String,
    pub request: String,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Inspect {
    pub job_id: String,
    #[serde(default)]
    pub cursor: u64,
    #[serde(default = "default_limit")]
    pub limit: usize,
}
fn default_limit() -> usize {
    20
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Continue {
    pub job_id: String,
    pub expected_revision: u64,
    pub request: String,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Cancel {
    pub job_id: String,
    pub expected_revision: u64,
    pub reason: String,
}

pub fn validate(name: &str, arguments: &str) -> Result<(), String> {
    if arguments.len() > 200_000 {
        return Err("invalid_arguments".into());
    }
    let valid = match name {
        "coding_start" => serde_json::from_str::<Start>(arguments)
            .map(|a| id_valid(&a.workspace_id) && text_valid(&a.request))
            .unwrap_or(false),
        "coding_inspect" => serde_json::from_str::<Inspect>(arguments)
            .map(|a| {
                id_valid(&a.job_id) && a.limit > 0 && a.limit <= 100 && a.cursor <= i64::MAX as u64
            })
            .unwrap_or(false),
        "coding_continue" => serde_json::from_str::<Continue>(arguments)
            .map(|a| {
                id_valid(&a.job_id)
                    && a.expected_revision <= i64::MAX as u64
                    && text_valid(&a.request)
            })
            .unwrap_or(false),
        "coding_cancel" => serde_json::from_str::<Cancel>(arguments)
            .map(|a| {
                id_valid(&a.job_id)
                    && a.expected_revision <= i64::MAX as u64
                    && text_valid(&a.reason)
            })
            .unwrap_or(false),
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err("invalid_arguments".into())
    }
}
fn text_valid(text: &str) -> bool {
    !text.trim().is_empty() && text.chars().count() <= MAX_REQUEST_CHARS
}

pub fn typescript_bindings() -> String {
    format!(
        "// Generated from Rust coding/contracts.rs. Do not edit.\nexport {}\n",
        CodingSettings::decl(&ts_rs::Config::default())
    )
}

fn id_valid(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 160
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
}
