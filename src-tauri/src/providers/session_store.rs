use rusqlite::params;

use crate::ipc_contract::ConversationMessage;
use crate::redact::bounded_text;
use crate::{
    database_error, new_id, now_iso, AppState, CleanupOutcome, ProviderFailureKind, StartTurnInput,
};

pub(crate) fn begin_provider_session(
    state: &AppState,
    runtime_run_id: &str,
    provider_id: &str,
    provider_kind: &str,
) -> Result<String, String> {
    let session_id = new_id("provider-session");
    let now = now_iso();
    let connection = state
        .connection
        .lock()
        .map_err(|_| "Database lock unavailable".to_string())?;
    connection
        .execute(
            "INSERT INTO provider_sessions(
               id, runtime_run_id, provider_id, provider_kind, fallback_used, output_started,
               release_status, status, started_at, updated_at
             ) VALUES (
               ?1, ?2, ?3, ?4, 0, 0,
               CASE WHEN ?4='larm' THEN 'not-started' ELSE 'not-applicable' END,
               'running', ?5, ?5
             )",
            params![session_id, runtime_run_id, provider_id, provider_kind, now],
        )
        .map_err(database_error)?;
    Ok(session_id)
}

pub(crate) fn persist_larm_selection(
    state: &AppState,
    session_id: &str,
    allocation: &crate::providers::larm::contracts::ReadyAllocation,
) -> Result<(), String> {
    let selection_reason = match allocation.selection_reason {
        crate::providers::larm::contracts::SelectionReason::Primary => "primary",
        crate::providers::larm::contracts::SelectionReason::Other => "other",
    };
    let connection = state
        .connection
        .lock()
        .map_err(|_| "Database lock unavailable".to_string())?;
    let changed = connection
        .execute(
            "UPDATE provider_sessions
             SET route_id='llm-default', allocation_id=?1, selected_runtime_id=?2,
                 fallback_used=?3, selection_reason=?4, updated_at=?5
             WHERE id=?6 AND provider_kind='larm' AND status='running' AND allocation_id IS NULL",
            params![
                allocation.allocation_id.as_str(),
                allocation.selected_runtime_id.as_str(),
                allocation.fallback_used,
                selection_reason,
                now_iso(),
                session_id
            ],
        )
        .map_err(database_error)?;
    if changed != 1 {
        return Err("LARM provider selection could not be persisted".to_string());
    }
    Ok(())
}

pub(crate) fn mark_provider_output_started(
    state: &AppState,
    session_id: &str,
) -> Result<(), String> {
    let connection = state
        .connection
        .lock()
        .map_err(|_| "Database lock unavailable".to_string())?;
    let changed = connection
        .execute(
            "UPDATE provider_sessions SET output_started=1, updated_at=?1
             WHERE id=?2 AND status='running' AND output_started=0",
            params![now_iso(), session_id],
        )
        .map_err(database_error)?;
    if changed != 1 {
        return Err("Provider output state could not be persisted".to_string());
    }
    Ok(())
}

pub(crate) fn mark_larm_release_pending(state: &AppState, session_id: &str) -> Result<(), String> {
    let connection = state
        .connection
        .lock()
        .map_err(|_| "Database lock unavailable".to_string())?;
    let changed = connection
        .execute(
            "UPDATE provider_sessions SET release_status='pending', updated_at=?1
             WHERE id=?2 AND provider_kind='larm' AND status='running'
               AND release_status='not-started'",
            params![now_iso(), session_id],
        )
        .map_err(database_error)?;
    if changed != 1 {
        return Err("LARM release state could not be persisted".to_string());
    }
    Ok(())
}

pub(crate) fn persist_larm_request_id(
    state: &AppState,
    session_id: &str,
    request_id: Option<&crate::providers::larm::contracts::BoundedIdentifier>,
) -> Result<(), String> {
    let Some(request_id) = request_id else {
        return Ok(());
    };
    let connection = state
        .connection
        .lock()
        .map_err(|_| "Database lock unavailable".to_string())?;
    let changed = connection
        .execute(
            "UPDATE provider_sessions SET request_id=?1, updated_at=?2
             WHERE id=?3 AND provider_kind='larm' AND status='running' AND request_id IS NULL",
            params![request_id.as_str(), now_iso(), session_id],
        )
        .map_err(database_error)?;
    if changed != 1 {
        return Err("LARM request correlation could not be persisted".to_string());
    }
    Ok(())
}

