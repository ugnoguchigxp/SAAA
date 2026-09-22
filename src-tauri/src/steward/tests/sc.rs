use super::{commands, invalidation, queue, repository as repo};
use crate::persistence::schema::initialize_database;
use crate::test_support::app_state;
use crate::{now_iso, PRIMARY_CONVERSATION_ID};
use rusqlite::{params, Connection};
use std::time::Duration;

fn db() -> Connection {
    let connection = Connection::open_in_memory().expect("db");
    initialize_database(&connection).expect("init");
    connection
}

fn goal(connection: &Connection, id: &str, summary: &str, budget_ms: i64) {
    connection
        .execute(
            "INSERT INTO steward_goals(id,conversation_id,origin,success_condition,status,created_at,summary)
             VALUES(?1,?2,'user_explicit','done','active',?3,?4)",
            params![id, PRIMARY_CONVERSATION_ID, now_iso(), summary],
        )
        .expect("goal");
    connection
        .execute(
            "INSERT INTO steward_delegations(id,goal_id,conversation_id,workspace_id,ops,budget_runs,budget_ms,notify,status,created_at)
             VALUES(?1,?2,?3,'ws','read',3,?4,'silent','active',?5)",
            params![
                format!("d-{id}"),
                id,
                PRIMARY_CONVERSATION_ID,
                budget_ms,
                now_iso()
            ],
        )
        .expect("delegation");
}

fn work(goal_id: &str, budget_ms: i64) -> repo::ActiveWork {
    repo::ActiveWork {
        goal_id: goal_id.into(),
        goal_status: "active".into(),
        delegation_id: format!("d-{goal_id}"),
        workspace_id: "ws".into(),
        budget_runs: 3,
        budget_ms,
        superseded: false,
        ops: "read".into(),
        verifier: "test_report_obtained".into(),
    }
}

struct ClearFault;
impl Drop for ClearFault {
    fn drop(&mut self) {
        super::faults::set(None);
    }
}

#[test]
fn sc_01_flush_rolls_back_message_when_mark_flushed_fails() {
    let _clear = ClearFault;
    let directory = tempfile::tempdir().expect("dir");
    let path = directory.path().join("sc01.sqlite3");
    let connection = Connection::open(&path).expect("open");
    initialize_database(&connection).expect("init");
    repo::enqueue_report(&connection, PRIMARY_CONVERSATION_ID, "sc01-digest", None, 0)
        .expect("report");
    let state = app_state(connection);
    super::faults::set(Some("mark_flushed"));
    let failed = super::flush_held_reports(&state, PRIMARY_CONVERSATION_ID);
    super::faults::set(None);
    assert!(failed.is_err(), "{failed:?}");
    let reopened = Connection::open(&path).expect("reopen");
    let messages: i64 = reopened
        .query_row("SELECT COUNT(*) FROM conversation_messages", [], |row| {
            row.get(0)
        })
        .expect("messages");
    let unflushed: i64 = reopened
        .query_row(
            "SELECT COUNT(*) FROM steward_reports WHERE flushed=0",
            [],
            |row| row.get(0),
        )
        .expect("unflushed");
    assert_eq!(messages, 0);
    assert_eq!(unflushed, 1);
    super::flush_held_reports(&state, PRIMARY_CONVERSATION_ID).expect("retry");
    let reopened = Connection::open(&path).expect("reopen after retry");
    let messages: i64 = reopened
        .query_row(
            "SELECT COUNT(*) FROM conversation_messages WHERE role='assistant'",
            [],
            |row| row.get(0),
        )
        .expect("messages");
    let delivered: i64 = reopened
        .query_row(
            "SELECT COUNT(*) FROM steward_reports WHERE flushed=1 AND message_id IS NOT NULL",
            [],
            |row| row.get(0),
        )
        .expect("delivered");
    let stranded: i64 = reopened
        .query_row(
            "SELECT COUNT(*) FROM conversation_messages m
             WHERE m.role='assistant' AND NOT EXISTS(
               SELECT 1 FROM steward_reports r WHERE r.flushed=1 AND r.message_id=m.id
             )",
            [],
            |row| row.get(0),
        )
        .expect("stranded");
    assert_eq!(messages, 1);
    assert_eq!(delivered, 1);
    assert_eq!(stranded, 0);
}

