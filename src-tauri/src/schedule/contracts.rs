use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleEntryView {
    pub id: String,
    pub kind: String,
    pub subject_ref: String,
    pub scope_ref: String,
    #[ts(type = "number")]
    pub due_at: i64,
    pub status: String,
    pub origin: String,
    #[ts(type = "number")]
    pub revision: i64,
    pub fire_result: Option<String>,
    pub payload_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleStatus {
    pub enabled: bool,
    pub calendar_enabled: bool,
    pub calendar_id: Option<String>,
    pub calendar_connected: bool,
    pub last_error: Option<String>,
    pub platform_supported: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScheduleAddInput {
    pub kind: String,
    pub subject_ref: String,
    pub scope_ref: String,
    #[ts(type = "number")]
    pub due_at: i64,
    #[ts(type = "number | null")]
    pub window_end_at: Option<i64>,
    pub delegation_ref: Option<String>,
    pub payload: Option<String>,
}

pub fn typescript_bindings() -> String {
    format!(
        "// Generated from src-tauri/src/schedule/contracts.rs. Do not edit.\nexport {}\n\nexport {}\n\nexport {}\n",
        ScheduleEntryView::decl(&ts_rs::Config::default()),
        ScheduleStatus::decl(&ts_rs::Config::default()),
        ScheduleAddInput::decl(&ts_rs::Config::default()),
    )
}
