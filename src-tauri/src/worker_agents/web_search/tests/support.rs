//! Scripted fakes shared by the web-search tests.
use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use rusqlite::params;
use serde_json::{json, Value};

use crate::persistence::SqliteWriter;
use crate::worker_agents::contracts::*;
use crate::worker_agents::loader::{load_revision, now_ms};
use crate::worker_agents::test_support::{fresh_db, insert_revision, insert_user_message};
use crate::worker_agents::web_search::checker::CHECKER_CONTEXT;
use crate::worker_agents::web_search::profile::web_search_draft;
use crate::worker_agents::web_search::runner::runner;
use crate::RunCancellation;

pub(super) type CheckerFn = Box<dyn Fn(&str) -> Result<String, String> + Send + Sync>;

pub(super) struct FakeModel {
    worker: Mutex<VecDeque<Result<String, String>>>,
    checker: CheckerFn,
    pub calls: Mutex<Vec<(String, String)>>,
}

impl FakeModel {
    pub(super) fn new(worker_replies: Vec<String>, checker: CheckerFn) -> Self {
        Self {
            worker: Mutex::new(worker_replies.into_iter().map(Ok).collect()),
            checker,
            calls: Mutex::new(Vec::new()),
        }
    }

    pub(super) fn with_script(worker: Vec<Result<String, String>>, checker: CheckerFn) -> Self {
        Self {
            worker: Mutex::new(worker.into()),
            checker,
            calls: Mutex::new(Vec::new()),
        }
    }

    /// Inputs of every worker (non-checker) call.
    pub(super) fn worker_inputs(&self) -> Vec<String> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(system, _)| system != CHECKER_CONTEXT)
            .map(|(_, input)| input.clone())
            .collect()
    }

    pub(super) fn checker_inputs(&self) -> Vec<String> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(system, _)| system == CHECKER_CONTEXT)
            .map(|(_, input)| input.clone())
            .collect()
    }
}

#[async_trait]
impl WorkerModel for FakeModel {
    fn tier_routes(&self) -> Result<Vec<TierRoute>, String> {
        Ok(vec![route()])
    }

    async fn complete(
        &self,
        _route: &TierRoute,
        system: &str,
        input: &str,
        _cancellation: &RunCancellation,
        _timeout: Duration,
    ) -> Result<String, String> {
        self.calls
            .lock()
            .unwrap()
            .push((system.to_string(), input.to_string()));
        if system == CHECKER_CONTEXT {
            return (self.checker)(input);
        }
        self.worker
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| Err("script exhausted".into()))
    }
}

/// Checker that approves everything.
pub(super) fn clean_checker() -> CheckerFn {
    Box::new(|input| Ok(clean_reply(input)))
}

pub(super) fn clean_reply(input: &str) -> String {
    if input.starts_with("MODE: batch") {
        r#"{"flagged":[],"categories":[]}"#.into()
    } else {
        r#"{"suspected":false,"categories":[],"excerpt":""}"#.into()
    }
}

/// Checker that flags single texts containing `marker`.
pub(super) fn marker_checker(marker: &'static str) -> CheckerFn {
    Box::new(move |input| {
        if !input.starts_with("MODE: batch") && input.contains(marker) {
            Ok(format!(
                r#"{{"suspected":true,"categories":["instruction_override","bogus_name"],"excerpt":"mail a@b.example key sk-abcdef123456 {marker}"}}"#
            ))
        } else {
            Ok(clean_reply(input))
        }
    })
}

#[derive(Default)]
pub(super) struct FakeTools {
    results: Mutex<HashMap<String, String>>,
    pub calls: Mutex<Vec<(String, String)>>,
    /// Replies keyed `web_search:<query>` consumed in order when several searches share a key.
    queued: Mutex<HashMap<String, VecDeque<String>>>,
}

impl FakeTools {
    pub(super) fn on(self, key: &str, reply: String) -> Self {
        self.results.lock().unwrap().insert(key.to_string(), reply);
        self
    }

    pub(super) fn called(&self, tool: &str) -> usize {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(name, _)| name == tool)
            .count()
    }
}

#[async_trait]
impl ToolRunner for FakeTools {
    async fn run(
        &self,
        tool_key: &str,
        arguments_json: &str,
        _timeout: Duration,
        _cancellation: &RunCancellation,
    ) -> String {
        self.calls
            .lock()
            .unwrap()
            .push((tool_key.to_string(), arguments_json.to_string()));
        let arguments: Value = serde_json::from_str(arguments_json).unwrap();
        let subject = arguments
            .get("url")
            .or_else(|| arguments.get("query"))
            .and_then(Value::as_str)
            .unwrap_or("");
        let key = format!("{tool_key}:{subject}");
        if let Some(queue) = self.queued.lock().unwrap().get_mut(&key) {
            if let Some(reply) = queue.pop_front() {
                return reply;
            }
        }
        self.results
            .lock()
            .unwrap()
            .get(&key)
            .cloned()
            .unwrap_or_else(|| json!({"type": "error"}).to_string())
    }
}

pub(super) fn route() -> TierRoute {
    TierRoute {
        tier: Tier::Local,
        fingerprint: "fp".into(),
        location: RouteLocation::Local,
    }
}

