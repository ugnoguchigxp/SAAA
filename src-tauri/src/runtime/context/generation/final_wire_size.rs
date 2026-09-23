use super::*;
pub(crate) const MAX_PROVIDER_REQUEST_BYTES: usize = 96_000;
/// The conservative input portion of the 96 KiB provider envelope after reserving 20 KiB for
/// output and 12 KiB for safety. This is measured on the final serialized wire body, not tokens.
pub(crate) const MAX_PROVIDER_CONTEXT_WIRE_BYTES: usize = 64_000;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FinalWireSize {
    Fits,
    RequiredContextOverflow,
    RequestTooLarge,
}
/// Classify the exact serialized provider body. Once the body contains a required item, crossing
/// the conservative input allotment is a safe dispatch refusal, never permission to omit it.
pub(crate) fn final_wire_size(payload_bytes: usize, has_required_context: bool) -> FinalWireSize {
    if payload_bytes > MAX_PROVIDER_CONTEXT_WIRE_BYTES && has_required_context {
        FinalWireSize::RequiredContextOverflow
    } else if payload_bytes > MAX_PROVIDER_REQUEST_BYTES {
        FinalWireSize::RequestTooLarge
    } else {
        FinalWireSize::Fits
    }
}
pub(crate) struct GenerationHandle {
    pub(super) writer: Arc<SqliteWriter>,
    pub(super) id: String,
    pub(super) world: Mutex<Option<super::super::world::turn::WorldReceipt>>,
    #[cfg(test)]
    pub(super) world_observation: Mutex<Option<&'static str>>,
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

    #[allow(dead_code)] // CW-45 reads the generation id from the handle.
    pub(crate) fn id(&self) -> &str {
        &self.id
    }

    pub(crate) fn writer_record(
        &self,
        model: Option<&str>,
        usage: &super::super::usage::ProviderUsage,
        source: super::super::usage::UsageSource,
        wire_bytes: usize,
        timings: &super::super::usage::UsageTimings,
        prefix_match_bytes: Option<i64>,
    ) -> Result<(), String> {
        self.writer.write(|connection| {
            let provider_id: String = connection
                .query_row(
                    "SELECT provider_id FROM context_generations WHERE id=?1",
                    [&self.id],
                    |row| row.get(0),
                )
                .map_err(database_error)?;
            super::super::usage::record(
                connection,
                &self.id,
                &provider_id,
                model,
                usage,
                source,
                wire_bytes,
                timings,
                prefix_match_bytes,
            )
        })
    }

    pub(crate) fn dispatch(&self) -> Result<(), String> {
        self.dispatch_checked(|_| Ok(()))
    }

    pub(crate) fn dispatch_checked(
        &self,
        check: impl FnOnce(&Connection) -> Result<(), String>,
    ) -> Result<(), String> {
        self.writer.write(|connection| {
            let transaction = connection
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                .map_err(database_error)?;
            check(&transaction)?;
            validate_dependencies(&transaction, &self.id)?;
            let changed = transaction
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
            transaction.commit().map_err(database_error)
        })
    }

    /// Re-check host-owned dependencies before starting an irreversible Tool action. A completed
    /// provider response is not authorization to act when a correction, forget, or withdrawal
    /// arrived while that response was in flight.
    pub(crate) fn revalidate_dependencies(&self) -> Result<(), String> {
        self.writer
            .write(|connection| validate_dependencies(connection, &self.id))
    }

    pub(crate) fn complete(&self) -> Result<(), String> {
        self.finish("completed", None)
    }

    pub(crate) fn complete_checked(
        &self,
        check: impl FnOnce(&Connection) -> Result<(), String>,
    ) -> Result<(), String> {
        self.finish_checked("completed", None, check)
    }

    pub(crate) fn fail(&self, failure_kind: &str) -> Result<(), String> {
        self.finish("failed", Some(failure_kind))
    }

    pub(crate) fn cancel(&self) -> Result<(), String> {
        self.finish("cancelled", Some("cancelled"))
    }

    fn finish(&self, status: &str, failure_kind: Option<&str>) -> Result<(), String> {
        self.finish_checked(status, failure_kind, |_| Ok(()))
    }

    fn finish_checked(
        &self,
        status: &str,
        failure_kind: Option<&str>,
        check: impl FnOnce(&Connection) -> Result<(), String>,
    ) -> Result<(), String> {
        let world_receipt = if status == "completed" {
            self.world
                .lock()
                .map_err(|_| "World receipt unavailable")?
                .as_ref()
                .filter(|r| r.service.sources_enabled())
                .cloned()
        } else {
            None
        };
        if let Some(receipt) = &world_receipt {
            if receipt.service.validate_result(&receipt.prepared)
                != Ok(saaa_personal_state_core::world::runtime_frame::FrameValidity::Current)
            {
                let _ = self.fail("world-source-changed");
                return Err("Context generation World source changed".into());
            }
        }
        self.writer.write(|connection| {
            let transaction = connection
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                .map_err(database_error)?;
            let connection = &transaction;
            check(connection)?;
            if let Some(receipt) = &world_receipt {
                receipt
                    .service
                    .validate_db_result(connection, &receipt.prepared)
                    .map_err(|e| e.code().to_string())?;
            }
            if status == "completed" {
                validate_dependencies(connection, &self.id)?;
            }
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
            transaction.commit().map_err(database_error)
        })?;
        self.observe_world();
        Ok(())
    }

    pub(crate) fn attach_world(&self, receipt: super::super::world::turn::WorldReceipt) {
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
            let outcome = super::super::world::turn::observe_receipt(&receipt);
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
    begin_with_writer(state.sqlite_writer.clone(), input)
}
pub(crate) fn begin_with_writer(
    writer: Arc<SqliteWriter>,
    input: BeginGeneration<'_>,
) -> Result<GenerationHandle, String> {
    let id = new_id("context_generation");
    let valid = writer.write(|connection| {
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
        writer,
        id,
        world: Mutex::new(None),
        #[cfg(test)]
        world_observation: Mutex::new(None),
    })
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
#[cfg(test)]
pub(crate) use super::test_helpers::assert_two_round_tool_manifest;
