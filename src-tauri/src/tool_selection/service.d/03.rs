impl ToolSelectionService {
pub fn describe(
        &self,
        context: &RequestContext,
        candidate_ref: &str,
        section: &str,
        cursor: Option<&str>,
    ) -> ToolSelectionResult<DescribeResponse> {
        if !matches!(
            section,
            "contract" | "usage" | "examples" | "troubleshooting"
        ) {
            return Err(ToolSelectionError::invalid());
        }
        let reference = self
            .references
            .resolve(candidate_ref, ReferenceKind::Candidate, now_ms())
            .ok_or_else(ToolSelectionError::not_found)?;
        if reference.principal_id != context.principal_id
            || reference.scope_key() != context.scope_key()
        {
            return Err(ToolSelectionError::unauthorized());
        }
        let (tool, revision) = self.current_revision(&reference)?;
        let (source_id, source_label) =
            super::mcp::service_support::source_display(&self.writer, &revision.tool_id);
        if section == "contract" {
            let body = json!({
                "revisionId": revision.id,
                "title": tool.backend_key,
                "inputSchema": revision.input_schema,
                "outputSchema": revision.output_schema,
            });
            if serde_json::to_vec(&body)
                .map(|bytes| bytes.len())
                .unwrap_or(usize::MAX)
                > DESCRIBE_RESPONSE_MAX_BYTES
            {
                // A contract that cannot be described is not runnable, so no execution ref is
                // issued.
                return Err(ToolSelectionError::new(
                    ToolSelectionErrorCode::Unavailable,
                    "The tool contract is too large to describe.",
                ));
            }
            let execution_ref = self.issue_execution_ref(context, &reference, &revision)?;
            return Ok(DescribeResponse {
                revision_id: revision.id,
                tool_id: revision.tool_id,
                source_id,
                source_label,
                section: section.to_string(),
                body,
                execution_ref: Some(execution_ref),
                cursor: None,
            });
        }
        let execution_ref = self.issue_execution_ref(context, &reference, &revision)?;
        let page = decode_cursor(cursor, &reference.revision_id, section)?;
        let text = self
            .writer
            .read_serialized(|connection| {
                repository::usage_page(connection, &reference.revision_id, section, page)
                    .map_err(|error| error.to_string())
            })
            .map_err(|_| ToolSelectionError::storage())?
            .ok_or_else(ToolSelectionError::not_found)?;
        let has_next = self
            .writer
            .read_serialized(|connection| {
                repository::usage_page(connection, &reference.revision_id, section, page + 1)
                    .map(|value| value.is_some())
                    .map_err(|error| error.to_string())
            })
            .map_err(|_| ToolSelectionError::storage())?;
        let body = json!({ "section": section, "page": page, "text": text });
        Ok(DescribeResponse {
            revision_id: revision.id,
            tool_id: revision.tool_id,
            source_id,
            source_label,
            section: section.to_string(),
            body,
            execution_ref: Some(execution_ref),
            cursor: has_next.then(|| encode_cursor(&reference.revision_id, section, page + 1)),
        })
    }
}
impl ToolSelectionService {
fn issue_execution_ref(
        &self,
        context: &RequestContext,
        candidate: &ReferenceEntry,
        revision: &repository::RevisionRow,
    ) -> ToolSelectionResult<String> {
        let tool = self
            .writer
            .read_serialized(|connection| {
                repository::tool_by_id(connection, &revision.tool_id)
                    .map_err(|error| error.to_string())
            })
            .map_err(|_| ToolSelectionError::storage())?
            .ok_or_else(ToolSelectionError::not_found)?;
        let entry = ReferenceEntry {
            kind: ReferenceKind::Execution,
            decision_id: candidate.decision_id.clone(),
            revision_id: revision.id.clone(),
            tool_id: tool.id.clone(),
            schema_hash: revision.schema_hash.clone(),
            principal_id: context.principal_id.clone(),
            conversation_id: context.conversation_id.clone(),
            run_id: context.run_id.clone(),
            project_id: context.project_id.clone(),
            task_id: context.task_id.clone(),
            catalog_epoch: candidate.catalog_epoch,
            acl_epoch: candidate.acl_epoch,
            rule_epoch: candidate.rule_epoch,
            input_schema: revision.input_schema.clone(),
            created_at_ms: now_ms(),
        };
        self.references.issue(entry, now_ms())
    }
}
impl ToolSelectionService {
fn current_revision(
        &self,
        reference: &ReferenceEntry,
    ) -> ToolSelectionResult<(repository::ToolRow, repository::RevisionRow)> {
        let principal = reference.principal_id.clone();
        let project = reference.project_id.clone();
        let tool_id = reference.tool_id.clone();
        let revision_id = reference.revision_id.clone();
        let now = now_ms();
        let result = self
            .writer
            .read_serialized(move |connection| {
                let tool = repository::tool_by_id(connection, &tool_id)
                    .map_err(|error| error.to_string())?
                    .ok_or_else(|| "not-found".to_string())?;
                if !tool.enabled
                    || tool.current_revision_id.as_deref() != Some(revision_id.as_str())
                {
                    return Err("stale".to_string());
                }
                // A residual reference must be refused when the source is disabled or stale,
                // even if no catalog epoch has moved since the reference was issued.
                if !super::source_lookup::source_eligible(connection, &tool.source_id, now)
                    .map_err(|error| error.to_string())?
                {
                    return Err("stale".to_string());
                }
                let authorized =
                    repository::grant_exists(connection, &principal, &tool_id, project.as_deref())
                        .map_err(|error| error.to_string())?;
                if !authorized {
                    return Err("unauthorized".to_string());
                }
                let revision = repository::revision_by_id(connection, &revision_id)
                    .map_err(|error| error.to_string())?
                    .ok_or_else(|| "not-found".to_string())?;
                Ok((tool, revision))
            })
            .map_err(|error| match error.as_str() {
                "stale" => ToolSelectionError::stale(),
                "unauthorized" => ToolSelectionError::unauthorized(),
                "not-found" => ToolSelectionError::not_found(),
                _ => ToolSelectionError::storage(),
            })?;
        Ok(result)
    }
}
impl ToolSelectionService {
/// Resolves the trusted effect for an execution reference without opening an invocation.
    /// Role routing uses this immediately before its own atomic reservation; the later invoke
    /// repeats all reference/epoch checks before the backend owner is started.
    pub(crate) fn execution_effect(
        &self,
        context: &RequestContext,
        execution_ref: &str,
    ) -> ToolSelectionResult<String> {
        let reference = self
            .references
            .resolve(execution_ref, ReferenceKind::Execution, now_ms())
            .ok_or_else(ToolSelectionError::not_found)?;
        if reference.principal_id != context.principal_id
            || reference.scope_key() != context.scope_key()
        {
            return Err(ToolSelectionError::unauthorized());
        }
        let (_, revision) = self.current_revision(&reference)?;
        let current_epochs = self.read_epochs()?;
        if current_epochs.rule != reference.rule_epoch {
            return Err(ToolSelectionError::changed());
        }
        if current_epochs.catalog != reference.catalog_epoch
            || current_epochs.acl != reference.acl_epoch
            || revision.schema_hash != reference.schema_hash
        {
            return Err(ToolSelectionError::stale());
        }
        Ok(revision.effect)
    }
}
impl ToolSelectionService {
pub async fn invoke(
        &self,
        context: &RequestContext,
        execution_ref: &str,
        arguments: &Value,
        run_cancellation: &RunCancellation,
    ) -> ToolSelectionResult<InvokeResponse> {
        self.invoke_with_origin(
            context,
            execution_ref,
            arguments,
            run_cancellation,
            "conversation",
        )
        .await
    }
}
impl ToolSelectionService {
/// Same as [`Self::invoke`] but records the origin that reaches the generated-capability call
    /// row (`conversation` or `mcp`).
    pub async fn invoke_with_origin(
        &self,
        context: &RequestContext,
        execution_ref: &str,
        arguments: &Value,
        run_cancellation: &RunCancellation,
        origin: &'static str,
    ) -> ToolSelectionResult<InvokeResponse> {
        let reference = self
            .references
            .resolve(execution_ref, ReferenceKind::Execution, now_ms())
            .ok_or_else(ToolSelectionError::not_found)?;
        if reference.principal_id != context.principal_id
            || reference.scope_key() != context.scope_key()
        {
            return Err(ToolSelectionError::unauthorized());
        }
        if !arguments.is_object() {
            return Err(ToolSelectionError::invalid());
        }
        let (tool, revision) = self.current_revision(&reference)?;
        let current_epochs = self.read_epochs()?;
        if current_epochs.rule != reference.rule_epoch {
            return Err(ToolSelectionError::changed());
        }
        if current_epochs.catalog != reference.catalog_epoch
            || current_epochs.acl != reference.acl_epoch
        {
            return Err(ToolSelectionError::stale());
        }
        if revision.schema_hash != reference.schema_hash {
            return Err(ToolSelectionError::stale());
        }
        validate_arguments(&revision.input_schema, arguments)?;
        let binding = revision.backend_binding.clone();
        let binding_kind = BackendRouter::kind(&binding);
        // For an external MCP binding the source must still be enabled, freshly synced and
        // matched to the recorded endpoint before an invocation row is even opened.
        if binding_kind == "mcp_http" {
            let parsed = McpBinding::parse(&binding).ok_or_else(ToolSelectionError::stale)?;
            let manager = self
                .mcp_manager
                .clone()
                .ok_or_else(ToolSelectionError::unavailable)?;
            manager
                .preflight(&parsed.source_id, &parsed.endpoint_hash)
                .await
                .map_err(super::mcp::service_support::map_call_error)?;
        }
        let invocation_id = crate::new_id("tsinv");
        let decision_id = reference.decision_id.clone();
        let now = now_ms();
        {
            let invocation_id = invocation_id.clone();
            let revision_id = revision.id.clone();
            write_transaction(&self.writer, move |connection| {
                repository::insert_invocation(
                    connection,
                    &invocation_id,
                    Some(&decision_id),
                    &revision_id,
                    now,
                )
                .map_err(|_| "storage".to_string())
            })
            .map_err(|_| ToolSelectionError::storage())?;
        }

        let request = BackendRequest {
            call_id: invocation_id.clone(),
            tool_id: tool.id.clone(),
            revision_id: revision.id.clone(),
            backend_key: tool.backend_key.clone(),
            binding,
            arguments: arguments.clone(),
            timeout: std::time::Duration::from_millis(BACKEND_TIMEOUT_MS),
            origin,
            actor: Some(crate::generated_capabilities::contracts::CallActor {
                principal_id: context.principal_id.clone(),
                conversation_id: context.conversation_id.clone(),
                project_id: context.project_id.clone(),
                run_id: context
                    .run_id
                    .clone()
                    .unwrap_or_else(|| format!("conversation:{}", context.conversation_id)),
            }),
        };
        // The management task owns the backend call, the cancellation handle, the result storage
        // and the terminal DB write. Dropping this caller future (HTTP disconnect, aborted
        // provider turn) detaches it but does not stop or leak the invocation.
        let receiver = super::invocation::spawn(super::invocation::ManagedInvocation {
            writer: self.writer.clone(),
            backend: self.backend.clone(),
            invocation_id: invocation_id.clone(),
            context: context.clone(),
            tool_id: tool.id.clone(),
            revision: revision.clone(),
            acl_epoch: current_epochs.acl,
            binding_kind: binding_kind.to_string(),
            request,
            cancellation: run_cancellation.clone(),
        });
        match receiver.await {
            Ok(response) => response,
            // The management task panicked before reporting; the row is settled by the
            // crash-reconcile path at the next startup.
            Err(_) => Err(ToolSelectionError::unavailable()),
        }
    }
}
impl ToolSelectionService {
/// Resolves one continuation page of a stored MCP result. Ownership, scope, TTL and the
    /// current ACL are re-checked on every read; a revoked or foreign reference is refused.
    pub fn describe_result(
        &self,
        context: &RequestContext,
        result_ref: &str,
        page: i64,
    ) -> ToolSelectionResult<ResultPageResponse> {
        super::mcp::service_support::describe_result(&self.writer, context, result_ref, page)
    }
}
impl ToolSelectionService {
/// Development/management API for importing a fixture catalog. Import is never a grant, but
    /// the evaluation fixture grants each tool to the evaluation principal explicitly.
    pub fn ingest_catalog(
        &self,
        principal_id: &str,
        source_id: &str,
        entries: &[CatalogEntry],
    ) -> ToolSelectionResult<usize> {
        let now = now_ms();
        write_transaction(&self.writer, move |connection| {
            for entry in entries {
                let revision_id = format!("{}-rev1", entry.tool_id);
                catalog::register_revision(
                    connection,
                    principal_id,
                    source_id,
                    entry,
                    &revision_id,
                    now,
                )
                .map_err(|error| error.code.as_str().to_string())?;
                repository::upsert_grant(
                    connection,
                    principal_id,
                    &entry.tool_id,
                    "user",
                    principal_id,
                )
                .map_err(|_| "storage".to_string())?;
            }
            repository::bump_epochs(connection, false, true, false)
                .map_err(|_| "storage".to_string())?;
            Ok(entries.len())
        })
        .map_err(|_| ToolSelectionError::storage())
    }
}
impl ToolSelectionService {
/// Computes and stores document embeddings for every authorized revision using the configured
    /// embedding provider.
    pub async fn index_embeddings(
        &self,
        principal_id: &str,
        project_id: Option<&str>,
    ) -> ToolSelectionResult<usize> {
        let eligible = self
            .writer
            .read_serialized({
                let principal = principal_id.to_string();
                let project = project_id.map(str::to_string);
                let now = now_ms();
                move |connection| {
                    repository::eligible_revisions(connection, &principal, project.as_deref(), now)
                        .map_err(|error| error.to_string())
                }
            })
            .map_err(|_| ToolSelectionError::storage())?;
        if eligible.is_empty() {
            return Ok(0);
        }
        let texts: Vec<String> = eligible
            .iter()
            .map(|item| retrieval::document_text(&item.revision.search_text))
            .collect();
        // Batch requests so one JSONL response stays well under the 2 MiB line limit.
        let mut vectors: Vec<Vec<f32>> = Vec::with_capacity(texts.len());
        for chunk in texts.chunks(EMBED_BATCH_SIZE) {
            let batch = self
                .embedding
                .embed(EmbedKind::Passage, chunk)
                .await
                .map_err(|_| ToolSelectionError::unavailable())?;
            if batch.len() != chunk.len()
                || batch
                    .iter()
                    .any(|vector| vector.len() != self.embedding.dimension())
            {
                return Err(ToolSelectionError::new(
                    ToolSelectionErrorCode::Integrity,
                    "The embedding model returned an unexpected shape.",
                ));
            }
            vectors.extend(batch);
        }
        let model_hash = self.embedding.model_hash().to_string();
        let rows: Vec<(String, Vec<f32>)> = eligible
            .iter()
            .map(|item| item.revision.id.clone())
            .zip(vectors)
            .collect();
        write_transaction(&self.writer, move |connection| {
            for (revision_id, vector) in &rows {
                repository::upsert_embedding(connection, revision_id, &model_hash, vector)
                    .map_err(|_| "storage".to_string())?;
            }
            Ok(rows.len())
        })
        .map_err(|_| ToolSelectionError::storage())
    }
}
impl ToolSelectionService {
/// Full stored candidate ranking for a decision, used by the evaluation CLI. The returned
    /// order is the persisted final order before the response limit is applied.
    pub fn decision_candidates(
        &self,
        decision_id: &str,
    ) -> ToolSelectionResult<Vec<CandidateRecord>> {
        self.writer
            .read_serialized(|connection| {
                repository::decision_by_id(connection, decision_id)
                    .map(|decision| decision.map(|value| value.candidates).unwrap_or_default())
                    .map_err(|error| error.to_string())
            })
            .map_err(|_| ToolSelectionError::storage())
    }
}
impl ToolSelectionService {
fn read_epochs(&self) -> ToolSelectionResult<Epochs> {
        self.writer
            .read_serialized(|connection| {
                repository::epochs(connection).map_err(|error| error.to_string())
            })
            .map_err(|_| ToolSelectionError::storage())
    }
}
/// Returns the local profile UUID used as the tool-selection principal. It is created once in the
/// `tool_selection` settings namespace and never derived from credentials or the OS user name.
pub fn ensure_principal(writer: &SqliteWriter) -> ToolSelectionResult<String> {
    writer
        .write(|connection| {
            let existing: Option<String> = connection
                .query_row(
                    "SELECT value_json FROM settings_documents
                      WHERE namespace = 'tool_selection' AND key = 'principal'",
                    [],
                    |row| row.get(0),
                )
                .optional()
                .map_err(crate::database_error)?;
            if let Some(value) = existing {
                if let Ok(serde_json::Value::String(id)) = serde_json::from_str(&value) {
                    if !id.is_empty() {
                        return Ok(id);
                    }
                }
            }
            let id = uuid::Uuid::new_v4().to_string();
            connection
                .execute(
                    "INSERT OR REPLACE INTO settings_documents(
                       namespace, key, schema_version, value_json, updated_at)
                     VALUES ('tool_selection', 'principal', 1, ?1, ?2)",
                    rusqlite::params![
                        serde_json::Value::String(id.clone()).to_string(),
                        crate::now_iso()
                    ],
                )
                .map_err(crate::database_error)?;
            Ok(id)
        })
        .map_err(|_| ToolSelectionError::storage())
}
const CHANGED: &str = "selection-changed";
enum PersistError {
    Storage,
    Changed,
}
