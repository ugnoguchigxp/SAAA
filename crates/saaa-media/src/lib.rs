//! Image and music execution shared by the desktop app and the lab host.
//! This crate does not open a database file, commit a transaction, or depend on Tauri.

mod backend;
mod contracts;
mod larm;
mod ledger;
mod ports;
pub mod replicate;
mod service;
mod store;

pub use backend::LiveBackend;
pub use contracts::{
    allowed_mime, validate_larm_token, validate_named_secret, ArtifactBytes, BoxFut, CancelOutcome,
    CancelStatus, Clock, ExplicitSecrets, HistoryQuery, ManualClock, MediaHostError, RemoteStop,
    RunHandle, RunTerminal, SystemClock,
};
pub use ledger::{
    cache, cached, finish, finish_query, get, history, history_limited, initialize,
    mark_absent_cancelled, phase, reconcile_interrupted, request_cancel, reserve, CancelRecord,
    FinishOutcome,
};
pub use ports::{
    AvailabilitySource, CredentialSource, FixedAvailability, GenerateCall, MediaBackend, MediaStore,
};
pub use service::MediaService;
pub use store::{DbOwner, MutexDb, SqlStore};

#[cfg(test)]
mod tests;
pub use saaa_larm_session::media::{
    MediaArtifact, MediaError, MediaKind, MediaProgress, MediaResult,
};
pub use saaa_provider_routing::ResolvedRoute;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GenerateInput {
    pub run_id: String,
    pub kind: MediaKind,
    pub prompt: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerateOutput {
    pub run_id: String,
    pub result: Option<MediaResult>,
    pub error: Option<MediaError>,
}

pub fn validate_run_id(id: &str) -> Result<(), String> {
    if uuid::Uuid::parse_str(id).is_err() {
        Err("生成要求の識別子が不正です。".into())
    } else {
        Ok(())
    }
}

pub fn validate_prompt(prompt: &str) -> Result<(), String> {
    if prompt.trim().is_empty() || prompt.len() > 16_384 {
        Err("生成内容は1〜16384バイトで入力してください。".into())
    } else {
        Ok(())
    }
}

pub fn database_error(error: rusqlite::Error) -> String {
    format!("SQLite operation failed: {error}")
}
