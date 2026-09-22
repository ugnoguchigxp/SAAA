use crate::diagnosis::contract::{DiagnosisItem, DiagnosisSeverity, DiagnosisStatus};

pub(super) mod harness;
pub(super) mod memory;
pub(super) mod providers;
pub(super) mod settings;
pub(super) mod sqlite;

pub(super) fn item(
    id: &str,
    group: &str,
    label: &str,
    status: DiagnosisStatus,
    severity: DiagnosisSeverity,
    message: &str,
    latency_ms: Option<u64>,
) -> DiagnosisItem {
    DiagnosisItem {
        id: id.to_string(),
        group: group.to_string(),
        label: label.to_string(),
        status,
        severity,
        message: crate::redact_runtime_text(message),
        latency_ms,
    }
}
