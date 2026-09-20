#![cfg(test)]
#![allow(dead_code)]

//! Deterministic M2A fixtures. Synthetic DB, fixed clock, fake coding owner.
//! No ASR, model, pi process or external service is started.

use super::runtime_frame::{GraphRequest, WorldFrameService};
use super::test_support::{insert_source, v2_entity_assertion, writer_db, Committer, PROJECT};
use crate::persistence::sqlite::{SqliteReaders, SqliteWriter};
use crate::runtime::context::scope;
use rusqlite::params;
use saaa_personal_state_core::world::model_v2::EntityKindV2;
use saaa_personal_state_core::world::runtime_frame::RuntimeRef;
use saaa_personal_state_core::world::traversal_v2::{CausalDirection, LimitsV2};
use saaa_personal_state_core::{AccessRequest, Classification, Purpose};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;

pub(crate) const RUN_ID: &str = "run1";
pub(crate) const MESSAGE_ID: &str = "msg1";
pub(crate) const CODING_ID: &str = "j1";
pub(crate) const CODING_SOURCE: &str = "csrc1";
pub(crate) const START_MS: i64 = 1_000;
pub(crate) const TTL_MS: u64 = 1_000;

pub(crate) struct Fixture {
    pub(crate) writer: Arc<SqliteWriter>,
    pub(crate) clock: Arc<AtomicI64>,
    pub(crate) principal: String,
    pub(crate) policy_revision: u64,
    pub(crate) project: String,
    _tempdir: Option<tempfile::TempDir>,
    db_path: Option<std::path::PathBuf>,
}

impl Fixture {
    pub(crate) fn new(targets: &[(&str, &str)]) -> Self {
        Self::build(targets, 0, false)
    }

    pub(crate) fn with_entities(targets: &[(&str, &str)], entities: usize) -> Self {
        Self::build(targets, entities, false)
    }

    /// `pending_review=true` marks the World projection as pending so the graph
    /// reader returns the pending notice.
    pub(crate) fn build(targets: &[(&str, &str)], entities: usize, pending_review: bool) -> Self {
        let writer = Arc::new(writer_db());
        Self::assemble(writer, targets, entities, pending_review, None, None)
    }

    /// A file-backed fixture so the persistent `SqliteReaders::open` path (with
    /// real read transactions) can be exercised.
    pub(crate) fn file(targets: &[(&str, &str)], entities: usize, pending_review: bool) -> Self {
        let directory = tempfile::tempdir().expect("temp dir");
        let path = directory.path().join("frame.sqlite3");
        let writer = Arc::new(SqliteWriter::open(&path).expect("writer opens"));
        Self::assemble(
            writer,
            targets,
            entities,
            pending_review,
            Some(directory),
            Some(path),
        )
    }

