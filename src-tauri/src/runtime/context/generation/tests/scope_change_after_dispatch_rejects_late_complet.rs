use super::*;
#[test]
pub(super) fn scope_change_after_dispatch_rejects_late_completion() {
    let connection = database();
    bind_scope(&connection);
    let state = crate::test_support::app_state(connection);
    let generation = begin(
        &state,
        BeginGeneration {
            run_id: "run",
            provider_session_id: None,
            provider_id: Some("provider"),
            purpose: "reasoning",
            request_payload: b"request",
            envelope_payload: b"envelope",
            current_instruction_count: 1,
        },
    )
    .unwrap();
    generation.dispatch().unwrap();
    state
        .sqlite_writer
        .write(|connection| {
            connection
                .execute(
                    "UPDATE context_scope_epochs SET epoch=epoch+1 WHERE scope_key='scope'",
                    [],
                )
                .map_err(database_error)?;
            Ok(())
        })
        .unwrap();
    assert!(generation.complete().is_err());
}
#[test]
pub(super) fn forgotten_assertion_before_dispatch_rejects_the_generation_cas() {
    let connection = database();
    bind_personal_assertion(&connection);
    let state = crate::test_support::app_state(connection);
    let generation = begin(
        &state,
        BeginGeneration {
            run_id: "run",
            provider_session_id: None,
            provider_id: Some("provider"),
            purpose: "reasoning",
            request_payload: b"request",
            envelope_payload: b"envelope",
            current_instruction_count: 1,
        },
    )
    .unwrap();
    bind_selected_assertion(&generation);
    state
        .sqlite_writer
        .write(|connection| {
            connection
                .execute(
                    "UPDATE personal_assertions SET erased=1 WHERE id='assertion'",
                    [],
                )
                .map_err(database_error)?;
            Ok(())
        })
        .unwrap();
    assert!(generation.dispatch().is_err());
}
#[test]
pub(super) fn corrected_assertion_before_dispatch_rejects_the_generation_cas() {
    let connection = database();
    bind_personal_assertion(&connection);
    let state = crate::test_support::app_state(connection);
    let generation = begin(
        &state,
        BeginGeneration {
            run_id: "run",
            provider_session_id: None,
            provider_id: Some("provider"),
            purpose: "reasoning",
            request_payload: b"request",
            envelope_payload: b"envelope",
            current_instruction_count: 1,
        },
    )
    .unwrap();
    bind_selected_assertion(&generation);
    state
        .sqlite_writer
        .write(|connection| {
            connection
                .execute(
                    "INSERT INTO personal_transitions(sequence,event_id,assertion_id,metadata)
                         VALUES(1,'correction','assertion','{}')",
                    [],
                )
                .map_err(database_error)?;
            Ok(())
        })
        .unwrap();
    assert!(generation.dispatch().is_err());
}
#[test]
pub(super) fn correction_after_provider_response_is_rejected_before_a_tool_starts() {
    let connection = database();
    bind_personal_assertion(&connection);
    let state = crate::test_support::app_state(connection);
    let generation = begin(
        &state,
        BeginGeneration {
            run_id: "run",
            provider_session_id: None,
            provider_id: Some("provider"),
            purpose: "reasoning",
            request_payload: b"request",
            envelope_payload: b"envelope",
            current_instruction_count: 1,
        },
    )
    .unwrap();
    bind_selected_assertion(&generation);
    generation.dispatch().unwrap();
    generation.complete().unwrap();
    state
        .sqlite_writer
        .write(|connection| {
            connection
                .execute(
                    "INSERT INTO personal_transitions(sequence,event_id,assertion_id,metadata)
                         VALUES(1,'correction-after-response','assertion','{}')",
                    [],
                )
                .map_err(database_error)?;
            Ok(())
        })
        .unwrap();
    assert!(generation.revalidate_dependencies().is_err());
}
#[test]
pub(super) fn forgotten_assertion_after_dispatch_rejects_late_completion() {
    let connection = database();
    bind_personal_assertion(&connection);
    let state = crate::test_support::app_state(connection);
    let generation = begin(
        &state,
        BeginGeneration {
            run_id: "run",
            provider_session_id: None,
            provider_id: Some("provider"),
            purpose: "reasoning",
            request_payload: b"request",
            envelope_payload: b"envelope",
            current_instruction_count: 1,
        },
    )
    .unwrap();
    bind_selected_assertion(&generation);
    generation.dispatch().unwrap();
    state
        .sqlite_writer
        .write(|connection| {
            connection
                .execute(
                    "UPDATE personal_assertions SET erased=1 WHERE id='assertion'",
                    [],
                )
                .map_err(database_error)?;
            Ok(())
        })
        .unwrap();
    assert!(generation.complete().is_err());
}
