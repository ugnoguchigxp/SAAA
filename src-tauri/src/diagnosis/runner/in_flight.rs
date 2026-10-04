use super::*;

pub(super) struct InFlight {
    app: Option<tauri::AppHandle>,
    store: std::sync::Arc<super::super::store::DiagnosisStore>,
    revision: u64,
    started_at: String,
    mode: DiagnosisMode,
    finished: bool,
}

impl InFlight {
    pub(super) fn start(
        app: Option<tauri::AppHandle>,
        store: std::sync::Arc<super::super::store::DiagnosisStore>,
        revision: u64,
        started_at: String,
        mode: DiagnosisMode,
    ) -> Self {
        Self {
            app,
            store,
            revision,
            started_at,
            mode,
            finished: false,
        }
    }

    pub(super) fn progress(&mut self, items: Vec<DiagnosisItem>, app: &tauri::AppHandle) {
        let mut report = report_from(self.revision, self.started_at.clone(), self.mode, items);
        report.running = true;
        report.finished_at = None;
        report.overall = DiagnosisStatus::Running;
        self.store.stage(report.clone());
        let _ = app.emit("diagnosis-updated", &report);
    }

    pub(super) fn finish(&mut self, items: Vec<DiagnosisItem>) -> DiagnosisReport {
        let report = report_from(self.revision, self.started_at.clone(), self.mode, items);
        self.store.publish(report.clone());
        self.finished = true;
        report
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        let report = report_from(
            self.revision,
            self.started_at.clone(),
            self.mode,
            vec![checks::item(
                "diagnosis.interrupted",
                "storage",
                "Diagnosis interrupted",
                DiagnosisStatus::Fail,
                DiagnosisSeverity::Degraded,
                "診断が中断されました。再診断してください。",
                None,
            )],
        );
        self.store.publish(report.clone());
        if let Some(app) = &self.app {
            let _ = app.emit("diagnosis-updated", &report);
            eprintln!(
                "self diagnosis published revision={} overall={}",
                report.revision,
                report.overall.as_str()
            );
        }
    }
}