pub(crate) fn finish_larm_provider_session(
    state: &AppState,
    session_id: &str,
    status: &str,
    failure_kind: Option<ProviderFailureKind>,
    cleanup: CleanupOutcome,
) -> Result<(), String> {
    let (release_status, release_failure_kind) = cleanup_persistence(cleanup);
    let connection = state
        .connection
        .lock()
        .map_err(|_| "Database lock unavailable".to_string())?;
    let changed = connection
        .execute(
            "UPDATE provider_sessions
             SET status=?1, failure_reason=?2, failure_kind=?2, release_status=?3,
                 release_failure_kind=?4, updated_at=?5
             WHERE id=?6 AND provider_kind='larm' AND status='running'",
            params![
                status,
                failure_kind.map(ProviderFailureKind::as_str),
                release_status,
                release_failure_kind,
                now_iso(),
                session_id
            ],
        )
        .map_err(database_error)?;
    if changed != 1 {
        return Err("Provider session was already finalized".to_string());
    }
    Ok(())
}

pub(crate) fn finish_dynamic_lan_provider_session(
    state: &AppState,
    session_id: &str,
    status: &str,
    failure_kind: Option<ProviderFailureKind>,
    cleanup: CleanupOutcome,
) -> Result<(), String> {
    let (release_status, release_failure_kind) = cleanup_persistence(cleanup);
    let connection = state
        .connection
        .lock()
        .map_err(|_| "Database lock unavailable".to_string())?;
    let changed = connection
        .execute(
            "UPDATE provider_sessions
             SET status=?1, failure_reason=?2, failure_kind=?2, release_status=?3,
                 release_failure_kind=?4, updated_at=?5
             WHERE id=?6 AND status='running'",
            params![
                status,
                failure_kind.map(ProviderFailureKind::as_str),
                release_status,
                release_failure_kind,
                now_iso(),
                session_id
            ],
        )
        .map_err(database_error)?;
    if changed != 1 {
        return Err("Provider session was already finalized".to_string());
    }
    Ok(())
}

pub(crate) fn cleanup_persistence(cleanup: CleanupOutcome) -> (&'static str, Option<&'static str>) {
    match cleanup {
        CleanupOutcome::NotApplicable => ("not-applicable", None),
        CleanupOutcome::NotStarted => ("not-started", None),
        CleanupOutcome::Released => ("released", None),
        CleanupOutcome::DeferredToTtl { kind } => {
            ("deferred-to-ttl", Some(release_failure_kind_str(kind)))
        }
        CleanupOutcome::DynamicLanDeferredToTtl { kind } => ("deferred-to-ttl", Some(kind)),
    }
}

pub(crate) fn release_failure_kind_str(
    kind: crate::providers::larm::contracts::ReleaseFailureKind,
) -> &'static str {
    use crate::providers::larm::contracts::ReleaseFailureKind as Release;
    match kind {
        Release::Authentication => "authentication",
        Release::Protocol => "protocol",
        Release::Upstream => "upstream",
        Release::Network => "network",
        Release::Timeout => "timeout",
        Release::Internal => "internal",
    }
}

pub(crate) fn finish_provider_session(
    state: &AppState,
    session_id: &str,
    status: &str,
    failure_kind: Option<ProviderFailureKind>,
) -> Result<(), String> {
    let connection = state
        .connection
        .lock()
        .map_err(|_| "Database lock unavailable".to_string())?;
    let changed = connection
        .execute(
            "UPDATE provider_sessions
             SET status = ?1, failure_reason = ?2, failure_kind = ?2, updated_at = ?3
             WHERE id = ?4 AND status = 'running'",
            params![
                status,
                failure_kind.map(ProviderFailureKind::as_str),
                now_iso(),
                session_id
            ],
        )
        .map_err(database_error)?;
    if changed != 1 {
        return Err("Provider session was already finalized".to_string());
    }
    Ok(())
}

pub(crate) fn persist_conversation_success(
    state: &AppState,
    input: &StartTurnInput,
    content: &str,
) -> Result<ConversationMessage, String> {
    let content = bounded_text(content.trim(), 64_000);
    if content.is_empty() {
        return Err("Assistant message cannot be empty".to_string());
    }
    let message = ConversationMessage {
        id: new_id("message"),
        conversation_id: input.conversation_id.clone(),
        role: "assistant".to_string(),
        content,
        created_at: now_iso(),
    };
    let mut connection = state
        .connection
        .lock()
        .map_err(|_| "Database lock unavailable".to_string())?;
    let transaction = connection.transaction().map_err(database_error)?;
    transaction
        .execute(
            "INSERT INTO conversation_messages(id, conversation_id, role, content, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                message.id,
                message.conversation_id,
                message.role,
                message.content,
                message.created_at
            ],
        )
        .map_err(database_error)?;
    transaction
        .execute(
            "UPDATE conversations SET updated_at = ?1 WHERE id = ?2",
            params![message.created_at, input.conversation_id],
        )
        .map_err(database_error)?;
    let changed = transaction
        .execute(
            "UPDATE runtime_runs
             SET status = 'completed', error_message = NULL, completed_at = ?1
             WHERE id = ?2 AND status = 'running'",
            params![now_iso(), input.run_id],
        )
        .map_err(database_error)?;
    if changed != 1 {
        return Err("Runtime run was already finalized".to_string());
    }
    transaction.commit().map_err(database_error)?;
    Ok(message)
}
