use crate::generated_capabilities::contracts::sha256_hex;
use crate::generated_capabilities::contracts::{CallActor, InvokeRequest};
use crate::generated_capabilities::generation::config::{RegisteredRequest, RequestScope};
use crate::generated_capabilities::generation::contracts::{
    GenerateInput, GenerationContext, GenerationStatus,
};
use crate::generated_capabilities::generation::generator::{FakeBody, FakeGenerator};
use crate::generated_capabilities::generation::service::{
    FixturePackager, GenerationService, SequencePackager,
};
use crate::generated_capabilities::host::process::Cancellation;
use crate::generated_capabilities::tests::{
    candidate_dir, object, TestEnv, ACCEPTANCE_A, ACCEPTANCE_B, CANDIDATE_A, CANDIDATE_B,
};
use crate::RunCancellation;
use serde_json::json;
use std::fs;
use std::sync::Arc;

pub(super) fn hashed(path: &std::path::Path) -> (std::path::PathBuf, String) {
    let bytes = fs::read(path).expect("fixture file");
    (path.to_path_buf(), sha256_hex(&bytes))
}

pub(super) fn registered(
    id: &str,
    candidate: &str,
    acceptance_id: &str,
    allow_create: bool,
    allow_update: bool,
) -> RegisteredRequest {
    let root = candidate_dir(candidate);
    let (request_path, request_hash) = hashed(&root.join("request.json"));
    let (suite_path, suite_hash) = hashed(&root.join("tests.json"));
    let (metadata_path, metadata_hash) = hashed(&root.join("capability.json"));
    RegisteredRequest {
        id: id.into(),
        capability_id: "enabled-user".into(),
        purpose: "利用者の受付可否を返す。".into(),
        fields: vec!["enabled".into(), "suspended".into()],
        request_path,
        request_hash,
        suite_path,
        suite_hash,
        metadata_path,
        metadata_hash,
        acceptance_id: acceptance_id.into(),
        scope: RequestScope::User,
        allow_create,
        allow_update,
        auto_activate: true,
        grant_on_create: true,
    }
}

pub(super) fn context(run: &str, message: &str) -> GenerationContext {
    GenerationContext {
        principal_id: "principal-flow".into(),
        conversation_id: crate::PRIMARY_CONVERSATION_ID.into(),
        run_id: run.into(),
        input_message_id: message.into(),
        project_id: None,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
pub(super) async fn rw_13_generate_a_invoke_update_b_and_keep_prior_call() {
    let env = TestEnv::start(true);
    let request_a = registered("req-a", CANDIDATE_A, ACCEPTANCE_A, true, false);
    let request_b = registered("req-b", CANDIDATE_B, ACCEPTANCE_B, false, true);
    let fake = Arc::new(FakeGenerator::new(
        &request_a,
        FakeBody::EnabledAndNotSuspended,
    ));
    let packager = Arc::new(SequencePackager::new(vec![
        candidate_dir(CANDIDATE_A),
        candidate_dir(CANDIDATE_B),
    ]));
    let generation = GenerationService::new(
        env.writer.clone(),
        env.service.clone(),
        vec![request_a, request_b],
        fake.clone(),
        packager,
        &env.data_directory,
    );
    let first = generation
        .generate(
            context("run-a", "msg-a"),
            GenerateInput {
                request_id: "req-a".into(),
                base_revision_id: None,
            },
            RunCancellation::default(),
        )
        .await;
    assert_eq!(first.status, GenerationStatus::Active, "{first:?}");
    let revision_a = first.revision_id.clone().expect("revision A");
    let capability_id: String = env
        .writer
        .read_serialized(|connection| {
            connection
                .query_row(
                    "SELECT capability_id FROM generated_capability_revisions WHERE id = ?1",
                    rusqlite::params![revision_a],
                    |row| row.get(0),
                )
                .map_err(crate::database_error)
        })
        .expect("capability id");
    let replay = generation
        .generate(
            context("run-a", "msg-a"),
            GenerateInput {
                request_id: "req-a".into(),
                base_revision_id: None,
            },
            RunCancellation::default(),
        )
        .await;
    assert_eq!(replay.job_id, first.job_id);
    assert_eq!(fake.model_calls(), 1);

    let resolved = env
        .service
        .resolve_active(&capability_id)
        .expect("active after A");
    let actor = CallActor {
        principal_id: "principal-flow".into(),
        conversation_id: crate::PRIMARY_CONVERSATION_ID.into(),
        project_id: None,
        run_id: "run-a".into(),
    };
    let mut call_a = InvokeRequest::new(
        resolved.clone(),
        "call-a-10".into(),
        object(json!({ "enabled": true, "suspended": false })),
    );
    call_a.actor = Some(actor.clone());
    call_a.origin = "conversation";
    let invoked = env
        .service
        .invoke(call_a, &Cancellation::default())
        .await
        .expect("A call");
    assert!(invoked.value);
    assert_eq!(invoked.revision_id, revision_a);

    fake.set_body(FakeBody::EnabledOnly);
    let second = generation
        .generate(
            context("run-b", "msg-b"),
            GenerateInput {
                request_id: "req-b".into(),
                base_revision_id: Some(revision_a.clone()),
            },
            RunCancellation::default(),
        )
        .await;
    assert_eq!(second.status, GenerationStatus::Active, "{second:?}");
    let resolved_b = env
        .service
        .resolve_active(&capability_id)
        .expect("active after B");
    assert_ne!(resolved_b.revision_id, revision_a);
    let mut call_kept = InvokeRequest::new(
        resolved.clone(),
        "call-a-repeat".into(),
        object(json!({ "enabled": true, "suspended": false })),
    );
    call_kept.actor = Some(actor);
    call_kept.origin = "conversation";
    // The prior call row stays bound to revision A even after B is active.
    let stored: String = env
        .writer
        .read_serialized(|connection| {
            connection
                .query_row(
                    "SELECT revision_id FROM generated_capability_calls WHERE id = 'call-a-10'",
                    [],
                    |row| row.get(0),
                )
                .map_err(crate::database_error)
        })
        .expect("stored A call");
    assert_eq!(stored, revision_a);
}

#[tokio::test]
pub(super) async fn rw_05_reconcile_does_not_regenerate() {
    let env = TestEnv::start(true);
    let request_a = registered("req-a", CANDIDATE_A, ACCEPTANCE_A, true, false);
    let fake = Arc::new(FakeGenerator::new(
        &request_a,
        FakeBody::EnabledAndNotSuspended,
    ));
    let generation = GenerationService::new(
        env.writer.clone(),
        env.service.clone(),
        vec![request_a],
        fake.clone(),
        Arc::new(FixturePackager {
            directory: candidate_dir(CANDIDATE_A),
        }),
        &env.data_directory,
    );
    let summary = generation.reconcile().expect("reconcile");
    assert_eq!(summary.interrupted_jobs, 0);
    assert_eq!(fake.model_calls(), 0);
}
