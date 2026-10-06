//! Fakes and a harness shared by the executor tests.
use super::store::{self, TaskRow};
use super::{admit, Executor, JsonRunner, Runners};
use crate::persistence::SqliteWriter;
use crate::task_queue::Job;
use crate::worker_agents::contracts::*;
use crate::worker_agents::test_support::*;
use crate::RunCancellation;
use async_trait::async_trait;
use rusqlite::params;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub(super) fn route(tier: Tier, fingerprint: &str) -> TierRoute {
    TierRoute {
        tier,
        fingerprint: fingerprint.into(),
        location: if tier == Tier::Cloud {
            RouteLocation::Cloud
        } else {
            RouteLocation::Local
        },
    }
}

pub(super) struct FakeModel {
    pub routes: Vec<TierRoute>,
    pub replies: Mutex<VecDeque<Result<String, String>>>,
    /// (route fingerprint, system, input)
    pub calls: Mutex<Vec<(String, String, String)>>,
}

impl FakeModel {
    pub(super) fn new(routes: Vec<TierRoute>, replies: Vec<Result<String, String>>) -> Arc<Self> {
        Arc::new(Self {
            routes,
            replies: Mutex::new(replies.into()),
            calls: Mutex::new(Vec::new()),
        })
    }
    pub(super) fn fingerprints(&self) -> Vec<String> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .map(|c| c.0.clone())
            .collect()
    }
}

#[async_trait]
impl WorkerModel for FakeModel {
    fn tier_routes(&self) -> Result<Vec<TierRoute>, String> {
        Ok(self.routes.clone())
    }
    async fn complete(
        &self,
        route: &TierRoute,
        system: &str,
        input: &str,
        _cancellation: &RunCancellation,
        _timeout: Duration,
    ) -> Result<String, String> {
        self.calls
            .lock()
            .unwrap()
            .push((route.fingerprint.clone(), system.into(), input.into()));
        self.replies
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| Err("no scripted reply".into()))
    }
}

#[derive(Default)]
pub(super) struct FakeTools {
    pub calls: Mutex<Vec<String>>,
    /// When set the call never completes (simulates a hung or crashed tool).
    pub hang: bool,
}

#[async_trait]
impl ToolRunner for FakeTools {
    async fn run(&self, tool_key: &str, _args: &str, _t: Duration, _c: &RunCancellation) -> String {
        self.calls.lock().unwrap().push(tool_key.into());
        if self.hang {
            std::future::pending::<()>().await;
        }
        r#"{"hits":[]}"#.into()
    }
}

