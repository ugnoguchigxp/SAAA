use super::contract::{DiagnosisReport, DiagnosisScope};
use super::engine;
use crate::AppState;

#[tauri::command]
pub(crate) async fn run_diagnosis(
    app: tauri::AppHandle,
    scope: DiagnosisScope,
) -> Result<DiagnosisReport, String> {
    Ok(engine::run_and_publish(&app, scope).await)
}

#[tauri::command]
pub(crate) fn get_diagnosis_report(
    state: tauri::State<'_, AppState>,
) -> Result<DiagnosisReport, String> {
    Ok(state.diagnosis.snapshot())
}
