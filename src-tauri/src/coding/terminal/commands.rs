use crate::{database_error, new_id, now_iso, AppState};
use rusqlite::params;
use serde_json::{json, Value};
#[tauri::command]
pub fn answer_terminal_question(
    state: tauri::State<'_, AppState>,
    conversation_id: String,
    job_id: String,
    expected_revision: u64,
    question_id: String,
    answer: Value,
) -> Result<Value, String> {
    if answer.to_string().len() > 32000 {
        return Err("question_answer_too_large".into());
    }
    let mut next = None;
    state.sqlite_writer.write(|c|{
        let tx=c.transaction().map_err(database_error)?;
        let message=new_id("message");
        tx.execute("INSERT INTO conversation_messages(id,conversation_id,role,content,created_at) VALUES(?1,?2,'user',?3,?4)",params![message,conversation_id,format!("作業への回答: {answer}"),now_iso()]).map_err(database_error)?;
        next=super::answer(&tx,&conversation_id,&job_id,expected_revision,&question_id,&answer,Some(&message),&new_id("terminal_answer"))?;
        tx.commit().map_err(database_error)
    })?;
    if let Some(run) = next {
        super::spawn(&state, run);
    }
    crate::steward::pump::signal_committed();
    Ok(json!({"accepted":true}))
}
#[tauri::command]
pub fn open_terminal_progress(
    state: tauri::State<'_, AppState>,
    conversation_id: String,
    job_id: String,
) -> Result<(), String> {
    let(directory,kind)=state.sqlite_readers.read(|c|{crate::coding::repository::authorize(c,&job_id,&conversation_id)?;c.query_row("SELECT t.directory,t.terminal FROM terminal_runs t JOIN coding_jobs j ON j.current_run_id=t.run_id WHERE j.id=?1",[&job_id],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?))).map_err(database_error)})?;
    super::launch::viewer(
        &kind,
        std::path::Path::new(&directory),
        &std::env::current_exe().map_err(|_| "helper_executable_missing")?,
    )
}

#[tauri::command]
pub fn confirm_terminal_completion(
    state: tauri::State<'_, AppState>,
    conversation_id: String,
    job_id: String,
    expected_revision: u64,
) -> Result<Value, String> {
    let result=state.sqlite_writer.write(|c|{
        let tx=c.transaction().map_err(database_error)?;
        crate::coding::repository::authorize(&tx,&job_id,&conversation_id)?;
        let(status,run)=crate::coding::repository::revision(&tx,&job_id,expected_revision)?;
        let phase:String=tx.query_row("SELECT phase FROM terminal_runs WHERE run_id=?1",[&run],|r|r.get(0)).map_err(database_error)?;
        if status!="awaiting_user" || phase!="review"{return Err("terminal_review_unavailable".into());}
        let source=new_id("message");
        tx.execute("INSERT INTO conversation_messages(id,conversation_id,role,content,created_at) VALUES(?1,?2,'user',?3,?4)",params![source,conversation_id,format!("実装ジョブ {job_id} の結果を確認し、完了として受け入れました。"),now_iso()]).map_err(database_error)?;
        tx.execute("UPDATE terminal_runs SET phase='verified',result_json=json_set(result_json,'$.complete',json('true'),'$.manualAcceptance',json(?2)) WHERE run_id=?1",params![run,json!({"sourceId":source,"kind":"human_acceptance"}).to_string()]).map_err(database_error)?;
        tx.execute("UPDATE coding_runs SET result_json=json_set(result_json,'$.complete',json('true'),'$.summary','ユーザーが結果を確認し、完了として受け入れました。','$.manualAcceptance',json(?2)) WHERE id=?1",params![run,json!({"sourceId":source,"kind":"human_acceptance"}).to_string()]).map_err(database_error)?;
        tx.execute("UPDATE coding_jobs SET state='completed',revision=revision+1 WHERE id=?1",[&job_id]).map_err(database_error)?;
        crate::coding::repository::event(&tx,&job_id,&run,"manual_completion",json!({"sourceId":source,"kind":"human_acceptance"}))?;
        super::ledger::report(&tx,&conversation_id,&format!("実装ジョブ {job_id}: ユーザーが結果を確認し、完了として受け入れました。"))?;
        tx.commit().map_err(database_error)?;Ok(json!({"accepted":true}))
    })?;
    crate::steward::pump::signal_committed();
    Ok(result)
}

#[tauri::command]
pub fn recover_terminal_job(
    state: tauri::State<'_, AppState>,
    conversation_id: String,
    job_id: String,
    expected_revision: u64,
) -> Result<Value, String> {
    let result =
        super::recovery::stop_unknown(&state, &conversation_id, &job_id, expected_revision)?;
    crate::steward::pump::signal_committed();
    Ok(result)
}