    fn assemble(
        writer: Arc<SqliteWriter>,
        targets: &[(&str, &str)],
        entities: usize,
        pending_review: bool,
        tempdir: Option<tempfile::TempDir>,
        db_path: Option<std::path::PathBuf>,
    ) -> Self {
        let principal: String = writer
            .read_serialized(|c| {
                c.query_row(
                    "SELECT principal FROM personal_scope WHERE id='primary'",
                    [],
                    |r| r.get(0),
                )
                .map_err(crate::database_error)
            })
            .expect("principal");
        let policy_revision: u64 = writer
            .read_serialized(|c| {
                c.query_row(
                    "SELECT policy_revision FROM personal_scope WHERE id='primary'",
                    [],
                    |r| r.get(0),
                )
                .map_err(crate::database_error)
            })
            .expect("policy revision");
        let project = PROJECT.to_string();
        let targets_owned: Vec<(String, String)> = targets
            .iter()
            .map(|(kind, id)| (kind.to_string(), id.to_string()))
            .collect();
        let project_id = project
            .strip_prefix("project:")
            .expect("project prefix")
            .to_string();
        // 1. Frame message, run and recorded scope. The epoch is fixed at 1 so
        //    the later world source's ensure_scope cannot invalidate it.
        writer
            .write(|c| {
                c.execute(
                    "INSERT INTO conversation_messages(id,conversation_id,role,content,created_at)
                     VALUES(?1,?2,'user','hello','1000')",
                    params![MESSAGE_ID, crate::PRIMARY_CONVERSATION_ID],
                )
                .map_err(crate::database_error)?;
                c.execute(
                    "INSERT INTO runtime_runs(id,conversation_id,route_kind,status,input_message_id,started_at)
                     VALUES(?1,?2,'conversation.respond','running',?3,'1000')",
                    params![RUN_ID, crate::PRIMARY_CONVERSATION_ID, MESSAGE_ID],
                )
                .map_err(crate::database_error)?;
                scope::register(c, "project", &project_id)?;
                let mut refs = vec![crate::TurnScopeRef {
                    kind: "project".into(),
                    id: project_id.clone(),
                    relation: "focus".into(),
                }];
                for (kind, id) in &targets_owned {
                    let target = scope::register(c, kind, id)?;
                    scope::link(c, &project, &target)?;
                    refs.push(crate::TurnScopeRef {
                        kind: kind.clone(),
                        id: id.clone(),
                        relation: "current".into(),
                    });
                }
                c.execute(
                    "UPDATE context_scope_epochs SET epoch=1 WHERE scope_key=?1",
                    [&project],
                )
                .map_err(crate::database_error)?;
                for (kind, id) in &targets_owned {
                    c.execute(
                        "UPDATE context_scope_epochs SET epoch=1 WHERE scope_key=?1",
                        [format!("{kind}:{id}")],
                    )
                    .map_err(crate::database_error)?;
                }
                let input = crate::StartTurnInput {
                    run_id: RUN_ID.to_string(),
                    conversation_id: crate::PRIMARY_CONVERSATION_ID.to_string(),
                    content: "hello".to_string(),
                    workspace_path: None,
                    retry_input_message_id: None,
                    source_id: None,
                    scope_refs: refs,
                    input_origin: "text".into(),
                    presentation_mode: "visual".into(),
                };
                scope::resolve(c, &input, MESSAGE_ID, false)?;
                c.execute(
                    "UPDATE personal_jobs SET status='completed' WHERE source_sequence=(
                       SELECT sequence FROM personal_sources WHERE message_id=?1)",
                    [MESSAGE_ID],
                )
                .map_err(crate::database_error)?;
                Ok(())
            })
            .expect("frame scope setup");

        // 2. World assertions. Rebuild records the current input_epoch so the
        //    graph projection is fresh for the frame request.
        if entities > 0 {
            let now_ms = crate::memory::personal_state::now();
            let source = writer
                .write(|c| Ok(insert_source(c, PROJECT, "s1", "world source")))
                .expect("source");
            let mut committer = Committer {
                writer: writer.as_ref(),
                project: PROJECT,
            };
            let mut created = 0usize;
            while created < entities {
                let batch: Vec<_> = (created..(created + 8).min(entities))
                    .map(|index| {
                        v2_entity_assertion(
                            &format!("e{index}"),
                            &format!("ent{index}"),
                            EntityKindV2::Concept,
                            &format!("名前{index}"),
                            &[],
                            None,
                            &source,
                            PROJECT,
                            now_ms,
                        )
                    })
                    .collect();
                let batch_len = batch.len();
                committer
                    .commit(&format!("m2-entities-{created}"), batch)
                    .expect("entities commit");
                created += batch_len;
            }
            if pending_review {
                // A source mapped to the project with no committed assertion keeps
                // the projection in pending review. Rebuild keeps the projection
                // header fresh so the pending (not stale) path is taken.
                writer
                    .write(|c| {
                        insert_source(c, PROJECT, "s_pending", "pending source");
                        crate::memory::personal_state::store::rebuild(
                            c,
                            crate::memory::personal_state::now(),
                        )?;
                        Ok(())
                    })
                    .expect("pending source");
            }
        }

