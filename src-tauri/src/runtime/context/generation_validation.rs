//! Dependency checks for generation dispatch and acceptance.
use super::*;
pub(super) fn validate_dependencies(
    connection: &Connection,
    generation_id: &str,
) -> Result<(), String> {
    let scope_stale: bool = connection
        .query_row(
            "SELECT EXISTS(
               SELECT 1 FROM context_generation_inputs i
               LEFT JOIN context_scope_epochs e ON e.scope_key=i.source_id
               WHERE i.generation_id=?1 AND i.source_kind='scope'
                 AND (e.scope_key IS NULL OR i.source_version!=e.epoch+1)
             )",
            [generation_id],
            |row| row.get(0),
        )
        .map_err(database_error)?;
    if scope_stale {
        return Err("Context generation scope dependency changed".into());
    }
    let policy_stale: bool = connection
        .query_row(
            "SELECT EXISTS(
               SELECT 1 FROM context_generation_inputs i, personal_scope p
               WHERE i.generation_id=?1 AND i.source_kind='policy'
                 AND i.source_id='personal-state-policy' AND p.id='primary'
                 AND i.source_version!=p.policy_revision
             )",
            [generation_id],
            |row| row.get(0),
        )
        .map_err(database_error)?;
    if policy_stale {
        return Err("Context generation policy dependency changed".into());
    }
    let personal_state_stale: bool = connection
        .query_row(
            "SELECT EXISTS(
               SELECT 1 FROM context_generation_inputs i
               LEFT JOIN personal_assertions a ON a.id=i.source_id
               WHERE i.generation_id=?1 AND i.source_kind='personal-state' AND i.selected=1
                 AND (a.id IS NULL OR a.erased=1 OR i.source_version!=(
                   SELECT COALESCE(MAX(t.sequence),0)+1 FROM personal_transitions t
                   WHERE t.assertion_id=i.source_id
                 ))
             )",
            [generation_id],
            |row| row.get(0),
        )
        .map_err(database_error)?;
    if personal_state_stale {
        return Err("Context generation Personal State dependency changed".into());
    }
    let pending_source_stale: bool = connection
        .query_row(
            "SELECT EXISTS(
               SELECT 1 FROM context_generation_inputs i
               WHERE i.generation_id=?1 AND i.source_kind='personal-pending' AND i.selected=1
                 AND NOT EXISTS(
                   SELECT 1 FROM personal_sources p
                   WHERE p.message_id=i.source_id AND p.version=i.source_version
                     AND p.available=1
                 )
             )",
            [generation_id],
            |row| row.get(0),
        )
        .map_err(database_error)?;
    if pending_source_stale {
        return Err("Context generation pending source dependency changed".into());
    }
    let task_continuation_stale: bool = connection
        .query_row(
            "SELECT EXISTS(
               SELECT 1 FROM context_generation_inputs i
               LEFT JOIN coding_jobs j ON j.id=i.source_id
               LEFT JOIN coding_runs r ON r.id=j.current_run_id
               WHERE i.generation_id=?1 AND i.source_kind='task-continuation' AND i.selected=1
                 AND (j.id IS NULL OR i.source_version!=j.revision
                      OR r.state NOT IN ('starting','running','stopping','outcome_unknown'))
             )",
            [generation_id],
            |row| row.get(0),
        )
        .map_err(database_error)?;
    if task_continuation_stale {
        return Err("Context generation task continuation changed".into());
    }
    let delegation_continuation_stale: bool = connection
        .query_row(
            "SELECT EXISTS(
               SELECT 1 FROM context_generation_inputs i
               LEFT JOIN steward_tasks t ON t.id=i.source_id
               LEFT JOIN steward_delegations d ON d.id=t.delegation_id
               LEFT JOIN steward_goals g ON g.id=d.goal_id
               WHERE i.generation_id=?1 AND i.source_kind='delegation-continuation' AND i.selected=1
                 AND (t.id IS NULL OR i.source_version!=t.revision
                      OR t.loop_state NOT IN ('queued','running','awaiting_user')
                      OR d.status!='active' OR d.superseded_by IS NOT NULL
                      OR g.status!='active' OR g.superseded_by IS NOT NULL)
             )",
            [generation_id],
            |row| row.get(0),
        )
        .map_err(database_error)?;
    if delegation_continuation_stale {
        return Err("Context generation delegation continuation changed".into());
    }
    let current: Option<(String, String)> = connection
        .query_row(
            "SELECT i.source_digest,m.content
             FROM context_generation_inputs i
             JOIN conversation_messages m ON m.id=i.source_id
             WHERE i.generation_id=?1 AND i.source_kind='current-instruction'",
            [generation_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(database_error)?;
    let Some((expected, content)) = current else {
        return Err("Context generation current instruction is unavailable".into());
    };
    if digest(content.as_bytes()) != expected {
        return Err("Context generation current instruction changed".into());
    }
    Ok(())
}

pub(super) fn resolve_provider(
    connection: &Connection,
    run_id: &str,
    provider_session_id: Option<&str>,
    provider_id: Option<&str>,
) -> Result<String, String> {
    match (provider_session_id, provider_id) {
        (Some(session_id), None) => connection
            .query_row(
                "SELECT provider_id FROM provider_sessions
                 WHERE id=?1 AND runtime_run_id=?2 AND status='running'",
                params![session_id, run_id],
                |row| row.get(0),
            )
            .map_err(database_error),
        (None, Some(provider_id)) if !provider_id.trim().is_empty() => Ok(provider_id.to_string()),
        _ => Err("Context generation provider binding is invalid".into()),
    }
}

pub(super) fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
