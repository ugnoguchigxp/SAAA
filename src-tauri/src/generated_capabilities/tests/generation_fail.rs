use super::generation_flow::{context, registered};
use super::{FixturePackager, GenerationService};
use crate::generated_capabilities::generation::contracts::{GenerateInput, GenerationStatus};
use crate::generated_capabilities::generation::generator::DisabledGenerator;
use crate::generated_capabilities::tests::{candidate_dir, TestEnv, ACCEPTANCE_A, CANDIDATE_A};
use crate::RunCancellation;
use std::sync::Arc;

#[tokio::test]
pub(super) async fn generator_failure_finishes_the_job() {
    let env = TestEnv::start(true);
    let request_a = registered("req-a", CANDIDATE_A, ACCEPTANCE_A, true, false);
    let generation = GenerationService::new(
        env.writer.clone(),
        env.service.clone(),
        vec![request_a],
        Arc::new(DisabledGenerator),
        Arc::new(FixturePackager {
            directory: candidate_dir(CANDIDATE_A),
        }),
        &env.data_directory,
    );
    let receipt = generation
        .generate(
            context("run-fail", "msg-fail"),
            GenerateInput {
                request_id: "req-a".into(),
                base_revision_id: None,
            },
            RunCancellation::default(),
        )
        .await;
    assert_eq!(receipt.status, GenerationStatus::Failed);
    let status: String = env
        .writer
        .read_serialized(|connection| {
            connection
                .query_row(
                    "SELECT status FROM generated_capability_generation_jobs WHERE id = ?1",
                    rusqlite::params![receipt.job_id],
                    |row| row.get(0),
                )
                .map_err(crate::database_error)
        })
        .unwrap();
    assert_eq!(status, "failed");
}
