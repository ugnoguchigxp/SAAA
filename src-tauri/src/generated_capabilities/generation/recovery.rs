//! Startup recovery for the generation flow (plan 12.6, G07/G09).
//!
//! Jobs left in a running state by a previous process move to `interrupted`; no model call, build
//! or side effect is replayed. `awaiting_activation` is a verified candidate and is kept. The
//! inspection area is reconciled separately: a directory with no DB row is an orphan and is
//! removed, so a half-written inspection is never published.

use super::super::errors::CapabilityResult;
use super::super::inspection::service::InspectionStore;
use super::super::{inspection::repository as inspection_repository, lifecycle};
use super::repository::{self, unix_ms};
use crate::persistence::SqliteWriter;

#[allow(dead_code)]
#[derive(Clone, Debug, Default)]
pub(crate) struct GenerationRecoverySummary {
    pub interrupted_jobs: usize,
    pub orphan_inspections: Vec<String>,
}

#[allow(dead_code)]
pub(crate) fn reconcile(
    writer: &SqliteWriter,
    inspections: &InspectionStore,
) -> CapabilityResult<GenerationRecoverySummary> {
    let interrupted_jobs = lifecycle::transaction(writer, |transaction| {
        repository::interrupt_running(transaction, unix_ms())
    })?;
    let known = lifecycle::read(writer, inspection_repository::all_directories)?;
    let orphan_inspections = inspections
        .orphan_directories(&known)
        .map_err(|error| error.to_capability_error())?;
    Ok(GenerationRecoverySummary {
        interrupted_jobs,
        orphan_inspections,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generated_capabilities::errors::CapabilityError;
    use crate::generated_capabilities::generation::contracts::GenerationStatus;
    use crate::generated_capabilities::generation::repository::{
        self as gen_repo, NewGenerationJob,
    };
    use crate::persistence::schema::initialize_database;

    fn job(id: &str) -> NewGenerationJob {
        NewGenerationJob {
            id: id.into(),
            principal_id: "P1".into(),
            conversation_id: crate::PRIMARY_CONVERSATION_ID.into(),
            run_id: format!("run-{id}"),
            input_message_id: format!("msg-{id}"),
            project_id: None,
            request_id: "req-1".into(),
            request_snapshot_json: "{}".into(),
            request_digest: "a".repeat(64),
            capability_id: id.into(),
            base_revision_id: None,
            expected_epoch: 0,
            created_at: gen_repo::unix_ms(),
        }
    }

    #[test]
    fn running_jobs_become_interrupted_and_awaiting_activation_is_kept() {
        let connection = rusqlite::Connection::open_in_memory().unwrap();
        initialize_database(&connection).unwrap();
        gen_repo::insert_job(&connection, &job("job-1")).unwrap();
        gen_repo::insert_job(&connection, &job("job-2")).unwrap();
        gen_repo::cas_status(
            &connection,
            "job-2",
            GenerationStatus::Requested,
            GenerationStatus::Verifying,
            gen_repo::unix_ms(),
        )
        .unwrap();
        gen_repo::insert_job(&connection, &job("job-3")).unwrap();
        gen_repo::cas_status(
            &connection,
            "job-3",
            GenerationStatus::Requested,
            GenerationStatus::AwaitingActivation,
            gen_repo::unix_ms(),
        )
        .unwrap();
        let writer = SqliteWriter::from_connection(connection);

        let data = tempfile::tempdir().unwrap();
        let store = InspectionStore::open(data.path());
        store.ensure_layout().unwrap();
        std::fs::create_dir_all(store.directory("orphan")).unwrap();

        let summary = reconcile(&writer, &store).unwrap();
        assert_eq!(summary.interrupted_jobs, 2);
        assert_eq!(summary.orphan_inspections, vec!["orphan".to_string()]);
        assert!(!store.directory("orphan").exists());

        let read_job = |id: &str| {
            writer
                .read_serialized(|connection| {
                    gen_repo::job_by_id(connection, id).map_err(|error| error.encode())
                })
                .map_err(CapabilityError::decode)
                .unwrap()
        };
        assert_eq!(read_job("job-1").status, GenerationStatus::Interrupted);
        assert_eq!(
            read_job("job-3").status,
            GenerationStatus::AwaitingActivation
        );
    }
}
