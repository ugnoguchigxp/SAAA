//! Isolated verification of the production tool adapter, admission checks, and warm cache.
use crate::{
    persistence,
    tts_dictionary::{self, tools},
    AppState, RunCancellation, StartTurnInput, PRIMARY_CONVERSATION_ID,
};
use rusqlite::{params, Connection};
use serde_json::{json, Value};

fn input(state: &AppState, key: &str, text: &str) -> Result<StartTurnInput, String> {
    state.sqlite_writer.write(|db| {
        db.execute("INSERT INTO conversation_messages(id,conversation_id,role,content,created_at) VALUES(?1,?2,'user',?3,'fixture')",
            params![format!("check_{key}"),PRIMARY_CONVERSATION_ID,text]).map_err(crate::database_error)?;
        db.execute("INSERT INTO runtime_runs(id,conversation_id,route_kind,status,started_at) VALUES(?1,?2,'conversation.respond','running','fixture')",
            params![format!("run_{key}"),PRIMARY_CONVERSATION_ID]).map_err(crate::database_error)?;
        Ok(())
    })?;
    Ok(StartTurnInput {
        run_id: format!("run_{key}"),
        conversation_id: PRIMARY_CONVERSATION_ID.into(),
        content: text.into(),
        workspace_path: None,
        retry_input_message_id: None,
        source_id: None,
        scope_refs: vec![],
        input_origin: "text".into(),
        presentation_mode: "visual-and-spoken".into(),
    })
}
fn call(
    state: &AppState,
    input: &StartTurnInput,
    name: &str,
    arguments: Value,
) -> Result<Value, String> {
    serde_json::from_str(&tools::execute(
        Some(state),
        input,
        name,
        &arguments.to_string(),
        &RunCancellation::default(),
    ))
    .map_err(|e| e.to_string())
}
fn lookup(state: &AppState, input: &StartTurnInput, spoken: &str) -> Result<Value, String> {
    call(
        state,
        input,
        "lookup_tts_pronunciation",
        json!({"written":"今日","proposedSpoken":spoken}),
    )
}
fn set(
    state: &AppState,
    input: &StartTurnInput,
    lookup: &Value,
    mode: &str,
) -> Result<Value, String> {
    call(
        state,
        input,
        "set_tts_pronunciation",
        json!({"lookupId":lookup["lookupId"],"mode":mode}),
    )
}
fn status(result: &Value, expected: &str) -> Result<(), String> {
    if result["status"] == expected {
        Ok(())
    } else {
        Err(format!("expected {expected}, got {result}"))
    }
}
fn publish(state: &AppState, input: &StartTurnInput) -> Result<(), String> {
    state.sqlite_writer.write(|db| {
        db.execute(
            "UPDATE runtime_runs SET status='completed' WHERE id=?1",
            [&input.run_id],
        )
        .map_err(crate::database_error)?;
        Ok(())
    })?;
    tools::finish_turn(state, &input.conversation_id, &input.run_id, true);
    Ok(())
}

