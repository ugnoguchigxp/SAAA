use crate::{database_error, new_id};
use rusqlite::{params, Connection};
use serde_json::{json, Value};

pub fn schedule(c: &Connection, job: &str, conversation: &str, run: &str) -> Result<(), String> {
    let settings: String = c
        .query_row(
            "SELECT settings_json FROM coding_jobs WHERE id=?1",
            [job],
            |r| r.get(0),
        )
        .map_err(database_error)?;
    let automatic = serde_json::from_str::<Value>(&settings)
        .ok()
        .is_some_and(|s| s["terminalAutoAnswer"] == true);
    let mut stmt=c.prepare("SELECT id,kind,input_json FROM terminal_questions WHERE run_id=?1 AND state='awaiting_user' ORDER BY rowid").map_err(database_error)?;
    let rows = stmt
        .query_map([run], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })
        .map_err(database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error)?;
    let mut count: i64 = c
        .query_row(
            "SELECT count(*) FROM terminal_decisions WHERE job_id=?1",
            [job],
            |r| r.get(0),
        )
        .map_err(database_error)?;
    for (id, kind, input) in rows {
        let prior:i64=c.query_row("SELECT count(*) FROM terminal_questions q JOIN terminal_decisions d ON d.question_id=q.id WHERE q.job_id=?1 AND q.kind=?2 AND q.input_json=?3",params![job,kind,input],|r|r.get(0)).map_err(database_error)?;
        if automatic && kind != "permission" && count < 3 && prior == 0 {
            c.execute("INSERT INTO terminal_decisions(question_id,job_id,status) VALUES(?1,?2,'reserved')",params![id,job]).map_err(database_error)?;
            count += 1;
            crate::task_queue::enqueue(
                c,
                conversation,
                "conversation",
                "terminal_question",
                &id,
                0,
                &json!({"questionId":id,"jobId":job,"origin":"terminal_event"}).to_string(),
                Some(8),
            )?;
        } else {
            super::ledger::report(c,conversation,&format!("実装ジョブ {job} が確認を求めています。SAAAの作業状況から回答してください。\n{}",question_text(&serde_json::from_str::<Value>(&input).unwrap_or(Value::Null))))?;
        }
    }
    Ok(())
}
pub(super) fn question_text(input: &Value) -> String {
    input["question"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| input.to_string())
        .chars()
        .take(8000)
        .collect()
}
fn valid_answer(kind: &str, input: &Value, answer: &Value) -> bool {
    if kind == "host_verification" {
        return answer
            .as_str()
            .is_some_and(|s| !s.trim().is_empty() && s.len() <= 32000);
    }
    if kind == "permission" {
        return matches!(answer.as_str(), Some("approve" | "deny"));
    }
    if kind == "question" {
        let Some(questions) = input["questions"].as_array() else {
            return false;
        };
        let Some(answers) = answer.as_object() else {
            return false;
        };
        return !questions.is_empty()
            && questions.len() <= 8
            && answers.len() == questions.len()
            && questions.iter().all(|q| {
                q["question"].as_str().is_some_and(|text| {
                    answers
                        .get(text)
                        .and_then(Value::as_str)
                        .is_some_and(|v| !v.trim().is_empty() && v.len() <= 8000)
                })
            });
    }
    answer
        .as_str()
        .is_some_and(|s| !s.trim().is_empty() && s.len() <= 8000)
}
/// The source is either a real human message or a checked terminal-event decision,
/// never a fabricated user turn. Caller holds the writer transaction.
#[allow(
    clippy::too_many_arguments,
    reason = "Each argument is a separately checked conversation, job, revision, question, answer, or decision binding."
)]
pub fn answer(
    c: &Connection,
    conversation: &str,
    job: &str,
    revision: u64,
    question: &str,
    answer: &Value,
    source: Option<&str>,
    decision: &str,
) -> Result<Option<String>, String> {
    crate::coding::repository::authorize(c, job, conversation)?;
    let (status, run) = crate::coding::repository::revision(c, job, revision)?;
    if status != "awaiting_user" {
        return Err("question_not_paused".into());
    }
    let (qrun, kind, input, qstate): (String, String, String, String) = c
        .query_row(
            "SELECT run_id,kind,input_json,state FROM terminal_questions WHERE id=?1 AND job_id=?2",
            params![question, job],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .map_err(|_| "question_unavailable")?;
    if qrun != run || qstate != "awaiting_user" {
        return Err("stale_question".into());
    }
    let phase: String = c
        .query_row(
            "SELECT phase FROM terminal_runs WHERE run_id=?1",
            [&run],
            |r| r.get(0),
        )
        .map_err(database_error)?;
    if phase != "paused" {
        return Err("question_process_not_stopped".into());
    }
    if source.is_none() && kind == "permission" {
        return Err("permission_requires_user".into());
    }
    let input: Value = serde_json::from_str(&input).map_err(|_| "question_invalid")?;
    if !valid_answer(&kind, &input, answer) {
        return Err("question_answer_invalid".into());
    }
    if let Some(source) = source {
        let exists:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM conversation_messages WHERE id=?1 AND conversation_id=?2 AND role IN ('user','transcript'))",params![source,conversation],|r|r.get(0)).map_err(database_error)?;
        if !exists {
            return Err("source_unavailable".into());
        }
    }
    let session: Option<String> = c
        .query_row(
            "SELECT session_id FROM coding_jobs WHERE id=?1",
            [job],
            |r| r.get(0),
        )
        .map_err(database_error)?;
    if session.is_none() {
        return Err("terminal_session_missing_cannot_resume".into());
    }
    let saved = json!({"kind":kind,"input":input,"answer":answer});
    c.execute("UPDATE terminal_questions SET state='answered',answer_json=?2,decision_id=?3,source_message_id=?4 WHERE id=?1",params![question,saved.to_string(),decision,source]).map_err(database_error)?;
    if kind == "permission" && answer == "deny" {
        c.execute(
            "UPDATE coding_jobs SET state='interrupted',revision=revision+1 WHERE id=?1",
            [job],
        )
        .map_err(database_error)?;
        super::ledger::report(
            c,
            conversation,
            &format!("実装ジョブ {job}: 操作を許可せず、作業を停止しました。"),
        )?;
        return Ok(None);
    }
    let remaining: i64 = c
        .query_row(
            "SELECT count(*) FROM terminal_questions WHERE run_id=?1 AND state='awaiting_user'",
            [&run],
            |r| r.get(0),
        )
        .map_err(database_error)?;
    if remaining > 0 {
        c.execute(
            "UPDATE coding_jobs SET revision=revision+1 WHERE id=?1",
            [job],
        )
        .map_err(database_error)?;
        return Ok(None);
    }
    let next = new_id("codingrun");
    let ledger_source = new_id("terminal_decision");
    c.execute("INSERT INTO terminal_resumes(source_id,job_id,question_id,decision_id,source_message_id) VALUES(?1,?2,?3,?4,?5)",params![ledger_source,job,question,decision,source]).map_err(database_error)?;
    let mut statement=c.prepare("SELECT input_json,answer_json FROM terminal_questions WHERE run_id=?1 AND state='answered' ORDER BY rowid").map_err(database_error)?;
    let all=statement.query_map([&run],|r|Ok(json!({"question":serde_json::from_str::<Value>(&r.get::<_,String>(0)?).unwrap_or(Value::Null),"decision":serde_json::from_str::<Value>(&r.get::<_,String>(1)?).unwrap_or(Value::Null)}))).map_err(database_error)?.collect::<Result<Vec<_>,_>>().map_err(database_error)?;
    let prompt=format!("SAAA host decision for the saved questions: {}. Continue only the original requested work. These decisions do not authorize other operations.",json!(all));
    super::super::service::insert_terminal_resume(
        c,
        job,
        &next,
        &ledger_source,
        &prompt,
        decision,
    )?;
    Ok(Some(next))
}
