use super::generation_flow::{context, registered};
use super::{FixturePackager, GenerationService, SequencePackager};
use crate::generated_capabilities::contracts::{CallActor, InvokeRequest};
use crate::generated_capabilities::errors::CapabilityErrorCode;
use crate::generated_capabilities::generation::contracts::{
    GenerateInput, GenerationErrorCode, GenerationStatus,
};
use crate::generated_capabilities::generation::generator::{FakeBody, FakeGenerator, Generator};
use crate::generated_capabilities::host::process::Cancellation;
use crate::generated_capabilities::inspection::comparison::CaseEvaluator;
use crate::generated_capabilities::inspection::contracts::{
    tests as inspection_fixtures, InspectionContext, InspectionReport,
};
use crate::generated_capabilities::inspection::repository::{
    self as inspection_repository, NewInspection,
};
use crate::generated_capabilities::inspection::service::{
    InspectionService, InspectionStore, Inspector,
};
use crate::generated_capabilities::lifecycle;
use crate::generated_capabilities::tests::{
    candidate_dir, object, TestEnv, ACCEPTANCE_A, ACCEPTANCE_B, CANDIDATE_A, CANDIDATE_B,
};
use crate::runtime::capability_commands::load_stored_inspection;
use crate::RunCancellation;
use async_trait::async_trait;
use serde_json::{json, Map, Value};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

fn hash64() -> String {
    "a".repeat(64)
}

fn actor() -> CallActor {
    CallActor {
        principal_id: "principal-flow".into(),
        conversation_id: crate::PRIMARY_CONVERSATION_ID.into(),
        project_id: None,
        run_id: "run-a".into(),
    }
}

fn capability_id(env: &TestEnv, revision_id: &str) -> String {
    env.writer
        .read_serialized(|connection| {
            connection
                .query_row(
                    "SELECT capability_id FROM generated_capability_revisions WHERE id = ?1",
                    rusqlite::params![revision_id],
                    |row| row.get(0),
                )
                .map_err(crate::database_error)
        })
        .expect("capability id")
}

fn job_status(env: &TestEnv, job_id: &str) -> String {
    env.writer
        .read_serialized(|connection| {
            connection
                .query_row(
                    "SELECT status FROM generated_capability_generation_jobs WHERE id = ?1",
                    rusqlite::params![job_id],
                    |row| row.get(0),
                )
                .map_err(crate::database_error)
        })
        .expect("job status")
}

fn grants(env: &TestEnv) -> i64 {
    env.scalar("SELECT COUNT(*) FROM tool_selection_grants")
}

fn catalog_enabled(env: &TestEnv) -> i64 {
    env.scalar("SELECT COUNT(*) FROM tool_selection_catalog WHERE enabled = 1")
}

