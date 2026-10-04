use super::questions::{answer, question_text};
use crate::{database_error, new_id, AppState};
use rusqlite::params;
use serde_json::{json, Value};

pub fn automatic_context(state: &AppState, question: &str) -> Result<Value, String> {
    state.sqlite_readers.read(|c|{
        let(job,conversation,kind,input,revision,phase,request,source,text):(String,String,String,String,u64,String,String,String,String)=c.query_row("SELECT j.id,j.conversation_id,q.kind,q.input_json,j.revision,t.phase,r.payload,j.source_id,m.content FROM terminal_questions q JOIN coding_jobs j ON j.id=q.job_id JOIN terminal_runs t ON t.run_id=q.run_id JOIN coding_runs r ON r.id=q.run_id JOIN conversation_messages m ON m.id=j.source_id AND m.conversation_id=j.conversation_id WHERE q.id=?1 AND q.state='awaiting_user' AND j.current_run_id=q.run_id",[question],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?,r.get(7)?,r.get(8)?))).map_err(|_|"question_unavailable")?;
        if kind=="permission" || phase!="paused"{return Err("question_not_automatic".into());}
        Ok(json!({"jobId":job,"conversationId":conversation,"kind":kind,"input":serde_json::from_str::<Value>(&input).map_err(|_|"question_invalid")?,"revision":revision,"request":request,"sources":[{"id":source,"text":text}],"questionId":question}))
    })
}
pub fn automatic_answer(
    state: &AppState,
    context: &Value,
    output: &Value,
    acceptance: Option<(
        &crate::providers::service_registry::ResolvedRoute,
        tokio::time::Instant,
    )>,
) -> Result<(), String> {
    let conversation = context["conversationId"]
        .as_str()
        .ok_or("decision_invalid")?;
    let job = context["jobId"].as_str().ok_or("decision_invalid")?;
    let question = context["questionId"].as_str().ok_or("decision_invalid")?;
    let allow = output["action"] == "answer"
        && context["sources"].as_array().is_some_and(|sources| {
            sources.iter().any(|s| {
                output["sourceId"] == s["id"]
                    && output["quote"].as_str().is_some_and(|quote| {
                        quote.len() >= 2
                            && s["text"].as_str().is_some_and(|text| text.contains(quote))
                            && all_answers_in_quote(&output["answer"], quote)
                    })
            })
        });
    let mut next = None;
    state.sqlite_writer.write(|c|{
        let tx=c.transaction().map_err(database_error)?;
        let current:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM terminal_questions q JOIN coding_jobs j ON j.id=q.job_id JOIN terminal_runs t ON t.run_id=q.run_id WHERE q.id=?1 AND q.state='awaiting_user' AND j.current_run_id=q.run_id AND j.revision=?2 AND t.phase='paused')",params![question,context["revision"].as_u64()],|r|r.get(0)).map_err(database_error)?;
        if !current || crate::coding::repository::authorize(&tx,job,conversation).is_err(){tx.execute("UPDATE terminal_decisions SET status='superseded' WHERE question_id=?1",[question]).map_err(database_error)?;return tx.commit().map_err(database_error);}
        if let Some((route,deadline))=acceptance {
            if tokio::time::Instant::now()>=deadline {return Err("purpose_deadline_exceeded".into());}
            crate::providers::service_registry::validate_active(&tx,route)?;
            crate::providers::service_registry::operations::accepted(&tx,question,route)?;
        }
        tx.execute("UPDATE terminal_decisions SET source_id=?2,quote=?3,answer_json=?4 WHERE question_id=?1",params![question,output["sourceId"].as_str(),output["quote"].as_str(),output.to_string()]).map_err(database_error)?;
        if allow{
            let count:i64=tx.query_row("SELECT count(*) FROM terminal_decisions WHERE job_id=?1",[job],|r|r.get(0)).map_err(database_error)?;
            if count<=3{next=answer(&tx,conversation,job,context["revision"].as_u64().ok_or("decision_invalid")?,question,&output["answer"],None,&new_id("terminal_answer"))?;}
        }
        let answered:bool=tx.query_row("SELECT state='answered' FROM terminal_questions WHERE id=?1",[question],|r|r.get(0)).map_err(database_error)?;
        tx.execute("UPDATE terminal_decisions SET status=?2 WHERE question_id=?1",params![question,if answered{"answered"}else{"manual"}]).map_err(database_error)?;
        if !answered{super::ledger::report(&tx,conversation,&format!("実装ジョブ {job} が確認を求めています。元の依頼だけでは回答を確定できないため、作業状況から回答してください。\n{}",question_text(&context["input"])))?;}
        tx.commit().map_err(database_error)
    })?;
    if let Some(run) = next {
        super::spawn(state, run);
    }
    crate::steward::pump::signal_committed();
    Ok(())
}
fn all_answers_in_quote(answer: &Value, quote: &str) -> bool {
    match answer {
        Value::String(s) => !s.is_empty() && quote.contains(s),
        Value::Object(map) => {
            !map.is_empty()
                && map.values().all(|v| {
                    v.as_str()
                        .is_some_and(|s| !s.is_empty() && quote.contains(s))
                })
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn answers_must_be_literal_in_the_same_source_quote() {
        assert!(all_answers_in_quote(
            &json!({"Color?":"Blue","Test?":"bun test"}),
            "Use Blue, and run bun test"
        ));
        assert!(!all_answers_in_quote(
            &json!({"Color?":"Red"}),
            "Use Blue, and run bun test"
        ));
        assert!(!all_answers_in_quote(&json!(""), "Use Blue"));
    }
}