#[test]
fn sc_02_remaining_deadline_uses_elapsed_run_time() {
    let connection = db();
    goal(&connection, "g-elapsed", "elapsed", 5_000);
    connection
        .execute(
            "INSERT INTO steward_tasks(id,delegation_id,conversation_id,trigger_kind,source_id,dedupe_key,loop_state,created_at,updated_at)
             VALUES('t-elapsed','d-g-elapsed',?1,'start','src','k-elapsed','running',?2,?2)",
            params![PRIMARY_CONVERSATION_ID, now_iso()],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO coding_jobs(id,conversation_id,source_id,workspace_id,workspace_path,settings_json,revision,session_path,state,current_run_id)
             VALUES('job-elapsed',?1,'src-elapsed','ws','/tmp','{}',1,'/tmp/sc-elapsed','running','run-elapsed')",
            [PRIMARY_CONVERSATION_ID],
        )
        .unwrap();
    connection
        .execute(
            "UPDATE steward_tasks SET coding_job_id='job-elapsed' WHERE id='t-elapsed'",
            [],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO coding_runs(id,job_id,source_id,host_run_id,payload,digest,delivery,state,started_at,ended_at)
             VALUES('run-elapsed','job-elapsed','src-elapsed','host','payload','digest','accepted','settled','1000','3500')",
            [],
        )
        .unwrap();
    let closed = super::budget::remaining_deadline_ms(&connection, &work("g-elapsed", 5_000))
        .expect("closed");
    assert_eq!(closed, 2_500);

    goal(&connection, "g-open", "open", 5_000);
    connection
        .execute(
            "INSERT INTO steward_tasks(id,delegation_id,conversation_id,trigger_kind,source_id,dedupe_key,loop_state,created_at,updated_at,coding_job_id)
             VALUES('t-open','d-g-open',?1,'start','src-open','k-open','running',?2,?2,'job-open')",
            params![PRIMARY_CONVERSATION_ID, now_iso()],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO coding_jobs(id,conversation_id,source_id,workspace_id,workspace_path,settings_json,revision,session_path,state,current_run_id)
             VALUES('job-open',?1,'src-open-job','ws','/tmp','{}',1,'/tmp/sc-open','queued','run-open')",
            [PRIMARY_CONVERSATION_ID],
        )
        .unwrap();
    let started = now_iso().parse::<i64>().unwrap() - 1_500;
    connection
        .execute(
            "INSERT INTO coding_runs(id,job_id,source_id,host_run_id,payload,digest,delivery,state,started_at)
             VALUES('run-open','job-open','src-open','host','payload','digest','accepted','failed',?1)",
            [started.to_string()],
        )
        .unwrap();
    let open =
        super::budget::remaining_deadline_ms(&connection, &work("g-open", 5_000)).expect("open");
    assert!(open < 5_000, "elapsed was not subtracted: {open}");
    assert!(open <= 4_000, "open run consumed too little: {open}");
    assert!(open >= 2_000, "open run consumed too much: {open}");
}

#[test]
fn sc_02_short_budget_stops_before_fixed_1800_seconds() {
    let connection = db();
    goal(&connection, "g-short", "short", 80);
    connection
        .execute(
            "INSERT INTO steward_tasks(id,delegation_id,conversation_id,trigger_kind,source_id,dedupe_key,loop_state,created_at,updated_at)
             VALUES('t-short','d-g-short',?1,'start','src','k-short','running',?2,?2)",
            params![PRIMARY_CONVERSATION_ID, now_iso()],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO coding_jobs(id,conversation_id,source_id,workspace_id,workspace_path,settings_json,revision,session_path,state,current_run_id)
             VALUES('job-short',?1,'src-short','ws','/tmp','{}',1,'/tmp/sc-short','queued','run-short')",
            [PRIMARY_CONVERSATION_ID],
        )
        .unwrap();
    connection
        .execute(
            "UPDATE steward_tasks SET coding_job_id='job-short' WHERE id='t-short'",
            [],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO coding_runs(id,job_id,source_id,host_run_id,payload,digest,delivery,state,started_at)
             VALUES('run-short','job-short','src-short','host','payload','digest','prepared','settled',?1)",
            [now_iso()],
        )
        .unwrap();
    let remaining =
        super::budget::remaining_deadline_ms(&connection, &work("g-short", 80)).expect("remaining");
    assert!(
        (1..=80).contains(&remaining),
        "short budget was replaced: {remaining}"
    );
    assert!(crate::runtime::pi::runner::wait_budget(Some(remaining)) < Duration::from_secs(2));
    assert_eq!(
        crate::runtime::pi::runner::wait_budget(None),
        Duration::from_secs(1800)
    );
    let elapsed = crate::runtime::pi::runner::sleep_until_budget(remaining);
    assert!(elapsed < Duration::from_secs(2), "{elapsed:?}");
}

