use crate::database_error;
use rusqlite::{params, Connection};
use serde_json::{json, Value};

pub fn migrate(c: &Connection) -> rusqlite::Result<()> {
    c.execute_batch("CREATE TABLE IF NOT EXISTS terminal_runs(
        run_id TEXT PRIMARY KEY REFERENCES coding_runs(id), directory TEXT NOT NULL UNIQUE,
        nonce TEXT NOT NULL, phase TEXT NOT NULL, offset INTEGER NOT NULL DEFAULT 0,
        cli TEXT NOT NULL, terminal TEXT NOT NULL, baseline_json TEXT NOT NULL,
        checks_json TEXT NOT NULL, runner_pid INTEGER, runner_identity TEXT,
        exit_json TEXT, result_json TEXT, candidate_json TEXT, created_at TEXT NOT NULL);
      CREATE TABLE IF NOT EXISTS terminal_inbox(id TEXT PRIMARY KEY, run_id TEXT NOT NULL REFERENCES terminal_runs(run_id), kind TEXT NOT NULL);
      CREATE TABLE IF NOT EXISTS terminal_questions(id TEXT PRIMARY KEY, run_id TEXT NOT NULL REFERENCES terminal_runs(run_id), job_id TEXT NOT NULL REFERENCES coding_jobs(id), kind TEXT NOT NULL, input_json TEXT NOT NULL, state TEXT NOT NULL, answer_json TEXT, decision_id TEXT UNIQUE, source_message_id TEXT, created_at TEXT NOT NULL);
      CREATE TABLE IF NOT EXISTS terminal_resumes(source_id TEXT PRIMARY KEY, job_id TEXT NOT NULL REFERENCES coding_jobs(id), question_id TEXT NOT NULL REFERENCES terminal_questions(id), decision_id TEXT NOT NULL UNIQUE, source_message_id TEXT);
      CREATE TABLE IF NOT EXISTS terminal_decisions(question_id TEXT PRIMARY KEY REFERENCES terminal_questions(id), job_id TEXT NOT NULL REFERENCES coding_jobs(id), status TEXT NOT NULL, source_id TEXT, quote TEXT, answer_json TEXT);
      CREATE INDEX IF NOT EXISTS terminal_question_job ON terminal_questions(job_id,state);")
}
pub fn context(c: &Connection, job: &str) -> Result<Value, String> {
    let run = c.query_row("SELECT t.run_id,t.phase,t.cli,t.terminal,t.result_json FROM terminal_runs t JOIN coding_jobs j ON j.current_run_id=t.run_id WHERE j.id=?1", [job], |r| Ok(json!({"runId":r.get::<_,String>(0)?,"phase":r.get::<_,String>(1)?,"cli":r.get::<_,String>(2)?,"terminal":r.get::<_,String>(3)?,"verification":r.get::<_,Option<String>>(4)?.and_then(|v|serde_json::from_str::<Value>(&v).ok())}))).unwrap_or(Value::Null);
    if run.is_null() {
        return Ok(run);
    }
    let mut value = run;
    value["verification"] = public_result(&value["verification"]);
    let mut stmt = c.prepare("SELECT id,kind,input_json,state FROM terminal_questions WHERE job_id=?1 AND state IN ('pending','awaiting_user') ORDER BY rowid LIMIT 8").map_err(database_error)?;
    let rows = stmt.query_map([job], |r| Ok(json!({"questionId":r.get::<_,String>(0)?,"kind":r.get::<_,String>(1)?,"input":serde_json::from_str::<Value>(&r.get::<_,String>(2)?).unwrap_or(Value::Null),"state":r.get::<_,String>(3)?}))).map_err(database_error)?;
    value["questions"] = json!(rows
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error)?);
    Ok(value)
}
pub fn report(c: &Connection, conversation: &str, body: &str) -> Result<(), String> {
    let id = crate::steward::enqueue_terminal_report(
        c,
        conversation,
        body,
        None,
        crate::task_queue::now_ms(),
    )?;
    c.execute("UPDATE steward_reports SET speak_requested=1,speech_state='pending' WHERE id=?1 AND flushed=0", [&id]).map_err(database_error)?;
    Ok(())
}
pub fn source_authorized(c: &Connection, source: &str, job: &str) -> Result<bool, String> {
    c.query_row("SELECT EXISTS(SELECT 1 FROM terminal_resumes r JOIN terminal_questions q ON q.id=r.question_id JOIN coding_jobs j ON j.id=r.job_id JOIN conversation_messages original ON original.id=j.source_id WHERE r.source_id=?1 AND r.job_id=?2 AND q.job_id=j.id AND q.state='answered' AND q.decision_id=r.decision_id AND original.conversation_id=j.conversation_id AND (r.source_message_id IS NULL OR EXISTS(SELECT 1 FROM conversation_messages m WHERE m.id=r.source_message_id AND m.conversation_id=j.conversation_id)))",params![source,job],|r|r.get(0)).map_err(database_error)
}

pub fn public_result(result: &Value) -> Value {
    let checks=result["verification"]["checks"].as_array().into_iter().flatten().take(8).map(|check|json!({"argv":check["argv"],"exitCode":check["exitCode"],"timeout":check["timeout"],"output":check["output"].as_str().unwrap_or_default().chars().take(800).collect::<String>(),"error":check["error"].as_str().unwrap_or_default().chars().take(800).collect::<String>(),"outputSha256":check["outputSha256"],"truncated":check["truncated"],"streamIncomplete":check["streamIncomplete"]})).collect::<Vec<_>>();
    json!({"summary":result["summary"],"complete":result["complete"],"candidate":{"summary":result["candidate"]["summary"].as_str().map(|s|s.chars().take(2000).collect::<String>()),"remainingManualChecks":result["candidate"]["remainingManualChecks"].as_array().map(|a|a.iter().take(8).filter_map(|v|v.as_str().map(|s|s.chars().take(300).collect::<String>())).collect::<Vec<_>>())},"manualAcceptance":result["manualAcceptance"],"checks":checks,"diffSha256":result["verification"]["after"]["diffSha256"],"diffBytes":result["verification"]["after"]["diffBytes"],"untrackedCount":result["verification"]["after"]["untracked"].as_array().map(Vec::len),"interrupted":result["verification"]["interrupted"]})
}
