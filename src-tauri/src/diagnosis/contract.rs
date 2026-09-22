use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum DiagnosisStatus {
    Ok,
    Warn,
    Fail,
    Skipped,
    Running,
}

impl DiagnosisStatus {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Warn => "warn",
            Self::Fail => "fail",
            Self::Skipped => "skipped",
            Self::Running => "running",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum DiagnosisSeverity {
    Fatal,
    Degraded,
    Info,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DiagnosisItem {
    pub(crate) id: String,
    pub(crate) group: String,
    pub(crate) label: String,
    pub(crate) status: DiagnosisStatus,
    pub(crate) severity: DiagnosisSeverity,
    pub(crate) message: String,
    #[ts(type = "number | null")]
    pub(crate) latency_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DiagnosisReport {
    #[ts(type = "number")]
    pub(crate) revision: u64,
    pub(crate) started_at: String,
    pub(crate) finished_at: Option<String>,
    pub(crate) running: bool,
    pub(crate) overall: DiagnosisStatus,
    pub(crate) items: Vec<DiagnosisItem>,
}

pub(crate) fn typescript_bindings() -> String {
    [
        DiagnosisStatus::decl(&Default::default()),
        DiagnosisSeverity::decl(&Default::default()),
        DiagnosisItem::decl(&Default::default()),
        DiagnosisReport::decl(&Default::default()),
    ]
    .map(|decl| format!("export {decl}"))
    .join("\n")
}

pub(crate) fn typescript_file() -> String {
    format!(
        "// Generated from src-tauri/src/diagnosis/contract.rs. Do not edit.\n{}\n",
        typescript_bindings()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dg_01_status_serializes_kebab_case() {
        assert_eq!(
            serde_json::to_string(&DiagnosisStatus::Skipped).expect("status serializes"),
            "\"skipped\""
        );
        assert_eq!(
            serde_json::to_string(&DiagnosisSeverity::Degraded).expect("severity serializes"),
            "\"degraded\""
        );
    }
}