        Self {
            writer,
            clock: Arc::new(AtomicI64::new(START_MS)),
            principal,
            policy_revision,
            project,
            _tempdir: tempdir,
            db_path,
        }
    }

    pub(crate) fn readers_open(&self) -> SqliteReaders {
        SqliteReaders::open(self.db_path.as_ref().expect("file fixture path"))
            .expect("readers open")
    }

    pub(crate) fn readers(&self) -> SqliteReaders {
        SqliteReaders::serialized(self.writer.clone())
    }

    pub(crate) fn service(&self) -> WorldFrameService {
        let clock = self.clock.clone();
        let clock_fn: Arc<dyn Fn() -> i64 + Send + Sync> =
            Arc::new(move || clock.load(Ordering::SeqCst));
        WorldFrameService::new(self.readers(), clock_fn)
    }

    pub(crate) fn now(&self) -> i64 {
        self.clock.load(Ordering::SeqCst)
    }

    pub(crate) fn set_now(&self, value: i64) {
        self.clock.store(value, Ordering::SeqCst);
    }

    pub(crate) fn access(&self) -> AccessRequest<'_> {
        AccessRequest {
            principal: &self.principal,
            scope: "primary",
            task_request: Some(&self.project),
            purpose: Purpose::Reasoning,
            max_classification: Classification::Confidential,
            policy_revision: self.policy_revision,
            authorized: true,
        }
    }

    pub(crate) fn request<'a>(
        &'a self,
        access: AccessRequest<'a>,
        runtime_refs: Vec<RuntimeRef>,
        graph_request: Option<GraphRequest>,
    ) -> super::runtime_frame::FrameRequest<'a> {
        super::runtime_frame::FrameRequest {
            run_id: RUN_ID,
            project_scope: &self.project,
            access,
            runtime_refs,
            graph_request,
            max_bytes: 8_192,
            ttl_ms: TTL_MS,
        }
    }

    pub(crate) fn coding_ref(&self, id: &str) -> RuntimeRef {
        RuntimeRef {
            kind: saaa_personal_state_core::world::runtime_frame::RuntimeKind::CodingJob,
            id: id.to_string(),
        }
    }

    pub(crate) fn add_coding_job(
        &self,
        revision: u64,
        state: &str,
        run_state: &str,
        delivery: &str,
    ) {
        self.add_coding_job_with_result(revision, state, run_state, delivery, None);
    }

    pub(crate) fn add_coding_job_with_result(
        &self,
        revision: u64,
        state: &str,
        run_state: &str,
        delivery: &str,
        result_json: Option<&str>,
    ) {
        self.writer
            .write(|c| {
                // Distinct from the run input so forgetting the coding source
                // does not deny the whole run scope. Finalize and complete the
                // job so the World projection is not left in pending review.
                insert_source(c, PROJECT, CODING_SOURCE, "coding source");
                c.execute(
                    "UPDATE personal_jobs SET status='completed' WHERE source_sequence=(
                       SELECT sequence FROM personal_sources WHERE message_id=?1)",
                    [CODING_SOURCE],
                )
                .map_err(crate::database_error)?;
                c.execute(
                    "INSERT INTO coding_workspaces(id,conversation_id,path)
                     VALUES('w1',?1,'/tmp/w')",
                    [crate::PRIMARY_CONVERSATION_ID],
                )
                .map_err(crate::database_error)?;
                c.execute(
                    "INSERT INTO coding_jobs(
                       id,conversation_id,source_id,workspace_id,workspace_path,settings_json,
                       revision,session_path,session_id,state,current_run_id)
                     VALUES(?1,?2,?3,'w1','/tmp/w','{}',?4,'/tmp/session',NULL,?5,?6)",
                    params![
                        CODING_ID,
                        crate::PRIMARY_CONVERSATION_ID,
                        CODING_SOURCE,
                        revision,
                        state,
                        "run_c1"
                    ],
                )
                .map_err(crate::database_error)?;
                c.execute(
                    "INSERT INTO coding_runs(
                       id,job_id,source_id,host_run_id,payload,digest,delivery,state,
                       result_json,started_at,ended_at)
                     VALUES('run_c1',?1,?2,'host','{}','digest',?3,?4,?5,'1000',NULL)",
                    params![CODING_ID, CODING_SOURCE, delivery, run_state, result_json],
                )
                .map_err(crate::database_error)?;
                crate::memory::personal_state::store::rebuild(
                    c,
                    crate::memory::personal_state::now(),
                )?;
                Ok(())
            })
            .expect("coding job");
    }

    pub(crate) fn set_coding_state(
        &self,
        revision: u64,
        state: &str,
        run_state: &str,
        delivery: &str,
    ) {
        self.writer
            .write(|c| {
                c.execute(
                    "UPDATE coding_jobs SET state=?1,revision=?2 WHERE id=?3",
                    params![state, revision, CODING_ID],
                )
                .map_err(crate::database_error)?;
                c.execute(
                    "UPDATE coding_runs SET state=?1,delivery=?2 WHERE id='run_c1'",
                    params![run_state, delivery],
                )
                .map_err(crate::database_error)?;
                Ok(())
            })
            .expect("coding state");
    }

    pub(crate) fn table_count(&self, table: &str) -> i64 {
        let sql = format!("SELECT COUNT(*) FROM {table}");
        self.writer
            .read_serialized(|c| {
                c.query_row(&sql, [], |r| r.get(0))
                    .map_err(crate::database_error)
            })
            .expect("count")
    }

    pub(crate) fn table_names(&self) -> Vec<String> {
        self.writer
            .read_serialized(|c| {
                let mut statement = c
                    .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
                    .map_err(crate::database_error)?;
                let names = statement
                    .query_map([], |row| row.get::<_, String>(0))
                    .map_err(crate::database_error)?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(crate::database_error)?;
                Ok(names)
            })
            .expect("table names")
    }

    pub(crate) fn ledger_count(&self) -> i64 {
        [
            "personal_source_refs",
            "personal_tombstones",
            "personal_assertions",
            "personal_transitions",
            "personal_coverage",
            "personal_patches",
        ]
        .iter()
        .map(|table| self.table_count(table))
        .sum()
    }

    pub(crate) fn fill_coverage(&self, count: usize) {
        self.writer
            .write(|c| {
                let prefix = crate::new_id("cov");
                for index in 0..count {
                    let key = saaa_personal_state_core::SourceKey {
                        id: format!("{prefix}_{index}"),
                        version: 1,
                        start: 0,
                        end: 1,
                    };
                    c.execute(
                        "INSERT OR REPLACE INTO personal_coverage VALUES(?1,?2)",
                        params![
                            crate::memory::personal_state::encode(&key)?,
                            crate::memory::personal_state::encode(
                                &saaa_personal_state_core::Coverage::Applied
                            )?
                        ],
                    )
                    .map_err(crate::database_error)?;
                }
                Ok(())
            })
            .expect("coverage fill");
    }

    pub(crate) fn total_changes(&self) -> i64 {
        self.writer
            .read_serialized(|c| {
                c.query_row("SELECT total_changes()", [], |r| r.get(0))
                    .map_err(crate::database_error)
            })
            .expect("changes")
    }

    pub(crate) fn graph_request(&self, seed: &str) -> GraphRequest {
        GraphRequest {
            seeds: vec![super::query::WorldSeed::EntityId(seed.to_string())],
            causal_direction: CausalDirection::Forward,
            limits: LimitsV2::m1(),
            flags: super::query_v2::IncludeFlags::default(),
            explicit_question: true,
        }
    }
}
