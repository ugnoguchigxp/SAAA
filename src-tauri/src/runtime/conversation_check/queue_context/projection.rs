use super::*;
use sha2::{Digest, Sha256};

type PersonalProjection = (
    Vec<String>,
    String,
    std::collections::BTreeSet<saaa_personal_state_core::SourceKey>,
);

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
) -> Result<PersonalProjection, String> {
    if memory::control_plane::memory_enabled() {
        let (mut texts, mut inputs) =
            memory::personal_state::projection::snapshots::view(c, scope, current)?;
        let (references, dependencies) =
            memory::personal_state::sources::episode_export::history_view(c, scope, current)?;
        inputs.extend(dependencies);
        if let Some(text) =
            references.filter(|t| texts.iter().map(String::len).sum::<usize>() + t.len() <= 32000)
        {
            texts.push(text);
        }
        let stamp = memory::personal_state::projection::snapshots::stamp(c, scope)?;
        let stamp = format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&(stamp, &inputs)).map_err(|e| e.to_string())?)
        );
        Ok((texts, stamp, inputs))
    } else {
        Ok((Vec::new(), String::new(), std::collections::BTreeSet::new()))
    }
}
