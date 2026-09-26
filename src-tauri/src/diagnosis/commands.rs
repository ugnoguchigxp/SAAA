use super::contract::DiagnosisReport;
use super::runner;
use crate::AppState;

#[tauri::command]
pub(crate) async fn run_diagnosis(app: tauri::AppHandle) -> Result<DiagnosisReport, String> {
    Ok(runner::run_and_publish(&app, super::contract::DiagnosisMode::Operational).await)
}

#[tauri::command]
pub(crate) async fn run_fast_diagnosis(app: tauri::AppHandle) -> Result<DiagnosisReport, String> {
    Ok(runner::run_and_publish(&app, super::contract::DiagnosisMode::Fast).await)
}

#[tauri::command]
pub(crate) fn get_diagnosis_report(
    state: tauri::State<'_, AppState>,
) -> Result<DiagnosisReport, String> {
    Ok(current_report(&state))
}

pub(crate) fn current_report(state: &AppState) -> DiagnosisReport {
    state.diagnosis.snapshot()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnosis::contract::DiagnosisStatus;
    use rusqlite::Connection;

    #[test]
    fn dg_10_get_report_returns_store_snapshot() {
        let connection = Connection::open_in_memory().expect("database opens");
        crate::initialize_database(&connection).expect("database initializes");
        let state = crate::test_support::app_state(connection);
        let revision = state.diagnosis.try_begin().expect("run starts");
        let mut report = state.diagnosis.snapshot();
        report.revision = revision;
        report.overall = DiagnosisStatus::Warn;
        report.running = true;
        state.diagnosis.publish(report);
        let snapshot = current_report(&state);
        assert_eq!(snapshot.revision, revision);
        assert!(!snapshot.running);
        assert_eq!(snapshot.overall, DiagnosisStatus::Warn);
    }
}