#[test]
fn sc_03_runner_request_contains_goal_summary() {
    let connection = db();
    let marker = "SC03-MARKER-摘要";
    goal(&connection, "g-req", marker, 1_000);
    connection
        .execute(
            "INSERT INTO steward_goal_plans(id,goal_id,revision,max_replans,created_at)
             VALUES('plan-req','g-req',1,1,?1)",
            [now_iso()],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO steward_plan_steps(plan_id,step_id,ordinal,recipe,verifier,depends_on_json)
             VALUES('plan-req','read',0,'read','test_report_obtained','[]')",
            [],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO steward_tasks(id,delegation_id,conversation_id,trigger_kind,source_id,dedupe_key,loop_state,created_at,updated_at,goal_plan_id,plan_step_id)
             VALUES('t-req','d-g-req',?1,'start','src','k-req','queued',?2,?2,'plan-req','read')",
            params![PRIMARY_CONVERSATION_ID, now_iso()],
        )
        .unwrap();
    let request =
        queue::request_for_task(&connection, &work("g-req", 1_000), "t-req").expect("request");
    assert!(request.contains(marker), "{request}");
    assert!(request.contains("Recipe: read"), "{request}");
    assert!(
        request.contains("Do not run tests or change files."),
        "{request}"
    );
    assert_ne!(
        request,
        "Inspect the existing failure evidence in this workspace and report causes. Do not run tests or change files."
    );
}

#[test]
fn sc_04_reorder_changes_next_eligible_task() {
    let connection = db();
    connection
        .execute_batch(&format!(
            "INSERT INTO steward_goals(id,conversation_id,origin,success_condition,status,created_at,summary) VALUES
               ('g1','{PRIMARY_CONVERSATION_ID}','user_explicit','one','active','1','one'),
               ('g2','{PRIMARY_CONVERSATION_ID}','user_explicit','two','active','2','two');
             INSERT INTO steward_delegations(id,goal_id,conversation_id,workspace_id,ops,budget_runs,budget_ms,notify,status,created_at) VALUES
               ('d1','g1','{PRIMARY_CONVERSATION_ID}','ws','read',2,1000,'silent','active','1'),
               ('d2','g2','{PRIMARY_CONVERSATION_ID}','ws','read',2,1000,'silent','active','2');
             INSERT INTO steward_tasks(id,delegation_id,conversation_id,trigger_kind,source_id,dedupe_key,loop_state,created_at,updated_at,queue_rank) VALUES
               ('t1','d1','{PRIMARY_CONVERSATION_ID}','start','s1','k1','queued','1','1',1),
               ('t2','d2','{PRIMARY_CONVERSATION_ID}','start','s2','k2','queued','2','2',2);
             INSERT INTO steward_dispatch_intents(id,task_id,state,idempotency_key,created_at,updated_at) VALUES
               ('i1','t1','pending','k1','1','1'),
               ('i2','t2','pending','k2','2','2');"
        ))
        .unwrap();
    let first = queue::next_eligible(&connection, Some(PRIMARY_CONVERSATION_ID))
        .unwrap()
        .map(|(_, task, _)| task);
    assert_eq!(first.as_deref(), Some("t1"));
    repo::reorder_queue(
        &connection,
        PRIMARY_CONVERSATION_ID,
        &["t2".into(), "t1".into()],
    )
    .unwrap();
    let next = queue::next_eligible(&connection, Some(PRIMARY_CONVERSATION_ID))
        .unwrap()
        .map(|(_, task, _)| task);
    assert_eq!(next.as_deref(), Some("t2"));
}