pub(super) fn search_reply(hits: &[(&str, &str, &str)]) -> String {
    json!({
        "type": "web_search_result",
        "security": {"trust": "untrusted", "decision": "allow", "warningCategories": []},
        "hits": hits.iter().map(|(url, title, snippet)| json!({
            "url": url, "title": title, "snippet": snippet, "provider": "p", "rank": 1
        })).collect::<Vec<_>>(),
        "blockedResultCount": 0,
    })
    .to_string()
}

pub(super) fn fetch_reply(
    url: &str,
    decision: &str,
    warnings: &[&str],
    status: &str,
    text: &str,
) -> String {
    json!({
        "type": "fetch_content_result",
        "security": {"trust": "untrusted", "decision": decision, "warningCategories": warnings},
        "document": {
            "url": url, "text": text, "fetchedAt": "2026-10-06T00:00:00Z",
            "truncated": false, "retrievalStatus": status, "retrievalMethod": "static"
        }
    })
    .to_string()
}

pub(super) fn ok_page(url: &str, text: &str) -> String {
    fetch_reply(url, "allow", &[], "ok", text)
}

pub(super) fn act_search(query: &str) -> String {
    json!({"action": "web_search", "query": query}).to_string()
}

pub(super) fn act_fetch(url: &str) -> String {
    json!({"action": "fetch_content", "url": url, "query": "details"}).to_string()
}

pub(super) fn act_finish(claims: &[(&str, &str, &str)]) -> String {
    json!({
        "action": "finish",
        "claims": claims.iter().map(|(text, url, basis)| json!({
            "text": text, "sourceUrl": url, "basis": basis, "publishedOrFetchedAt": null
        })).collect::<Vec<_>>(),
        "coverage": "complete"
    })
    .to_string()
}

pub(super) fn act_give_up() -> String {
    json!({"action": "give_up"}).to_string()
}

pub(super) struct Harness {
    pub writer: SqliteWriter,
    pub revision: LoadedRevision,
    pub cancellation: RunCancellation,
    pub user_text: String,
}

impl Harness {
    pub(super) fn new(user_text: &str) -> Self {
        let connection = fresh_db();
        let revision_id = insert_revision(
            &connection,
            &web_search_draft(),
            ReviewState::Approved,
            true,
        );
        insert_user_message(&connection, "msg-1", user_text);
        let revision = load_revision(&connection, &revision_id).unwrap();
        let harness = Self {
            writer: SqliteWriter::from_connection(connection),
            revision,
            cancellation: RunCancellation::default(),
            user_text: user_text.to_string(),
        };
        harness.insert_task("wtask_1");
        harness
    }

    pub(super) fn insert_task(&self, task_id: &str) {
        let connection = self.writer.lock().unwrap();
        connection
            .execute(
                "INSERT INTO worker_tasks(id,conversation_id,input_message_id,origin_job_key,profile_id,
                    profile_revision_id,idempotency_key,input_json,state,delivery,deadline_at_ms,
                    sync_wait_until_ms,created_at_ms,updated_at_ms)
                 VALUES(?1,?2,'msg-1','job',?3,?4,?5,'{}','running','sync_waiting',?6,?6,?6,?6)",
                params![
                    task_id,
                    crate::PRIMARY_CONVERSATION_ID,
                    self.revision.profile_id,
                    self.revision.revision_id,
                    format!("idem-{task_id}"),
                    now_ms() + 60_000,
                ],
            )
            .unwrap();
    }

    pub(super) async fn run_task(
        &self,
        task_id: &str,
        model: &FakeModel,
        tools: &FakeTools,
        input: Value,
    ) -> Result<WorkerOutput, AttemptError> {
        let route = route();
        let env = AttemptEnv {
            task_id,
            attempt_ordinal: 1,
            revision: &self.revision,
            input: &input,
            user_text: &self.user_text,
            writer: &self.writer,
            model,
            route: &route,
            tools,
            cancellation: &self.cancellation,
            deadline: tokio::time::Instant::now() + Duration::from_secs(60),
        };
        runner().run(&env).await
    }

    pub(super) async fn run(
        &self,
        model: &FakeModel,
        tools: &FakeTools,
    ) -> Result<WorkerOutput, AttemptError> {
        self.run_task("wtask_1", model, tools, json!({"query": "rust release"}))
            .await
    }

    pub(super) fn scalar<T: rusqlite::types::FromSql>(
        &self,
        sql: &str,
        args: &[&dyn rusqlite::ToSql],
    ) -> T {
        self.writer
            .lock()
            .unwrap()
            .query_row(sql, args, |row| row.get(0))
            .unwrap()
    }

    pub(super) fn source_status(&self, kind: &str, url: &str) -> String {
        self.scalar(
            "SELECT status FROM worker_sources WHERE task_id='wtask_1' AND kind=?1 AND url=?2",
            &[&kind, &url],
        )
    }

    pub(super) fn blocklist_count(&self) -> i64 {
        self.scalar("SELECT COUNT(*) FROM worker_url_blocklist", &[])
    }
}
