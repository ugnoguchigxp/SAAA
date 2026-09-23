use super::*;
pub(super) fn record_event(connection: &Connection, input: &FrontendAuditEventInput) -> Result<(), String> {
    validate_event(input)?;
    let attributes_json = serde_json::to_string(&input.attributes)
        .map_err(|error| format!("Could not encode audit attributes: {error}"))?;
    if attributes_json.len() > MAX_ATTRIBUTES_JSON_BYTES {
        return Err("Audit attributes are too large".to_string());
    }
    connection
        .execute(
            "INSERT INTO audit_events(
               id,occurred_at,component,event_name,phase,outcome,correlation_id,causation_id,
               conversation_id,runtime_run_id,session_id,subject_id,failure_code,attributes_json
             ) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)",
            params![
                new_id("audit"),
                now_iso(),
                input.component,
                input.event_name,
                input.phase,
                input.outcome,
                input.correlation_id,
                input.causation_id,
                input.conversation_id,
                input.runtime_run_id,
                input.session_id,
                input.subject_id,
                input.failure_code,
                attributes_json,
            ],
        )
        .map_err(database_error)?;
    Ok(())
}
pub(super) fn validate_event(input: &FrontendAuditEventInput) -> Result<(), String> {
    if !matches!(
        input.component.as_str(),
        "app"
            | "frontend"
            | "microphone"
            | "voice-asr"
            | "conversation"
            | "provider"
            | "tts"
            | "settings"
            | "voice-policy"
            | "situation"
    ) {
        return Err("Invalid audit component".to_string());
    }
    if !is_event_name(&input.event_name) {
        return Err("Invalid audit event name".to_string());
    }
    if !matches!(
        input.phase.as_str(),
        "request" | "start" | "state" | "progress" | "decision" | "terminal" | "error"
    ) {
        return Err("Invalid audit phase".to_string());
    }
    if input.outcome.as_deref().is_some_and(|outcome| {
        !matches!(
            outcome,
            "success" | "failure" | "cancelled" | "interrupted" | "degraded" | "blocked"
        )
    }) {
        return Err("Invalid audit outcome".to_string());
    }
    for value in [
        input.correlation_id.as_deref(),
        input.causation_id.as_deref(),
        input.conversation_id.as_deref(),
        input.runtime_run_id.as_deref(),
        input.session_id.as_deref(),
        input.subject_id.as_deref(),
        input.failure_code.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        validate_tag(value)?;
    }
    for (key, value) in &input.attributes {
        if !is_allowed_attribute(key) {
            return Err(format!("Unsupported audit attribute: {key}"));
        }
        if let AuditAttributeValue::Tag(value) = value {
            validate_tag(value)?;
        }
    }
    Ok(())
}
pub(super) fn is_event_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 80
        && value.chars().all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
        })
}
pub(super) fn validate_tag(value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 160
        || !value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.' | ':')
        })
    {
        return Err("Invalid audit identifier".to_string());
    }
    Ok(())
}
pub(super) fn is_allowed_attribute(key: &str) -> bool {
    matches!(
        key,
        "state"
            | "previousState"
            | "nextState"
            | "reasonCode"
            | "handoffId"
            | "reasoningRequestId"
            | "providerId"
            | "providerKind"
            | "routeKind"
            | "routeId"
            | "protocol"
            | "scope"
            | "sequence"
            | "enabled"
            | "inputOrigin"
            | "presentationMode"
            | "finalizeCurrent"
            | "fallbackUsed"
            | "selectionReason"
            | "releaseStatus"
            | "source"
            | "policyRevision"
            | "resultCode"
            | "recoverExisting"
            | "commitReason"
            | "fatal"
            | "fromProtocol"
            | "toProtocol"
            | "queueDepth"
            | "deliveryMode"
            | "microphoneEnabled"
            | "systemAudioEnabled"
            | "taskMode"
            | "role"
            | "lane"
            | "namespace"
            | "settingsKey"
            | "schemaVersion"
            | "entryKind"
            | "proposedAttention"
            | "actualExecution"
            | "actualPresentation"
            | "toolName"
            | "durationMs"
    )
}
pub(crate) fn recent_events(connection: &Connection, limit: usize) -> Result<Vec<Value>, String> {
    let limit = limit.clamp(1, 2_000) as i64;
    let mut statement = connection
        .prepare(
            "SELECT sequence,id,occurred_at,component,event_name,phase,outcome,correlation_id,
                    causation_id,conversation_id,runtime_run_id,session_id,subject_id,failure_code,attributes_json
             FROM (
               SELECT audit.sequence,audit.id,audit.occurred_at,audit.component,audit.event_name,
                      audit.phase,audit.outcome,audit.correlation_id,audit.causation_id,
                      audit.conversation_id,audit.runtime_run_id,audit.session_id,audit.subject_id,
                      COALESCE(
                        audit.failure_code,
                        CASE WHEN audit.event_name='runtime-run-finished' THEN (
                          SELECT run.failure_code FROM runtime_runs AS run WHERE run.id=audit.runtime_run_id
                        ) END,
                        CASE WHEN audit.event_name='provider-session-state' THEN (
                          SELECT COALESCE(provider.failure_kind,provider.release_failure_kind)
                          FROM provider_sessions AS provider WHERE provider.id=audit.session_id
                        ) END
                      ) AS failure_code,
                      audit.attributes_json
               FROM audit_events AS audit ORDER BY audit.sequence DESC LIMIT ?1
             ) ORDER BY sequence ASC",
        )
        .map_err(database_error)?;
    let events = statement
        .query_map([limit], audit_event_from_row)
        .map_err(database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error)?;
    Ok(events)
}
pub(super) fn audit_event_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Value> {
    let attributes_json: String = row.get(14)?;
    let attributes = serde_json::from_str::<Value>(&attributes_json).unwrap_or_else(|_| json!({}));
    Ok(json!({
        "sequence": row.get::<_, i64>(0)?,
        "id": row.get::<_, String>(1)?,
        "occurredAt": row.get::<_, String>(2)?,
        "component": row.get::<_, String>(3)?,
        "eventName": row.get::<_, String>(4)?,
        "phase": row.get::<_, String>(5)?,
        "outcome": row.get::<_, Option<String>>(6)?,
        "correlationId": row.get::<_, Option<String>>(7)?,
        "causationId": row.get::<_, Option<String>>(8)?,
        "conversationId": row.get::<_, Option<String>>(9)?,
        "runtimeRunId": row.get::<_, Option<String>>(10)?,
        "sessionId": row.get::<_, Option<String>>(11)?,
        "subjectId": row.get::<_, Option<String>>(12)?,
        "failureCode": row.get::<_, Option<String>>(13)?,
        "attributes": attributes
    }))
}
pub(crate) fn list_ui_events(
    state: &AppState,
    input: AuditEventListInput,
) -> Result<Vec<Value>, String> {
    state.sqlite_readers.read(|connection| {
        let sort_column = match input.sort_by {
            AuditEventSortField::OccurredAt => "CAST(occurred_at AS INTEGER)",
            AuditEventSortField::Component => "component",
            AuditEventSortField::EventName => "event_name",
            AuditEventSortField::Phase => "phase",
            AuditEventSortField::Outcome => "outcome",
            AuditEventSortField::FailureCode => "failure_code",
        };
        let direction = match input.direction {
            SortDirection::Asc => "ASC",
            SortDirection::Desc => "DESC",
        };
        let sql = format!(
            "SELECT sequence,id,occurred_at,component,event_name,phase,outcome,correlation_id,
                    causation_id,conversation_id,runtime_run_id,session_id,subject_id,failure_code,attributes_json
             FROM (
               SELECT audit.sequence,audit.id,audit.occurred_at,audit.component,audit.event_name,
                      audit.phase,audit.outcome,audit.correlation_id,audit.causation_id,
                      audit.conversation_id,audit.runtime_run_id,audit.session_id,audit.subject_id,
                      COALESCE(
                        audit.failure_code,
                        CASE WHEN audit.event_name='runtime-run-finished' THEN (
                          SELECT run.failure_code FROM runtime_runs AS run WHERE run.id=audit.runtime_run_id
                        ) END,
                        CASE WHEN audit.event_name='provider-session-state' THEN (
                          SELECT COALESCE(provider.failure_kind,provider.release_failure_kind)
                          FROM provider_sessions AS provider WHERE provider.id=audit.session_id
                        ) END
                      ) AS failure_code,
                      audit.attributes_json
               FROM audit_events AS audit ORDER BY audit.sequence DESC LIMIT ?1
             ) ORDER BY {sort_column} {direction}, sequence DESC"
        );
        let mut statement = connection.prepare(&sql).map_err(database_error)?;
        let events = statement
            .query_map([AUDIT_UI_EVENT_LIMIT as i64], audit_event_from_row)
            .map_err(database_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)?;
        Ok(events)
    })
}
#[tauri::command]
pub(crate) fn list_audit_events(
    input: AuditEventListInput,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<Value>, String> {
    list_ui_events(&state, input)
}

pub(crate) fn record_tool_execution(
    state: &AppState,
    input: &StartTurnInput,
    session_id: &str,
    tool_call_id: &str,
    tool_name: &str,
    outcome: Option<(&str, std::time::Duration)>,
) -> Result<(), String> {
    let mut attributes = BTreeMap::new();
    if validate_tag(tool_name).is_ok() {
        attributes.insert(
            "toolName".to_string(),
            AuditAttributeValue::Tag(tool_name.to_string()),
        );
    }
    let (phase, outcome_value) = if let Some((outcome, elapsed)) = outcome {
        attributes.insert(
            "durationMs".to_string(),
            AuditAttributeValue::Integer(u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)),
        );
        let outcome = if matches!(
            outcome,
            "success" | "failure" | "cancelled" | "interrupted" | "degraded" | "blocked"
        ) {
            outcome
        } else {
            "failure"
        };
        ("terminal", Some(outcome.to_string()))
    } else {
        ("start", None)
    };
    let subject_id = if validate_tag(tool_call_id).is_ok() {
        tool_call_id.to_string()
    } else {
        session_id.to_string()
    };
    let event = FrontendAuditEventInput {
        component: "provider".to_string(),
        event_name: "tool-execution".to_string(),
        phase: phase.to_string(),
        outcome: outcome_value,
        correlation_id: Some(input.run_id.clone()),
        causation_id: input.source_id.clone(),
        conversation_id: Some(input.conversation_id.clone()),
        runtime_run_id: Some(input.run_id.clone()),
        session_id: Some(session_id.to_string()),
        subject_id: Some(subject_id),
        failure_code: None,
        attributes,
    };
    record_frontend_event(state, &event)
}
