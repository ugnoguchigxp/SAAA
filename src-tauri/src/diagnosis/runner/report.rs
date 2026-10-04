use super::*;

pub(super) fn report_from(
    revision: u64,
    started_at: String,
    mode: DiagnosisMode,
    mut items: Vec<DiagnosisItem>,
) -> DiagnosisReport {
    items.push(checks::item(
        match mode {
            DiagnosisMode::Fast => "diagnosis.mode.fast",
            DiagnosisMode::Operational => "diagnosis.mode.operational",
        },
        "settings",
        "Diagnosis mode",
        DiagnosisStatus::Skipped,
        DiagnosisSeverity::Info,
        "",
        None,
    ));
    DiagnosisReport {
        revision,
        started_at,
        finished_at: Some(crate::now_iso()),
        running: false,
        overall: overall(&items),
        items,
    }
}
