use std::sync::Arc;

use super::SituationRuntime;
use crate::persistence::SqliteWriter;

pub(crate) fn spawn_situation_monitor(
    connection: Arc<SqliteWriter>,
    runtime: Arc<SituationRuntime>,
) {
    if !runtime.enabled() || !runtime.begin_worker() {
        return;
    }
    tauri::async_runtime::spawn(async move {
        while runtime.enabled() {
            let result = runtime
                .sample_platform()
                .and_then(|sample| runtime.tick_sampled(&connection, sample));
            if let Err(error) = result {
                runtime.record_failure(error);
            }
            runtime.wait_for_next_sample().await;
        }
        runtime.finish_worker();
        if runtime.enabled() {
            spawn_situation_monitor(connection, runtime);
        }
    });
}