pub(super) fn run() -> Result<Value, String> {
    let db = Connection::open_in_memory().map_err(crate::database_error)?;
    persistence::schema::initialize_database(&db).map_err(crate::database_error)?;
    let state = crate::test_support::app_state(db);
    let first = input(&state, "dict-add", "今日をきょうと読んで")?;
    let found = lookup(&state, &first, "きょう")?;
    status(&found, "missing")?;
    let mut wrong = input(&state, "dict-wrong", "別の会話")?;
    wrong.conversation_id = "other".into();
    status(
        &set(&state, &wrong, &found, "register")?,
        "proposal_expired",
    )?;
    status(&set(&state, &first, &found, "register")?, "added")?;
    let snapshot = state.tts_dictionary_cache.snapshot(&state.sqlite_readers)?;
    if snapshot.apply("今日") != "きょう" {
        return Err("saved pronunciation not published".into());
    }
    let same = lookup(&state, &first, "きょう")?;
    status(&same, "same")?;
    status(&set(&state, &first, &same, "register")?, "unchanged")?;
    if !std::sync::Arc::ptr_eq(
        &snapshot,
        &state.tts_dictionary_cache.snapshot(&state.sqlite_readers)?,
    ) {
        return Err("same pronunciation rebuilt cache".into());
    }
    let different = lookup(&state, &first, "こんにち")?;
    status(
        &set(&state, &first, &different, "register")?,
        "confirmation_required",
    )?;
    let early = input(&state, "dict-early", "はい")?;
    status(
        &set(&state, &early, &different, "confirm")?,
        "proposal_expired",
    )?;
    // A fresh question after the early response produces a fresh, published confirmation.
    let question = input(&state, "dict-question", "今日をこんにちと読んで")?;
    let different = lookup(&state, &question, "こんにち")?;
    status(
        &set(&state, &question, &different, "register")?,
        "confirmation_required",
    )?;
    publish(&state, &question)?;
    let yes = input(&state, "dict-yes", "はい")?;
    status(&set(&state, &yes, &different, "confirm")?, "updated")?;
    status(
        &set(&state, &yes, &different, "confirm")?,
        "proposal_expired",
    )?;
    // Intervening saved input invalidates a stale 'yes' even if its turn has not completed.
    let question = input(&state, "dict-stale-question", "今日をきょうと読んで")?;
    let different = lookup(&state, &question, "きょう")?;
    status(
        &set(&state, &question, &different, "register")?,
        "confirmation_required",
    )?;
    publish(&state, &question)?;
    input(&state, "dict-other", "別の話です")?;
    let late = input(&state, "dict-late", "はい")?;
    status(
        &set(&state, &late, &different, "confirm")?,
        "proposal_expired",
    )?;
    // Explicit replacement still checks current SQLite state after lookup.
    let change = input(&state, "dict-change", "今日をキョウに変更して")?;
    let stale = lookup(&state, &change, "キョウ")?;
    state.sqlite_writer.write(|db| {
        let mutation = tts_dictionary::service::save(
            db,
            Some("今日"),
            &tts_dictionary::Entry {
                written: "今日".into(),
                spoken: "キョー".into(),
            },
            &tts_dictionary::ExpectedEntry {
                spoken: Some("こんにち".into()),
            },
            "fixture",
            |_| Ok(()),
        )?;
        if let Some(index) = mutation.dictionary {
            state.tts_dictionary_cache.publish_compiled(index);
        }
        Ok(())
    })?;
    status(&set(&state, &change, &stale, "replace")?, "conflict")?;
    let fresh = lookup(&state, &change, "キョウ")?;
    status(&set(&state, &change, &fresh, "replace")?, "updated")?;
    let cancelled = input(&state, "dict-cancel", "今日をきょうに変更して")?;
    let found = lookup(&state, &cancelled, "きょう")?;
    state.sqlite_writer.write(|db| {
        db.execute(
            "UPDATE runtime_runs SET status='cancelled' WHERE id=?1",
            [&cancelled.run_id],
        )
        .map_err(crate::database_error)?;
        Ok(())
    })?;
    status(&set(&state, &cancelled, &found, "replace")?, "cancelled")?;
    let invalid = input(&state, "dict-invalid", "読みの訂正")?;
    status(&lookup(&state, &invalid, "")?, "failed")?;
    status(
        &call(
            &state,
            &invalid,
            "lookup_tts_pronunciation",
            json!({"written":"今日","proposedSpoken":"きょう","force":true}),
        )?,
        "failed",
    )?;
    let warm = state.tts_dictionary_cache.snapshot(&state.sqlite_readers)?;
    state.sqlite_writer.write(|db| {
        db.execute_batch("DROP TABLE tts_dictionary")
            .map_err(crate::database_error)
    })?;
    if !std::sync::Arc::ptr_eq(
        &warm,
        &state.tts_dictionary_cache.snapshot(&state.sqlite_readers)?,
    ) {
        return Err("warm cache accessed dictionary DB".into());
    }
    Ok(json!({"cases":15,"warmCacheWithoutDictionaryTable":true}))
}
