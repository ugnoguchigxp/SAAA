use super::*;
pub(super) fn database(input: &Value) -> Result<rusqlite::Connection, String> {
    let c = rusqlite::Connection::open_in_memory().map_err(database_error)?;
    crate::persistence::schema::initialize_database(&c).map_err(database_error)?;
    c.execute(
        "UPDATE personal_scope SET principal='personal-state-synthetic-eval'",
        [],
    )
    .map_err(database_error)?;
    let owner = "user:personal-state-synthetic-eval".to_string();
    c.execute("INSERT INTO context_scopes(scope_key,kind,opaque_id,state,created_at) VALUES(?1,'user','personal-state-synthetic-eval','active',?2)",rusqlite::params![owner,crate::now_iso()]).map_err(database_error)?;
    c.execute(
        "INSERT INTO context_scope_epochs(scope_key,epoch) VALUES(?1,0)",
        [&owner],
    )
    .map_err(database_error)?;
    let sources = input["sources"]
        .as_array()
        .filter(|s| !s.is_empty() && s.len() <= 64)
        .ok_or("harness-sources")?;
    for s in sources {
        let id = s["id"].as_str().ok_or("harness-source")?;
        crate::validate_identifier(id, "synthetic source")?;
        let text = s["text"].as_str().ok_or("harness-source")?;
        if s["version"] != 1 {
            return Err("harness-source-version".into());
        }
        let role = s.get("role").and_then(Value::as_str).unwrap_or("user");
        if !matches!(role, "user" | "assistant" | "transcript") {
            return Err("harness-source-role".into());
        }
        let at = s
            .get("recordedAt")
            .and_then(Value::as_i64)
            .unwrap_or(now() - 31000);
        c.execute(
            "INSERT INTO conversation_messages VALUES(?1,?2,?3,?4,?5)",
            rusqlite::params![
                id,
                crate::PRIMARY_CONVERSATION_ID,
                role,
                text,
                at.to_string()
            ],
        )
        .map_err(database_error)?;
        let key = if let Some(scope) = s.get("scope").and_then(Value::as_str) {
            let (kind, opaque) = scope.split_once(':').ok_or("harness-source-scope")?;
            if !matches!(kind, "project" | "task") {
                return Err("harness-source-scope".into());
            }
            crate::runtime::context::scope::register(&c, kind, opaque)?
        } else {
            owner.clone()
        };
        c.execute(
            "INSERT INTO conversation_message_scopes VALUES(?1,?2,'focus')",
            rusqlite::params![id, key],
        )
        .map_err(database_error)?;
        c.execute(
            "INSERT INTO personal_source_scope_refs VALUES(?1,1,?2)",
            rusqlite::params![id, key],
        )
        .map_err(database_error)?;
        c.execute("UPDATE personal_jobs SET scope_key=?2 WHERE source_sequence=(SELECT sequence FROM personal_sources WHERE message_id=?1 AND available=1)",rusqlite::params![id,key]).map_err(database_error)?;
    }
    c.execute(
        "UPDATE personal_consolidation_settings SET enabled=?1 WHERE id=1",
        [input["consolidationEnabled"].as_bool().unwrap_or(false)],
    )
    .map_err(database_error)?;
    Ok(c)
}

#[cfg(test)]
#[path = "source_tests.rs"]
mod tests;
