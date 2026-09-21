use crate::persistence::SqliteWriter;
use crate::{database_error, new_id, now_iso, AppState};
use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};
use std::sync::{Arc, Mutex};

pub(crate) const MAX_PROVIDER_REQUEST_BYTES: usize = 96_000;

pub(crate) struct GenerationHandle {
    writer: Arc<SqliteWriter>,
    id: String,
    world: Mutex<Option<super::world::turn::WorldReceipt>>,
    #[cfg(test)]
    world_observation: Mutex<Option<&'static str>>,
}

impl GenerationHandle {
    /// Completes the immutable, digest-only receipt before the dispatch CAS. The
    /// request digest was captured from the final serialized provider body in
    /// `begin`; this call binds that body to the exact required candidate set.
    pub(crate) fn set_required_receipt(&self, required_set_digest: &str) -> Result<(), String> {
        if required_set_digest.len() != 64
            || !required_set_digest
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err("Required context receipt digest is invalid".into());
        }
        self.writer.write(|connection| {
            let changed = connection
                .execute(
                    "UPDATE context_generations
                     SET required_set_digest=?2
                     WHERE id=?1 AND status='planned' AND required_set_digest IS NULL",
                    params![self.id, required_set_digest],
                )
                .map_err(database_error)?;
            if changed != 1 {
                return Err("Context generation receipt could not be recorded".into());
            }
            Ok(())
        })
    }

    pub(crate) fn set_health(&self, status: &str) -> Result<(), String> {
        if !matches!(status, "green" | "yellow" | "red") {
            return Err("Context health status is invalid".into());
        }
        self.writer.write(|connection| {
            let changed = connection
                .execute(
                    "UPDATE context_generations SET health_status=?2
                     WHERE id=?1 AND status IN ('planned','dispatched')",
                    params![self.id, status],
                )
                .map_err(database_error)?;
            if changed != 1 {
                return Err("Context generation health could not be updated".into());
            }
            Ok(())
        })
    }

    #[allow(clippy::too_many_arguments)] // Mirrors the normalized manifest row contract.
    pub(crate) fn add_input(
        &self,
        source_kind: &str,
        source_id: &str,
        source_version: u64,
        source_digest: &str,
        requirement: &str,
        placement: &str,
        selected: bool,
        omission_reason: Option<&str>,
    ) -> Result<(), String> {
        self.writer.write(|connection| {
            connection
                .execute(
                    "INSERT INTO context_generation_inputs(
                       generation_id,source_kind,source_id,source_version,source_digest,
                       requirement,placement,selected,omission_reason,metadata_json
                     ) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,'{}')
                     ON CONFLICT(generation_id,source_kind,source_id,source_version) DO NOTHING",
                    params![
                        self.id,
                        source_kind,
                        source_id,
                        source_version,
                        source_digest,
                        requirement,
                        placement,
                        selected,
                        omission_reason
                    ],
                )
                .map_err(database_error)?;
            Ok(())
        })
    }

    pub(crate) fn dispatch(&self) -> Result<(), String> {
        self.writer.write(|connection| {
            validate_dependencies(connection, &self.id)?;
            let changed = connection
                .execute(
                    "UPDATE context_generations
                     SET status='dispatched'
                     WHERE id=?1 AND status='planned' AND health_status!='red'",
                    [&self.id],
                )
                .map_err(database_error)?;
            if changed != 1 {
                return Err("Context generation could not be dispatched".into());
            }
            Ok(())
        })
    }

    pub(crate) fn complete(&self) -> Result<(), String> {
        self.finish("completed", None)
    }

    pub(crate) fn fail(&self, failure_kind: &str) -> Result<(), String> {
        self.finish("failed", Some(failure_kind))
    }

    pub(crate) fn cancel(&self) -> Result<(), String> {
        self.finish("cancelled", Some("cancelled"))
    }

    fn finish(&self, status: &str, failure_kind: Option<&str>) -> Result<(), String> {
        self.writer.write(|connection| {
            validate_dependencies(connection, &self.id)?;
            let changed = connection
                .execute(
                    "UPDATE context_generations
                     SET status=?2,failure_kind=?3,completed_at=?4
                     WHERE id=?1 AND status IN ('planned','dispatched')",
                    params![self.id, status, failure_kind, now_iso()],
                )
                .map_err(database_error)?;
            if changed != 1 {
                return Err("Context generation was already finalized".into());
            }
            Ok(())
        })?;
        self.observe_world();
        Ok(())
    }

    pub(crate) fn attach_world(&self, receipt: super::world::turn::WorldReceipt) {
        *self
            .world
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(receipt);
    }

    fn observe_world(&self) {
        let receipt = self
            .world
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(receipt) = receipt {
            let outcome = super::world::turn::observe_receipt(&receipt);
            #[cfg(test)]
            {
                *self
                    .world_observation
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(outcome);
            }
            #[cfg(not(test))]
            let _ = outcome;
        }
    }

    #[cfg(test)]
    pub(crate) fn world_observation(&self) -> Option<&'static str> {
        *self
            .world_observation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl Drop for GenerationHandle {
    fn drop(&mut self) {
        let _ = self.writer.write(|connection| {
            connection
                .execute(
                    "UPDATE context_generations
                     SET status='interrupted',failure_kind='generation-handle-dropped',completed_at=?2
                     WHERE id=?1 AND status IN ('planned','dispatched')",
                    params![self.id, now_iso()],
                )
                .map_err(database_error)?;
            Ok(())
        });
    }
}