#[test]
fn sc_05_forget_keeps_sibling_goal_report() {
    let connection = db();
    connection
        .execute(
            "INSERT INTO conversation_messages(id,conversation_id,role,content,created_at) VALUES
             ('src-a',?1,'user','forgotten-body','1'),
             ('src-b',?1,'user','sibling-body','1')",
            [PRIMARY_CONVERSATION_ID],
        )
        .unwrap();
    connection
        .execute_batch(&format!(
            "INSERT INTO steward_goals(id,conversation_id,origin,success_condition,status,created_at,summary) VALUES
               ('ga','{PRIMARY_CONVERSATION_ID}','user_explicit','a','active','1','A'),
               ('gb','{PRIMARY_CONVERSATION_ID}','user_explicit','b','active','1','B');
             INSERT INTO steward_delegations(id,goal_id,conversation_id,workspace_id,ops,budget_runs,budget_ms,notify,status,created_at) VALUES
               ('da','ga','{PRIMARY_CONVERSATION_ID}','ws','read',1,1000,'silent','active','1'),
               ('db','gb','{PRIMARY_CONVERSATION_ID}','ws','read',1,1000,'silent','active','1');
             INSERT INTO steward_tasks(id,delegation_id,conversation_id,trigger_kind,source_id,dedupe_key,loop_state,created_at,updated_at) VALUES
               ('ta','da','{PRIMARY_CONVERSATION_ID}','start','src-a','ka','done','1','1'),
               ('tb','db','{PRIMARY_CONVERSATION_ID}','start','src-b','kb','done','1','1');
             INSERT INTO steward_reports(id,conversation_id,digest,flushed,created_at,task_id,message_id) VALUES
               ('ra','{PRIMARY_CONVERSATION_ID}','drop-digest',0,'1','ta','src-a'),
               ('rb','{PRIMARY_CONVERSATION_ID}','keep-digest',0,'1','tb','src-b');"
        ))
        .unwrap();
    invalidation::forget_source(&connection, "src-a").expect("forget");
    let forgotten: (i64, String) = connection
        .query_row(
            "SELECT invalidated,digest FROM steward_reports WHERE id='ra'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    let kept: (i64, String) = connection
        .query_row(
            "SELECT invalidated,digest FROM steward_reports WHERE id='rb'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    let forgotten_body: String = connection
        .query_row(
            "SELECT content FROM conversation_messages WHERE id='src-a'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let sibling_body: String = connection
        .query_row(
            "SELECT content FROM conversation_messages WHERE id='src-b'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(forgotten, (1, String::new()));
    assert_eq!(kept, (0, "keep-digest".into()));
    assert_eq!(forgotten_body, "");
    assert_eq!(sibling_body, "sibling-body");
}

#[test]
fn sc_06_withdraw_one_goal_leaves_the_other_running() {
    let connection = db();
    connection
        .execute(
            "INSERT INTO conversation_messages(id,conversation_id,role,content,created_at) VALUES
             ('src-a',?1,'user','a','1'),('src-b',?1,'user','b','1')",
            [PRIMARY_CONVERSATION_ID],
        )
        .unwrap();
    connection
        .execute_batch(&format!(
            "INSERT INTO steward_goals(id,conversation_id,origin,success_condition,status,created_at,summary) VALUES
               ('ga','{PRIMARY_CONVERSATION_ID}','user_explicit','a','active','1','A'),
               ('gb','{PRIMARY_CONVERSATION_ID}','user_explicit','b','active','1','B');
             INSERT INTO steward_delegations(id,goal_id,conversation_id,workspace_id,ops,budget_runs,budget_ms,notify,status,created_at) VALUES
               ('da','ga','{PRIMARY_CONVERSATION_ID}','ws','read',2,1000,'silent','active','1'),
               ('db','gb','{PRIMARY_CONVERSATION_ID}','ws','read',2,1000,'silent','active','1');
             INSERT INTO steward_tasks(id,delegation_id,conversation_id,trigger_kind,source_id,dedupe_key,loop_state,created_at,updated_at,coding_job_id) VALUES
               ('ta','da','{PRIMARY_CONVERSATION_ID}','start','src-a','ka','running','1','1','job-a'),
               ('tb','db','{PRIMARY_CONVERSATION_ID}','start','src-b','kb','running','1','1','job-b');
             INSERT INTO coding_jobs(id,conversation_id,source_id,workspace_id,workspace_path,settings_json,revision,session_path,state,current_run_id) VALUES
               ('job-a','{PRIMARY_CONVERSATION_ID}','delegated-ta','ws','/tmp','{{}}',1,'/tmp/sc-a','running','run-a'),
               ('job-b','{PRIMARY_CONVERSATION_ID}','delegated-tb','ws','/tmp','{{}}',1,'/tmp/sc-b','running','run-b');
             INSERT INTO coding_runs(id,job_id,source_id,host_run_id,payload,digest,delivery,state,started_at) VALUES
               ('run-a','job-a','src-a','host','a','d','accepted','running','1'),
               ('run-b','job-b','src-b','host','b','d','accepted','failed','1');
             INSERT INTO coding_origin_bindings(id,job_id,origin_kind,origin_id,operation_digest,created_at) VALUES
               ('ob-a','job-a','delegated_event','ta','d','1'),
               ('ob-b','job-b','delegated_event','tb','d','1');"
        ))
        .unwrap();
    assert_eq!(
        commands::unspecified_goal_withdraw().unwrap_err(),
        "goal_required"
    );
    commands::withdraw_specified(&connection, PRIMARY_CONVERSATION_ID, "ga").expect("withdraw a");
    let task_a: String = connection
        .query_row(
            "SELECT loop_state FROM steward_tasks WHERE id='ta'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let task_b: String = connection
        .query_row(
            "SELECT loop_state FROM steward_tasks WHERE id='tb'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let job_a: String = connection
        .query_row(
            "SELECT state FROM coding_jobs WHERE id='job-a'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let job_b: String = connection
        .query_row(
            "SELECT state FROM coding_jobs WHERE id='job-b'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let goal_b: String = connection
        .query_row(
            "SELECT status FROM steward_goals WHERE id='gb'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(task_a, "cancelled");
    assert_eq!(job_a, "cancel_requested");
    assert_eq!(task_b, "running");
    assert_eq!(job_b, "running");
    assert_eq!(goal_b, "active");
}