type Hook = Box<dyn Fn(&AttemptEnv<'_>) + Send + Sync>;

/// Scripted attempt runner: pops one result per attempt, optionally running a side effect first.
pub(super) struct ScriptRunner {
    script: Mutex<VecDeque<Result<WorkerOutput, AttemptError>>>,
    pub fingerprints: Mutex<Vec<String>>,
    hook: Option<Hook>,
    block_until_cancelled: bool,
}

impl ScriptRunner {
    pub(super) fn new(script: Vec<Result<WorkerOutput, AttemptError>>) -> Arc<Self> {
        Arc::new(Self {
            script: Mutex::new(script.into()),
            fingerprints: Mutex::new(Vec::new()),
            hook: None,
            block_until_cancelled: false,
        })
    }
    pub(super) fn with_hook(
        script: Vec<Result<WorkerOutput, AttemptError>>,
        hook: impl Fn(&AttemptEnv<'_>) + Send + Sync + 'static,
    ) -> Arc<Self> {
        Arc::new(Self {
            script: Mutex::new(script.into()),
            fingerprints: Mutex::new(Vec::new()),
            hook: Some(Box::new(hook)),
            block_until_cancelled: false,
        })
    }
    pub(super) fn blocking() -> Arc<Self> {
        Arc::new(Self {
            script: Mutex::new(VecDeque::new()),
            fingerprints: Mutex::new(Vec::new()),
            hook: None,
            block_until_cancelled: true,
        })
    }
    pub(super) fn calls(&self) -> usize {
        self.fingerprints.lock().unwrap().len()
    }
}

#[async_trait]
impl AttemptRunner for ScriptRunner {
    async fn run(&self, env: &AttemptEnv<'_>) -> Result<WorkerOutput, AttemptError> {
        self.fingerprints
            .lock()
            .unwrap()
            .push(env.route.fingerprint.clone());
        if let Some(hook) = &self.hook {
            hook(env);
        }
        if self.block_until_cancelled {
            env.cancellation.cancelled().await;
            return Err(AttemptError::Cancelled);
        }
        self.script
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Err(AttemptError::Transport("script exhausted".into())))
    }
}

pub(super) fn json_ok() -> Result<WorkerOutput, AttemptError> {
    Ok(WorkerOutput::JsonV1(serde_json::json!({"answer": 1})))
}

pub(super) fn claims_output(url: &str) -> WorkerOutput {
    WorkerOutput::WebClaimsV1(WebClaims {
        claims: vec![WebClaim {
            text: "桜は例年三月下旬に開花する".into(),
            source_url: url.into(),
            basis: ClaimBasis::Page,
            published_or_fetched_at: None,
        }],
        excluded: ExcludedSummary::default(),
        confidence: Confidence::SingleSource,
        coverage: Coverage::Complete,
    })
}

pub(super) struct Harness {
    pub writer: Arc<SqliteWriter>,
    pub exec: Executor,
    pub revision_id: String,
    pub decision_id: String,
    pub reports: Arc<Mutex<Vec<String>>>,
}

pub(super) const AGENT: &str = "alpha_agent";
pub(super) const MESSAGE_ID: &str = "msg_user_1";

pub(super) struct Setup {
    pub draft: ProfileDraft,
    pub model: Arc<FakeModel>,
    pub tools: Arc<FakeTools>,
    pub web: Arc<dyn AttemptRunner>,
    pub json: Arc<dyn AttemptRunner>,
}

impl Setup {
    /// Default: json profile, one local rung, `JsonRunner` over a model with no replies.
    pub(super) fn new() -> Self {
        Self {
            draft: sample_draft(AGENT, "answers questions"),
            model: FakeModel::new(vec![route(Tier::Local, "l0")], vec![]),
            tools: Arc::new(FakeTools::default()),
            web: ScriptRunner::new(vec![]),
            json: Arc::new(JsonRunner),
        }
    }
}

impl Harness {
    pub(super) fn build(setup: Setup) -> Self {
        let connection = fresh_db();
        insert_user_message(&connection, MESSAGE_ID, "桜の開花を調べて");
        let revision_id = insert_revision(&connection, &setup.draft, ReviewState::Approved, true);
        connection
            .execute(
                "INSERT INTO worker_discovery_decisions(id, conversation_id, input_message_id, job_key,
                    registry_epoch, acl_epoch, model_hash, status, created_at_ms)
                 VALUES('dec_1', ?1, ?2, 'job_1', 0, 0, NULL, 'ok', 1)",
                params![crate::PRIMARY_CONVERSATION_ID, MESSAGE_ID],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO worker_discovery_candidates(decision_id, profile_revision_id, score,
                    final_rank, offered, pinned) VALUES('dec_1', ?1, 1.0, 1, 1, 0)",
                params![revision_id],
            )
            .unwrap();
        let writer = Arc::new(SqliteWriter::from_connection(connection));
        let reports = Arc::new(Mutex::new(Vec::new()));
        let sink = reports.clone();
        let exec = Executor::new(
            writer.clone(),
            setup.model,
            setup.tools,
            Runners {
                web_claims: setup.web,
                json: setup.json,
            },
        )
        .with_report_hook(Arc::new(move |conversation_id| {
            sink.lock().unwrap().push(conversation_id.to_string());
        }));
        Self {
            writer,
            exec,
            revision_id,
            decision_id: "dec_1".into(),
            reports,
        }
    }

    pub(super) fn admit_with(
        &self,
        agent: &str,
        input: serde_json::Value,
        conversation_deadline_ms: i64,
    ) -> Result<WorkerOutcome, String> {
        let delegate = DelegateRequest {
            agent: agent.into(),
            input,
        };
        let request = AdmitRequest {
            conversation_id: crate::PRIMARY_CONVERSATION_ID,
            input_message_id: MESSAGE_ID,
            origin_job_key: "job_1",
            decision_id: &self.decision_id,
            delegate: &delegate,
            conversation_deadline_ms,
        };
        self.writer
            .transact(|c| admit(c, &request, crate::worker_agents::loader::now_ms()))
    }

    pub(super) fn admit(&self) -> WorkerOutcome {
        self.admit_with(
            AGENT,
            serde_json::json!({"query": "桜"}),
            crate::worker_agents::loader::now_ms() + 120_000,
        )
        .unwrap()
    }

    pub(super) fn admit_task_id(&self) -> String {
        match self.admit() {
            WorkerOutcome::Pending { task_id } => task_id,
            other => panic!("expected pending admission, got {other:?}"),
        }
    }

    pub(super) fn claim(&self) -> Option<Job> {
        self.writer
            .write(|c| crate::task_queue::claim(c, WORKER_LANE))
            .unwrap()
    }

    /// Claims the next worker job and processes it to completion.
    pub(super) async fn run_next(&self) -> Result<(), String> {
        let job = self.claim().expect("a queued worker job");
        self.exec.process(&job, &RunCancellation::default()).await
    }

    pub(super) fn task(&self, task_id: &str) -> TaskRow {
        self.writer
            .read_serialized(|c| store::load_task(c, task_id))
            .unwrap()
            .expect("task row")
    }

    pub(super) fn count(&self, sql: &str) -> i64 {
        self.writer
            .read_serialized(|c| {
                c.query_row(sql, [], |row| row.get(0))
                    .map_err(|e| e.to_string())
            })
            .unwrap()
    }

    pub(super) fn exec_sql(&self, sql: &str) {
        self.writer
            .read_serialized(|c| c.execute_batch(sql).map_err(|e| e.to_string()))
            .unwrap();
    }
}
