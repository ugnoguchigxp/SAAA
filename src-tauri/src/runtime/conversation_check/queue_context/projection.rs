use super::*;

pub(super) fn project_window(
    state: &AppState,
    run_id: &str,
    message_id: &str,
) -> Result<(memory::context_window::ContextWindow, ScopeSnapshot), String> {
    state
        .sqlite_readers
        .read(|connection| project_window_connection(connection, run_id, message_id))
}

pub(super) fn project_window_connection(
    connection: &rusqlite::Connection,
    run_id: &str,
    message_id: &str,
) -> Result<(memory::context_window::ContextWindow, ScopeSnapshot), String> {
    let (window, scope) = {
        let scope = scope::load(connection, run_id)?;
        if scope.status != "resolved" {
            return Err("会話のスコープが無効です。".into());
        }
        for selected in &scope.scopes {
            let current: bool = connection.query_row(
                "SELECT EXISTS(SELECT 1 FROM context_scopes s JOIN context_scope_epochs e ON e.scope_key=s.scope_key WHERE s.scope_key=?1 AND s.state='active' AND e.epoch=?2)",
                rusqlite::params![selected.key, selected.epoch], |row| row.get(0),
            ).map_err(database_error)?;
            if !current {
                return Err("会話のScopeまたは根拠の世代が失効しました。".into());
            }
        }
        let mut loaded =
            memory::context_window::load(connection, PRIMARY_CONVERSATION_ID, message_id, &scope)?;
        // Personal State owns derived current values on the production conversation path.
        // Legacy projections remain stored for audit/migration, not competing current truth.
        if memory::control_plane::memory_enabled() {
            loaded.memory_items.clear();
        }
        let window = memory::context_window::compose(loaded)?;
        (window, scope)
    };
    Ok((window, scope))
}

pub(super) fn scope_data(scope: &ScopeSnapshot) -> Value {
    json!({"status":scope.status,"focus_scope_key":scope.focus_scope_key,"digest":scope.digest,"reason_code":scope.reason_code,
        "scopes":scope.scopes.iter().map(|s| json!({"key":s.key,"kind":s.kind,"relation":s.relation,"epoch":s.epoch})).collect::<Vec<_>>()})
}

pub(super) fn personal_state(
    c: &rusqlite::Connection,
    scope: &ScopeSnapshot,
    current: &str,
) -> Result<(Vec<String>, String), String> {
    if memory::control_plane::memory_enabled() {
        Ok((
            memory::personal_state::snapshots::read(c, scope, current)?,
            memory::personal_state::snapshots::stamp(c, scope)?,
        ))
    } else {
        Ok((Vec::new(), String::new()))
    }
}