async fn generate_a(env: &TestEnv) -> (GenerationService, Arc<FakeGenerator>, String) {
    let request_a = registered("req-a", CANDIDATE_A, ACCEPTANCE_A, true, false);
    let request_b = registered("req-b", CANDIDATE_B, ACCEPTANCE_B, false, true);
    let fake = Arc::new(FakeGenerator::new(
        &request_a,
        FakeBody::EnabledAndNotSuspended,
    ));
    let generation = GenerationService::new(
        env.writer.clone(),
        env.service.clone(),
        vec![request_a, request_b],
        fake.clone(),
        Arc::new(SequencePackager::new(vec![
            candidate_dir(CANDIDATE_A),
            candidate_dir(CANDIDATE_B),
        ])),
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
    (generation, fake, first.revision_id.expect("revision A"))
}

async fn invoke_on(env: &TestEnv, call_id: &str, revision_id: &str) -> String {
    let capability_id = capability_id(env, revision_id);
    let resolved = env.service.resolve_active(&capability_id).expect("active");
    let mut request = InvokeRequest::new(
        resolved,
        call_id.into(),
        object(json!({ "enabled": true, "suspended": false })),
    );
    request.actor = Some(actor());
    request.origin = "conversation";
    let invoked = env
        .service
        .invoke(request, &Cancellation::default())
        .await
        .expect("invoke");
    assert_eq!(invoked.revision_id, revision_id);
    capability_id
}

fn bind_typescript(env: &TestEnv, revision_id: &str, inspection_id: &str, typescript: &str) {
    let revision = lifecycle::read(&env.writer, |connection| {
        crate::generated_capabilities::repository::revision_by_id(connection, revision_id)
    })
    .expect("revision");
    let store = InspectionStore::open(&env.data_directory);
    let report = json!({ "typescript": { "source": typescript } });
    store
        .publish(
            inspection_id,
            typescript,
            &serde_json::to_vec(&report).expect("report"),
        )
        .expect("publish inspection");
    lifecycle::transaction(&env.writer, |transaction| {
        inspection_repository::insert(
            transaction,
            &NewInspection {
                id: inspection_id.into(),
                revision_id: revision_id.into(),
                inspector_digest: hash64(),
                package_hash: if revision.package_hash.len() == 64 {
                    revision.package_hash
                } else {
                    hash64()
                },
                source_hash: revision.source_hash.unwrap_or_else(hash64),
                program_hash: revision.program_hash.unwrap_or_else(hash64),
                artifact_hash: revision.artifact_hash.unwrap_or_else(hash64),
                projection_hash: hash64(),
                relative_directory: inspection_id.into(),
                comparison_json: "{}".into(),
                created_at: 1,
            },
        )
    })
    .expect("record inspection");
}

struct FixtureInspector(InspectionReport);

impl Inspector for FixtureInspector {
    fn inspect(
        &self,
        _package_directory: &Path,
        _package_hash: &str,
    ) -> crate::generated_capabilities::inspection::contracts::InspectionResult<InspectionReport>
    {
        Ok(self.0.clone())
    }
}

struct SameEval;

impl CaseEvaluator for SameEval {
    fn evaluate(&self, _input: &Map<String, Value>) -> Result<bool, String> {
        Ok(true)
    }
}

struct HoldGenerator;

#[async_trait]
impl Generator for HoldGenerator {
    async fn generate(
        &self,
        _prompt: &crate::generated_capabilities::generation::generator::GenerationPrompt,
        cancellation: &RunCancellation,
    ) -> Result<String, GenerationErrorCode> {
        loop {
            if cancellation.is_cancelled() {
                return Err(GenerationErrorCode::Cancelled);
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}

struct GatedGenerator {
    inner: FakeGenerator,
    started: Arc<AtomicBool>,
    release: Arc<AtomicBool>,
}

#[async_trait]
impl Generator for GatedGenerator {
    async fn generate(
        &self,
        prompt: &crate::generated_capabilities::generation::generator::GenerationPrompt,
        cancellation: &RunCancellation,
    ) -> Result<String, GenerationErrorCode> {
        self.started.store(true, Ordering::SeqCst);
        while !self.release.load(Ordering::SeqCst) {
            if cancellation.is_cancelled() {
                return Err(GenerationErrorCode::Cancelled);
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        self.inner.generate(prompt, cancellation).await
    }
}

#[tokio::test]
async fn gc_01_fixture_inspector_stores_display_without_a_live_kit() {
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
        fake,
        Arc::new(FixturePackager {
            directory: candidate_dir(CANDIDATE_A),
        }),
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
    let revision_a = first.revision_id.expect("revision A");
    invoke_on(&env, "call-inspect-1", &revision_a).await;
    let revision = lifecycle::read(&env.writer, |connection| {
        crate::generated_capabilities::repository::revision_by_id(connection, &revision_a)
    })
    .expect("revision");
    let mut report = inspection_fixtures::sample_report();
    report.package_hash = revision.package_hash.clone();
    let source = revision.source_hash.clone().unwrap_or_else(hash64);
    let program = revision.program_hash.clone().unwrap_or_else(hash64);
    let artifact = revision.artifact_hash.clone().unwrap_or_else(hash64);
    report.artifacts.source_hash = source.clone();
    report.artifacts.program_hash = program.clone();
    report.artifacts.artifact_hash = artifact;
    report.typescript.source_hash = source;
    report.typescript.program_hash = program;
    report.typescript.source = "export const marker = \"fixture-on-demand\";\n".into();
    report.contract.input = serde_json::from_str(&revision.contract_json).expect("contract");
    let service = InspectionService::new(&env.data_directory, hash64());
    let receipt = service
        .inspect_execution(
            &env.writer,
            &InspectionContext {
                principal_id: "principal-flow".into(),
                conversation_id: crate::PRIMARY_CONVERSATION_ID.into(),
                project_id: None,
            },
            "call-inspect-1",
            &FixtureInspector(report),
            &SameEval,
            &SameEval,
        )
        .expect("fixture inspect");
    assert!(receipt.typescript_text.contains("fixture-on-demand"));
    let shown = load_stored_inspection(
        &env.writer,
        &env.data_directory,
        "principal-flow",
        "call-inspect-1",
    )
    .expect("stored display");
    assert_eq!(shown.typescript_text, receipt.typescript_text);
}

#[tokio::test]
async fn gc_02_inspect_after_b_keeps_revision_a_typescript() {
    let env = TestEnv::start(true);
    let (generation, fake, revision_a) = generate_a(&env).await;
    invoke_on(&env, "call-a-10", &revision_a).await;
    bind_typescript(
        &env,
        &revision_a,
        "insp-a",
        "export const marker = \"rev-A\";\n",
    );
    let before = load_stored_inspection(
        &env.writer,
        &env.data_directory,
        "principal-flow",
        "call-a-10",
    )
    .expect("inspect A");
    assert!(before.typescript_text.contains("rev-A"));
    assert_eq!(before.revision_id, revision_a);

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
    let revision_b = second.revision_id.expect("revision B");
    assert_ne!(revision_b, revision_a);
    bind_typescript(
        &env,
        &revision_b,
        "insp-b",
        "export const marker = \"rev-B\";\n",
    );
    let after = load_stored_inspection(
        &env.writer,
        &env.data_directory,
        "principal-flow",
        "call-a-10",
    )
    .expect("inspect A after B");
    assert_eq!(after.revision_id, revision_a);
    assert!(after.typescript_text.contains("rev-A"));
    assert!(!after.typescript_text.contains("rev-B"));
}

#[tokio::test]
async fn gc_03_wrong_acceptance_keeps_a_and_does_not_grant() {
    let env = TestEnv::start(true);
    let request_a = registered("req-a", CANDIDATE_A, ACCEPTANCE_A, true, false);
    let mut request_b = registered("req-b", CANDIDATE_B, ACCEPTANCE_A, false, true);
    request_b.acceptance_id = ACCEPTANCE_A.into();
    let fake = Arc::new(FakeGenerator::new(
        &request_a,
        FakeBody::EnabledAndNotSuspended,
    ));
    let generation = GenerationService::new(
        env.writer.clone(),
        env.service.clone(),
        vec![request_a, request_b],
        fake.clone(),
        Arc::new(SequencePackager::new(vec![
            candidate_dir(CANDIDATE_A),
            candidate_dir(CANDIDATE_B),
        ])),
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
    let grants_after_a = grants(&env);
    assert_eq!(grants_after_a, 1);
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
    assert_eq!(second.status, GenerationStatus::Failed, "{second:?}");
    assert_eq!(
        second.error_code,
        Some(GenerationErrorCode::AcceptanceFailed)
    );
    assert_eq!(job_status(&env, &second.job_id), "failed");
    assert_eq!(
        env.service
            .resolve_active(&capability_id(&env, &revision_a))
            .expect("A remains")
            .revision_id,
        revision_a
    );
    assert_eq!(grants(&env), grants_after_a);
}

#[tokio::test]
async fn gc_04_suspend_unpublishes_catalog_and_keeps_past_inspect() {
    let env = TestEnv::start(true);
    let (generation, _fake, revision_a) = generate_a(&env).await;
    let _ = generation;
    let capability_id = invoke_on(&env, "call-a-10", &revision_a).await;
    bind_typescript(
        &env,
        &revision_a,
        "insp-a",
        "export const marker = \"rev-A\";\n",
    );
    assert!(catalog_enabled(&env) >= 1);
    let epoch = env.service.catalog_epoch(&capability_id).expect("epoch");
    env.service
        .suspend_revision(&revision_a, epoch)
        .expect("suspend");
    assert_eq!(catalog_enabled(&env), 0);
    assert!(env.service.resolve_active(&capability_id).is_err());
    let shown = load_stored_inspection(
        &env.writer,
        &env.data_directory,
        "principal-flow",
        "call-a-10",
    )
    .expect("past inspect");
    assert!(shown.typescript_text.contains("rev-A"));
    let epoch = env.service.catalog_epoch(&capability_id).expect("epoch");
    env.service
        .retire_revision(&revision_a, epoch)
        .expect("retire after suspend");
    assert!(env.service.resolve_active(&capability_id).is_err());
    let shown = load_stored_inspection(
        &env.writer,
        &env.data_directory,
        "principal-flow",
        "call-a-10",
    )
    .expect("inspect after retire");
    assert_eq!(shown.revision_id, revision_a);
}

#[tokio::test]
async fn gc_05_cancel_and_timeout_finish_the_job() {
    let env = TestEnv::start(true);
    let request_a = registered("req-a", CANDIDATE_A, ACCEPTANCE_A, true, false);
    let generation = Arc::new(GenerationService::new(
        env.writer.clone(),
        env.service.clone(),
        vec![request_a],
        Arc::new(HoldGenerator),
        Arc::new(FixturePackager {
            directory: candidate_dir(CANDIDATE_A),
        }),
        &env.data_directory,
    ));
    let cancellation = RunCancellation::default();
    let task = {
        let generation = generation.clone();
        let cancellation = cancellation.clone();
        tokio::spawn(async move {
            generation
                .generate(
                    context("run-cancel", "msg-cancel"),
                    GenerateInput {
                        request_id: "req-a".into(),
                        base_revision_id: None,
                    },
                    cancellation,
                )
                .await
        })
    };
    let started = tokio::time::Instant::now();
    loop {
        if env.scalar("SELECT COUNT(*) FROM generated_capability_generation_jobs") > 0 {
            break;
        }
        if started.elapsed() > Duration::from_secs(2) {
            panic!("generation job was not inserted");
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    cancellation.cancel();
    let cancelled = task.await.expect("join");
    assert_eq!(
        cancelled.status,
        GenerationStatus::Cancelled,
        "{cancelled:?}"
    );
    assert_eq!(job_status(&env, &cancelled.job_id), "cancelled");
    assert_eq!(
        generation.reconcile().expect("reconcile").interrupted_jobs,
        0
    );

    let timed = generation
        .generate(
            context("run-timeout", "msg-timeout"),
            GenerateInput {
                request_id: "req-a".into(),
                base_revision_id: None,
            },
            RunCancellation::default(),
        )
        .await;
    assert_eq!(timed.status, GenerationStatus::Failed, "{timed:?}");
    assert_eq!(timed.error_code, Some(GenerationErrorCode::BudgetExceeded));
    assert_eq!(job_status(&env, &timed.job_id), "failed");
}

#[tokio::test]
async fn gc_06_epoch_change_during_update_conflicts() {
    let env = TestEnv::start(true);
    let request_a = registered("req-a", CANDIDATE_A, ACCEPTANCE_A, true, false);
    let request_b = registered("req-b", CANDIDATE_B, ACCEPTANCE_B, false, true);
    let started = Arc::new(AtomicBool::new(false));
    let release = Arc::new(AtomicBool::new(false));
    let fake_a = FakeGenerator::new(&request_a, FakeBody::EnabledAndNotSuspended);
    let generation_a = GenerationService::new(
        env.writer.clone(),
        env.service.clone(),
        vec![request_a.clone(), request_b.clone()],
        Arc::new(fake_a),
        Arc::new(SequencePackager::new(vec![
            candidate_dir(CANDIDATE_A),
            candidate_dir(CANDIDATE_B),
        ])),
        &env.data_directory,
    );
    let first = generation_a
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
    let revision_a = first.revision_id.expect("revision A");
    let capability_id = capability_id(&env, &revision_a);
    let gated = GatedGenerator {
        inner: FakeGenerator::new(&request_b, FakeBody::EnabledOnly),
        started: started.clone(),
        release: release.clone(),
    };
    let generation_b = Arc::new(GenerationService::new(
        env.writer.clone(),
        env.service.clone(),
        vec![request_a, request_b],
        Arc::new(gated),
        Arc::new(SequencePackager::new(vec![candidate_dir(CANDIDATE_B)])),
        &env.data_directory,
    ));
    let task = {
        let generation_b = generation_b.clone();
        let revision_a = revision_a.clone();
        tokio::spawn(async move {
            generation_b
                .generate(
                    context("run-b", "msg-b"),
                    GenerateInput {
                        request_id: "req-b".into(),
                        base_revision_id: Some(revision_a),
                    },
                    RunCancellation::default(),
                )
                .await
        })
    };
    let wait = tokio::time::Instant::now();
    while !started.load(Ordering::SeqCst) {
        if wait.elapsed() > Duration::from_secs(2) {
            panic!("update generator did not start");
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    env.writer
        .write(|connection| {
            connection
                .execute(
                    "UPDATE generated_capabilities SET catalog_epoch = catalog_epoch + 1 WHERE id = ?1",
                    rusqlite::params![capability_id],
                )
                .unwrap();
            Ok(())
        })
        .expect("bump epoch");
    release.store(true, Ordering::SeqCst);
    let second = task.await.expect("join");
    assert_eq!(second.status, GenerationStatus::Conflict, "{second:?}");
    assert_eq!(
        env.service
            .resolve_active(&capability_id)
            .expect("A still active")
            .revision_id,
        revision_a
    );
}

#[tokio::test]
async fn gc_06_other_principal_cannot_load_stored_inspection() {
    let env = TestEnv::start(true);
    let (_generation, _fake, revision_a) = generate_a(&env).await;
    invoke_on(&env, "call-a-10", &revision_a).await;
    bind_typescript(
        &env,
        &revision_a,
        "insp-a",
        "export const marker = \"rev-A\";\n",
    );
    assert_eq!(
        load_stored_inspection(&env.writer, &env.data_directory, "other-user", "call-a-10")
            .unwrap_err()
            .code,
        CapabilityErrorCode::NotActive
    );
}