pub(crate) struct BeginGeneration<'a> {
    pub(crate) run_id: &'a str,
    pub(crate) provider_session_id: Option<&'a str>,
    pub(crate) provider_id: Option<&'a str>,
    pub(crate) purpose: &'a str,
    pub(crate) request_payload: &'a [u8],
    pub(crate) envelope_payload: &'a [u8],
    pub(crate) current_instruction_count: usize,
}

/// Creates a manifest before network dispatch. Only digests and source identity are persisted.
pub(crate) fn begin(
    state: &AppState,
    input: BeginGeneration<'_>,
) -> Result<GenerationHandle, String> {
    let id = new_id("context_generation");
    let valid = state.sqlite_writer.write(|connection| {
        let transaction = connection.transaction().map_err(database_error)?;
        let provider_id = resolve_provider(
            &transaction,
            input.run_id,
            input.provider_session_id,
            input.provider_id,
        )?;
        let (message_id, content): (String, String) = transaction
            .query_row(
                "SELECT m.id,m.content
                 FROM runtime_runs r
                 JOIN conversation_messages m ON m.id=r.input_message_id
                 WHERE r.id=?1 AND r.status='running'
                   AND m.conversation_id=r.conversation_id
                   AND m.role IN ('user','transcript')",
                [input.run_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(database_error)?;
        let ordinal: u64 = transaction
            .query_row(
                "SELECT COALESCE(MAX(ordinal),0)+1 FROM context_generations WHERE run_id=?1",
                [input.run_id],
                |row| row.get(0),
            )
            .map_err(database_error)?;
        let scope_digest: Option<String> = transaction
            .query_row(
                "SELECT scope_digest FROM runtime_scope_resolutions WHERE run_id=?1",
                [input.run_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(database_error)?;
        let instruction_valid = input.current_instruction_count == 1;
        let budget_valid = input.request_payload.len() <= MAX_PROVIDER_REQUEST_BYTES;
        let valid = instruction_valid && budget_valid;
        let now = now_iso();
        transaction
            .execute(
                "INSERT INTO context_generations(
                   id,run_id,provider_session_id,provider_id,ordinal,purpose,
                   envelope_digest,request_digest,projected_bytes,current_instruction_count,
                   health_status,status,failure_kind,started_at,completed_at,scope_digest
                 ) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)",
                params![
                    id,
                    input.run_id,
                    input.provider_session_id,
                    provider_id,
                    ordinal,
                    input.purpose,
                    digest(input.envelope_payload),
                    digest(input.request_payload),
                    input.request_payload.len() as u64,
                    input.current_instruction_count as u64,
                    if valid { "green" } else { "red" },
                    if valid { "planned" } else { "failed" },
                    if instruction_valid && budget_valid {
                        None::<&str>
                    } else if !instruction_valid {
                        Some("current-instruction-count")
                    } else {
                        Some("provider-input-budget")
                    },
                    now,
                    if valid {
                        None::<String>
                    } else {
                        Some(now_iso())
                    },
                    scope_digest,
                ],
            )
            .map_err(database_error)?;
        transaction
            .execute(
                "INSERT INTO context_generation_inputs(
                   generation_id,source_kind,source_id,source_version,source_digest,
                   requirement,placement,selected,metadata_json
                 )
                 SELECT ?1,'scope',r.scope_key,r.epoch+1,?2,'must','reference',1,
                   json_object('relation',r.relation)
                 FROM runtime_run_scopes r WHERE r.run_id=?3",
                params![id, digest(input.run_id.as_bytes()), input.run_id],
            )
            .map_err(database_error)?;
        let policy_revision: u64 = transaction
            .query_row(
                "SELECT policy_revision FROM personal_scope WHERE id='primary'",
                [],
                |row| row.get(0),
            )
            .map_err(database_error)?;
        transaction
            .execute(
                "INSERT INTO context_generation_inputs(
                   generation_id,source_kind,source_id,source_version,source_digest,
                   requirement,placement,selected,metadata_json
                 ) VALUES(?1,'policy','personal-state-policy',?2,?3,'must','reference',1,'{}')",
                params![
                    id,
                    policy_revision,
                    digest(policy_revision.to_string().as_bytes())
                ],
            )
            .map_err(database_error)?;
        transaction
            .execute(
                "INSERT INTO context_generation_inputs(
                   generation_id,source_kind,source_id,source_version,source_digest,
                   requirement,placement,selected,metadata_json
                 ) VALUES(?1,'current-instruction',?2,?3,?4,'must','base',1,
                   '{\"instructionAuthority\":\"current\"}')",
                params![id, message_id, 1_u64, digest(content.as_bytes())],
            )
            .map_err(database_error)?;
        transaction.commit().map_err(database_error)?;
        Ok(valid)
    })?;
    if !valid {
        return Err(if input.current_instruction_count != 1 {
            "Context generation must contain exactly one current instruction"
        } else {
            "Context generation exceeds the conservative provider input budget"
        }
        .into());
    }
    Ok(GenerationHandle {
        writer: state.sqlite_writer.clone(),
        id,
        world: Mutex::new(None),
        #[cfg(test)]
        world_observation: Mutex::new(None),
    })
}

