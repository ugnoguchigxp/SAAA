//! Acceptance-boundary regressions: ambiguous JSON and failed persistence.
use super::*;

pub(super) fn response(text: &str) -> Option<String> {
    if text.starts_with("reordered") {
        return Some(
            r#"{"sources":[],"content":"順序の違うJSONも一度だけ。","action":"answer"}"#.into(),
        );
    }
    if text.starts_with("duplicate") {
        return Some(r#"{"action":"answer","content":"仮の回答。","action":"web_search","query":"must not execute"}"#.into());
    }
    None
}

pub(super) async fn run(state: &AppState) -> Result<(), String> {
    turn(state, "context-reordered", "Context試験:reordered", false).await?;
    turn(state, "context-duplicate", "Context試験:duplicate", true).await?;
    state.sqlite_writer.write(|db| {
        db.execute_batch("CREATE TRIGGER context_reject_commit BEFORE INSERT ON conversation_messages WHEN NEW.id='reply_context-commit-failure' BEGIN SELECT RAISE(ABORT,'fixture reject commit'); END;").map_err(crate::database_error)
    })?;
    turn(
        state,
        "context-commit-failure",
        "Context試験:commit-failure",
        true,
    )
    .await?;
    state.sqlite_readers.read(|db| {
        let attempts:i64=db.query_row("SELECT attempts FROM task_queue_jobs WHERE job_key='context-commit-failure' AND kind='user_input'",[],|r|r.get(0)).map_err(crate::database_error)?;
        if attempts!=1 {return Err("commit failure regenerated a published answer".into());}
        Ok(())
    })?;
    Ok(())
}

pub(super) fn verify(state: &AppState, fixture: &Fixture) -> Result<(), String> {
    if fixture
        .searches
        .lock()
        .map_err(|_| "search lock")?
        .iter()
        .any(|query| query == "must not execute")
    {
        return Err("ambiguous action dispatched a tool after publishing an answer".into());
    }
    if fixture
        .spoken
        .lock()
        .map_err(|_| "speech lock")?
        .iter()
        .filter(|text| text.as_str() == "順序の違うJSONも一度だけ。")
        .count()
        != 1
    {
        return Err("reordered answer was not spoken exactly once".into());
    }
    let rows = receipts(state)?;
    let attempt = rows
        .iter()
        .find(|row| row["correlationId"] == "context-reordered" && row["usageStatus"].is_string())
        .ok_or("reordered attempt missing")?;
    let visible = rows
        .iter()
        .find(|row| {
            row["httpAttemptId"] == attempt["httpAttemptId"] && row["firstVisibleMs"].is_number()
        })
        .ok_or("reordered final publication missing")?;
    if attempt["firstContentMs"].as_u64().is_some_and(|content| {
        visible["firstVisibleMs"]
            .as_u64()
            .is_some_and(|visible| visible < content)
    }) {
        return Err("visible time precedes transport content".into());
    }
    Ok(())
}