fn validate_dependencies(connection: &Connection, generation_id: &str) -> Result<(), String> {
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

#[cfg(test)]
pub(crate) fn begin_direct_dispatched(
    state: &AppState,
    run_id: &str,
    provider_id: &str,
    purpose: &str,
    payload: &[u8],
) -> Result<GenerationHandle, String> {
    let generation = begin(
        state,
        BeginGeneration {
            run_id,
            provider_session_id: None,
            provider_id: Some(provider_id),
            purpose,
            request_payload: payload,
            envelope_payload: payload,
            current_instruction_count: 1,
        },
    )?;
    generation.dispatch()?;
    Ok(generation)
}

pub(crate) fn record_red(state: &AppState, run_id: &str, reason: &str) {
    if let Ok(generation) = begin(
        state,
        BeginGeneration {
            run_id,
            provider_session_id: None,
            provider_id: Some("context-broker"),
            purpose: "context-build",
            request_payload: reason.as_bytes(),
            envelope_payload: reason.as_bytes(),
            current_instruction_count: 1,
        },
    ) {
        let _ = generation.set_health("red");
        let _ = generation.fail(reason);
    }
}

pub(crate) fn finish_result<T>(
    generation: &GenerationHandle,
    result: &Result<T, String>,
    cancelled: bool,
) -> Result<(), String> {
    match result {
        Ok(_) => generation.complete(),
        Err(_) if cancelled => generation.cancel(),
        Err(_) => generation.fail("provider-error"),
    }
}

fn resolve_provider(
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

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
pub(crate) fn assert_two_round_tool_manifest(state: &crate::AppState, run_id: &str) {
    state
        .sqlite_readers
        .read(|connection| {
            let manifest: (u64, u64, u64) = connection
                .query_row(
                    "SELECT COUNT(*),
                       SUM(purpose='reasoning' AND status='completed'),
                       SUM(purpose='tool-followup' AND status='completed')
                     FROM context_generations WHERE run_id=?1",
                    [run_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .map_err(database_error)?;
            assert_eq!(manifest, (2, 1, 1));
            let invalid: bool = connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM context_generations
                     WHERE run_id=?1 AND current_instruction_count!=1)",
                    [run_id],
                    |row| row.get(0),
                )
                .map_err(database_error)?;
            assert!(!invalid);
            let tool_snapshots: u64 = connection
                .query_row(
                    "SELECT COUNT(*) FROM context_generation_inputs i
                     JOIN context_generations g ON g.id=i.generation_id
                     WHERE g.run_id=?1 AND i.source_kind='static-tool-offer'
                       AND i.selected=1 AND length(i.source_digest)=64",
                    [run_id],
                    |row| row.get(0),
                )
                .map_err(database_error)?;
            assert!(tool_snapshots > 0);
            Ok(())
        })
        .unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn database() -> Connection {
        let connection = Connection::open_in_memory().expect("database opens");
        crate::persistence::schema::initialize_database(&connection).expect("schema initializes");
        connection
            .execute(
                "INSERT INTO conversation_messages(id,conversation_id,role,content,created_at)
                 VALUES('input',?1,'user','secret current request','1')",
                [crate::PRIMARY_CONVERSATION_ID],
            )
            .expect("input inserts");
        connection
            .execute(
                "INSERT INTO runtime_runs(id,conversation_id,route_kind,status,input_message_id,started_at)
                 VALUES('run',?1,'conversation.respond','running','input','1')",
                [crate::PRIMARY_CONVERSATION_ID],
            )
            .expect("run inserts");
        connection
    }

    fn bind_scope(connection: &Connection) {
        connection
            .execute(
                "INSERT INTO context_scopes(scope_key,kind,opaque_id,state,created_at)
                 VALUES('scope','project','opaque','active','1')",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO context_scope_epochs(scope_key,epoch) VALUES('scope',0)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO runtime_scope_resolutions(
                   run_id,status,focus_scope_key,scope_digest,resolved_at
                 ) VALUES('run','resolved','scope',?1,'1')",
                [digest(b"scope")],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO runtime_run_scopes(run_id,scope_key,relation,source,epoch)
                 VALUES('run','scope','current','runtime',0)",
                [],
            )
            .unwrap();
    }

    fn bind_personal_assertion(connection: &Connection) {
        connection
            .execute(
                "INSERT INTO personal_payloads(id,value_json,bytes)
                 VALUES('payload','{}',2)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO personal_assertions(id,metadata,payload_id,erased)
                 VALUES('assertion','{}','payload',0)",
                [],
            )
            .unwrap();
    }

    fn bind_selected_assertion(generation: &GenerationHandle) {
        generation
            .add_input(
                "personal-state",
                "assertion",
                1,
                &digest(b"assertion"),
                "must",
                "base",
                true,
                None,
            )
            .unwrap();
    }

    #[test]
    fn schema_records_only_digests_and_one_current_source() {
        let mut connection = database();
        let transaction = connection.transaction().unwrap();
        transaction
            .execute(
                "INSERT INTO context_generations(
                   id,run_id,provider_id,ordinal,purpose,envelope_digest,request_digest,
                   projected_bytes,current_instruction_count,health_status,status,started_at
                 ) VALUES('generation','run','provider',1,'reasoning',?1,?1,42,1,'green','planned','1')",
                [digest(b"request")],
            )
            .unwrap();
        transaction
            .execute(
                "INSERT INTO context_generation_inputs(
                   generation_id,source_kind,source_id,source_version,source_digest,
                   requirement,placement,selected,metadata_json
                 ) VALUES('generation','current-instruction','input',1,?1,'must','base',1,'{}')",
                [digest(b"secret current request")],
            )
            .unwrap();
        transaction.commit().unwrap();

        let stored: String = connection
            .query_row(
                "SELECT envelope_digest || request_digest FROM context_generations WHERE id='generation'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!stored.contains("secret current request"));
        assert_eq!(stored.len(), 128);
    }

    #[test]
    fn required_receipt_is_written_before_dispatch_without_context_text() {
        let state = crate::test_support::app_state(database());
        let generation = begin(
            &state,
            BeginGeneration {
                run_id: "run",
                provider_session_id: None,
                provider_id: Some("provider"),
                purpose: "reasoning",
                request_payload: b"serialized final provider body",
                envelope_payload: b"envelope",
                current_instruction_count: 1,
            },
        )
        .unwrap();
        let required = digest(b"ordered required candidate identities");
        generation.set_required_receipt(&required).unwrap();
        generation.dispatch().unwrap();
        state
            .sqlite_readers
            .read(|connection| {
                let receipt: (String, String) = connection
                    .query_row(
                        "SELECT required_set_digest,request_digest
                         FROM context_generations WHERE run_id='run'",
                        [],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .map_err(database_error)?;
                assert_eq!(
                    receipt,
                    (required, digest(b"serialized final provider body"))
                );
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn dropped_generation_handles_are_terminalized() {
        let state = crate::test_support::app_state(database());
        let generation = begin(
            &state,
            BeginGeneration {
                run_id: "run",
                provider_session_id: None,
                provider_id: Some("provider"),
                purpose: "reasoning",
                request_payload: b"request",
                envelope_payload: b"request",
                current_instruction_count: 1,
            },
        )
        .unwrap();
        generation.dispatch().unwrap();
        drop(generation);
        state
            .sqlite_readers
            .read(|connection| {
                let status: String = connection
                    .query_row(
                        "SELECT status FROM context_generations WHERE run_id='run'",
                        [],
                        |row| row.get(0),
                    )
                    .map_err(database_error)?;
                assert_eq!(status, "interrupted");
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn invalid_current_instruction_count_is_persisted_red_and_rejected() {
        let state = crate::test_support::app_state(database());
        let error = begin(
            &state,
            BeginGeneration {
                run_id: "run",
                provider_session_id: None,
                provider_id: Some("provider"),
                purpose: "reasoning",
                request_payload: b"request",
                envelope_payload: b"envelope",
                current_instruction_count: 0,
            },
        )
        .err()
        .expect("invalid envelope is rejected");
        assert!(error.contains("exactly one current instruction"));
        state
            .sqlite_readers
            .read(|connection| {
                let state: (String, String, u64) = connection
                    .query_row(
                        "SELECT health_status,status,current_instruction_count
                         FROM context_generations WHERE run_id='run'",
                        [],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                    )
                    .map_err(database_error)?;
                assert_eq!(state, ("red".into(), "failed".into(), 0));
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn red_health_cannot_be_dispatched() {
        let state = crate::test_support::app_state(database());
        let generation = begin(
            &state,
            BeginGeneration {
                run_id: "run",
                provider_session_id: None,
                provider_id: Some("provider"),
                purpose: "context-build",
                request_payload: b"request",
                envelope_payload: b"envelope",
                current_instruction_count: 1,
            },
        )
        .unwrap();
        generation.set_health("red").unwrap();
        assert!(generation.dispatch().is_err());
        drop(generation);
        state
            .sqlite_readers
            .read(|connection| {
                let state: (String, String) = connection
                    .query_row(
                        "SELECT health_status,status FROM context_generations WHERE run_id='run'",
                        [],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .map_err(database_error)?;
                assert_eq!(state, ("red".into(), "interrupted".into()));
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn manifest_records_selected_and_omitted_sources_without_content() {
        let state = crate::test_support::app_state(database());
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
        generation
            .add_input(
                "fixture-selected",
                "assertion-1",
                3,
                &digest(b"selected"),
                "should",
                "base",
                true,
                None,
            )
            .unwrap();
        generation
            .add_input(
                "personal-pending",
                "message-2",
                1,
                &digest(b"omitted"),
                "may",
                "base",
                false,
                Some("budget-or-policy"),
            )
            .unwrap();
        generation.dispatch().unwrap();
        generation.complete().unwrap();
        state
            .sqlite_readers
            .read(|connection| {
                let rows: Vec<(String, bool, Option<String>)> = connection
                    .prepare(
                        "SELECT source_id,selected,omission_reason
                         FROM context_generation_inputs
                         WHERE source_kind IN ('fixture-selected','personal-pending') ORDER BY source_id",
                    )
                    .map_err(database_error)?
                    .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
                    .map_err(database_error)?
                    .collect::<Result<_, _>>()
                    .map_err(database_error)?;
                assert_eq!(
                    rows,
                    vec![
                        ("assertion-1".into(), true, None),
                        ("message-2".into(), false, Some("budget-or-policy".into()))
                    ]
                );
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn conservative_provider_budget_is_persisted_red() {
        let state = crate::test_support::app_state(database());
        let oversized = vec![b'x'; 96_001];
        assert!(begin(
            &state,
            BeginGeneration {
                run_id: "run",
                provider_session_id: None,
                provider_id: Some("provider"),
                purpose: "reasoning",
                request_payload: &oversized,
                envelope_payload: b"envelope",
                current_instruction_count: 1,
            },
        )
        .is_err());
        state
            .sqlite_readers
            .read(|connection| {
                let failure: String = connection
                    .query_row(
                        "SELECT failure_kind FROM context_generations WHERE run_id='run'",
                        [],
                        |row| row.get(0),
                    )
                    .map_err(database_error)?;
                assert_eq!(failure, "provider-input-budget");
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn policy_revision_change_rejects_a_late_completion() {
        let state = crate::test_support::app_state(database());
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
                        "UPDATE personal_scope SET policy_revision=policy_revision+1
                         WHERE id='primary'",
                        [],
                    )
                    .map_err(database_error)?;
                Ok(())
            })
            .unwrap();
        assert!(generation.complete().is_err());
    }

    #[test]
    fn scope_change_before_dispatch_rejects_the_generation_cas() {
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
        assert!(generation.dispatch().is_err());
    }

    #[test]
    fn scope_change_after_dispatch_rejects_late_completion() {
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
    fn forgotten_assertion_before_dispatch_rejects_the_generation_cas() {
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
    fn forgotten_assertion_after_dispatch_rejects_late_completion() {
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
}
