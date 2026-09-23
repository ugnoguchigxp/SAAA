use super::*;
impl McpManager {
    /// Applies the config-derived grants for one source in its own transaction. Unknown tool
    /// names stay pending until a later sync publishes them.
    pub(super) async fn apply_config_grants(&self, source_id: &str, generation: i64) {
        let Some(spec) = self.source_spec(source_id).await else {
            return;
        };
        let desired = self.desired_grants(source_id, &spec.grants).await;
        let writer = self.writer.clone();
        let principal = self.principal_id.clone();
        let source = source_id.to_string();
        let _ = generation;
        let _ = writer.write(move |connection| {
            let transaction = connection
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                .map_err(crate::database_error)?;
            let existing = mcp_repository::managed_grants_for_source(&transaction, &source)
                .map_err(|_| "storage".to_string())?;
            let desired_set: HashSet<(String, String, String, String)> = desired
                .iter()
                .map(|grant| {
                    (
                        grant.principal_id.clone(),
                        grant.tool_id.clone(),
                        grant.scope_kind.clone(),
                        grant.scope_id.clone(),
                    )
                })
                .collect();
            let mut acl_changed = false;
            for grant in &existing {
                let key = (
                    grant.principal_id.clone(),
                    grant.tool_id.clone(),
                    grant.scope_kind.clone(),
                    grant.scope_id.clone(),
                );
                if !desired_set.contains(&key)
                    && mcp_repository::delete_managed_grant(&transaction, grant)
                        .map_err(|_| "storage".to_string())?
                {
                    acl_changed = true;
                }
            }
            for grant in &desired {
                let already_managed = mcp_repository::managed_grant_exists(
                    &transaction,
                    &grant.source_id,
                    &grant.principal_id,
                    &grant.tool_id,
                    &grant.scope_kind,
                    &grant.scope_id,
                )
                .map_err(|_| "storage".to_string())?;
                if already_managed {
                    continue;
                }
                // A grant that already exists but is not config-owned is a manual grant. Never
                // adopt it, so removing the source cannot revoke a grant the host created by
                // another management path.
                let manually_granted = super::super::super::source_lookup::exact_grant_exists(
                    &transaction,
                    &grant.principal_id,
                    &grant.tool_id,
                    &grant.scope_kind,
                    &grant.scope_id,
                )
                .map_err(|_| "storage".to_string())?;
                if manually_granted {
                    continue;
                }
                repository::upsert_grant(
                    &transaction,
                    &grant.principal_id,
                    &grant.tool_id,
                    &grant.scope_kind,
                    &grant.scope_id,
                )
                .map_err(|_| "storage".to_string())?;
                mcp_repository::insert_managed_grant(&transaction, grant)
                    .map_err(|_| "storage".to_string())?;
                acl_changed = true;
            }
            if acl_changed {
                repository::bump_epochs(&transaction, false, true, false)
                    .map_err(|_| "storage".to_string())?;
            }
            let _ = &principal;
            transaction.commit().map_err(crate::database_error)?;
            Ok(())
        });
    }
}
impl McpManager {
    pub(super) async fn desired_grants(
        &self,
        source_id: &str,
        grants: &[super::super::config::McpGrantSpec],
    ) -> Vec<mcp_repository::ManagedGrant> {
        if grants.is_empty() {
            return Vec::new();
        }
        let principal = self.principal_id.clone();
        let entries: Vec<(String, McpGrantScope, Option<String>)> = grants
            .iter()
            .map(|grant| {
                (
                    grant.tool_name.clone(),
                    grant.scope_kind,
                    grant.project_id.clone(),
                )
            })
            .collect();
        let source = source_id.to_string();
        let tool_ids: Vec<(String, McpGrantScope, Option<String>)> = self
            .writer
            .read_serialized(move |connection| {
                let mut resolved = Vec::new();
                for (tool_name, scope, project_id) in &entries {
                    let tool_id = descriptors::tool_id(&source, tool_name);
                    let exists = repository::tool_by_id(connection, &tool_id)
                        .map_err(|error| error.to_string())?
                        .is_some();
                    if !exists {
                        continue;
                    }
                    resolved.push((tool_id, *scope, project_id.clone()));
                }
                Ok(resolved)
            })
            .unwrap_or_default();
        let mut desired = Vec::new();
        for (tool_id, scope, project_id) in tool_ids {
            let scope_id = match scope {
                McpGrantScope::User => principal.clone(),
                McpGrantScope::Project => {
                    let Some(project_id) = project_id else {
                        continue;
                    };
                    if !self.project_exists(&project_id) {
                        continue;
                    }
                    project_id
                }
            };
            desired.push(mcp_repository::ManagedGrant {
                source_id: source_id.to_string(),
                principal_id: principal.clone(),
                tool_id,
                scope_kind: scope.as_str().to_string(),
                scope_id,
            });
        }
        desired
    }
}
impl McpManager {
    pub(super) fn project_exists(&self, project_id: &str) -> bool {
        let project_id = project_id.to_string();
        self.writer
            .read_serialized(move |connection| {
                connection
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM coding_workspaces WHERE id = ?1)",
                        rusqlite::params![project_id],
                        |row| row.get::<_, i64>(0),
                    )
                    .map(|value| value == 1)
                    .map_err(|error| error.to_string())
            })
            .unwrap_or(false)
    }
}
impl McpManager {
    /// Embeds only revisions that do not yet have a vector for the configured model. Old revision
    /// vectors are never moved to a new revision.
    pub(super) async fn index_missing_embeddings(&self) -> Result<usize, ()> {
        let Some(embedder) = self.embedder.clone() else {
            return Ok(0);
        };
        if embedder.dimension() == 0 {
            return Ok(0);
        }
        let model_hash = embedder.model_hash().to_string();
        let rows: Vec<(String, String)> = self
            .writer
            .read_serialized({
                let model_hash = model_hash.clone();
                move |connection| {
                    let mut statement = connection
                        .prepare(
                            "SELECT r.id, r.search_text
                               FROM tool_selection_revisions r
                               JOIN tool_selection_catalog c ON c.id = r.tool_id
                              WHERE c.current_revision_id = r.id AND c.enabled = 1
                                AND NOT EXISTS (
                                  SELECT 1 FROM tool_selection_embeddings e
                                   WHERE e.revision_id = r.id AND e.model_hash = ?1)",
                        )
                        .map_err(|error| error.to_string())?;
                    let rows = statement
                        .query_map(rusqlite::params![model_hash], |row| {
                            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                        })
                        .map_err(|error| error.to_string())?;
                    rows.collect::<Result<Vec<_>, _>>()
                        .map_err(|error| error.to_string())
                }
            })
            .map_err(|_| ())?;
        if rows.is_empty() {
            return Ok(0);
        }
        let mut indexed = 0;
        for chunk in rows.chunks(64) {
            let texts: Vec<String> = chunk
                .iter()
                .map(|(_, text)| {
                    super::super::super::repository::truncate_utf8(
                        text,
                        super::super::super::contracts::SEARCH_TEXT_MAX_BYTES,
                    )
                    .to_string()
                })
                .collect();
            let Ok(vectors) = embedder.embed(EmbedKind::Passage, &texts).await else {
                // Degrade to BM25 and retry the missing rows on the next sync.
                return Ok(indexed);
            };
            if vectors.len() != chunk.len() {
                return Ok(indexed);
            }
            let pairs: Vec<(String, Vec<f32>)> = chunk
                .iter()
                .map(|(id, _)| id.clone())
                .zip(vectors)
                .collect();
            let pair_count = pairs.len();
            let model_hash = model_hash.clone();
            let _ = self.writer.write(move |connection| {
                for (revision_id, vector) in &pairs {
                    repository::upsert_embedding(connection, revision_id, &model_hash, vector)
                        .map_err(|error| error.to_string())?;
                }
                Ok(())
            });
            indexed += pair_count;
        }
        Ok(indexed)
    }
}
impl McpManager {
    /// Pre-flight check used by `invoke` before an invocation row is opened. It performs exactly
    /// the same source gate as `invoke` minus the send.
    pub async fn preflight(&self, source_id: &str, endpoint_hash: &str) -> Result<(), CallError> {
        if self.shutting_down.load(Ordering::SeqCst) {
            return Err(CallError::Closed);
        }
        let gate = self.gate_for(source_id).await;
        let _guard = gate.read().await;
        self.check_locked(source_id, endpoint_hash)
            .await
            .map(|_| ())
    }
}
impl McpManager {
    pub(super) async fn check_locked(
        &self,
        source_id: &str,
        endpoint_hash: &str,
    ) -> Result<super::super::config::McpSourceSpec, CallError> {
        if self.stopped.read().await.contains(source_id) {
            return Err(CallError::Closed);
        }
        let Some(spec) = self.source_spec(source_id).await else {
            return Err(CallError::Closed);
        };
        if !spec.enabled || spec.endpoint_hash() != endpoint_hash {
            return Err(CallError::Unavailable("source-changed"));
        }
        if self.is_self_endpoint(&spec.url).await {
            return Err(CallError::Unavailable("source-self-reference"));
        }
        if !self.ready.read().await.contains(source_id) {
            return Err(CallError::Unavailable("source-not-synced"));
        }
        let source = source_id.to_string();
        let fresh = self
            .writer
            .read_serialized(move |connection| {
                mcp_repository::is_fresh(connection, &source, now_ms())
                    .map_err(|error| error.to_string())
            })
            .unwrap_or(false);
        if !fresh {
            return Err(CallError::Unavailable("source-stale"));
        }
        Ok(spec)
    }
}
impl McpManager {
    /// Checks whether a call may be sent for this binding. Config stop always wins if it happened
    /// first; already-sent requests are never claimed to be reversible.
    #[allow(clippy::too_many_arguments)]
    pub async fn invoke(
        &self,
        source_id: &str,
        endpoint_hash: &str,
        tool_name: &str,
        arguments: Value,
        timeout: Duration,
        cancellation: &crate::RunCancellation,
    ) -> Result<Value, CallError> {
        if self.shutting_down.load(Ordering::SeqCst) {
            return Err(CallError::Closed);
        }
        let gate = self.gate_for(source_id).await;
        let _guard = gate.read().await;
        let spec = self.check_locked(source_id, endpoint_hash).await?;
        self.pool
            .call(
                &spec,
                self.config_generation(),
                tool_name,
                arguments,
                timeout,
                cancellation,
            )
            .await
    }
}
impl McpManager {
    /// Refresh freshness for search eligibility without holding the writer lock during inference.
    pub fn source_fresh(&self, source_id: &str) -> bool {
        let source_id = source_id.to_string();
        self.writer
            .read_serialized(move |connection| {
                mcp_repository::is_fresh(connection, &source_id, now_ms())
                    .map_err(|error| error.to_string())
            })
            .unwrap_or(false)
    }
}
impl McpManager {
    /// Diagnostics for management callers. Never contains a token, URL, session id or raw body.
    pub async fn status(&self) -> Value {
        let sources = self.config.read().await.clone();
        let ready = self.ready.read().await.clone();
        let stopped = self.stopped.read().await.clone();
        let diagnostics = self.source_diagnostics.read().await.clone();
        let generation = self.config_generation();
        let mut rows = Vec::new();
        for spec in &sources.sources {
            let id = spec.id.clone();
            let (last_success_at, last_error_code, published_generation) = self
                .writer
                .read_serialized({
                    let id = id.clone();
                    move |connection| {
                        mcp_repository::source(connection, &id)
                            .map(|row| {
                                row.map(|row| {
                                    (
                                        row.last_success_at,
                                        row.last_error_code,
                                        row.published_generation,
                                    )
                                })
                                .unwrap_or((None, None, 0))
                            })
                            .map_err(|error| error.to_string())
                    }
                })
                .unwrap_or((None, None, 0));
            let fresh = last_success_at
                .map(|at| now_ms() - at <= MCP_SOURCE_STALE_AFTER_MILLIS)
                .unwrap_or(false);
            rows.push(json!({
                "sourceId": spec.id,
                "enabled": spec.enabled,
                "state": if stopped.contains(&spec.id) {
                    "stopped"
                } else if ready.contains(&spec.id) {
                    "ready"
                } else if let Some(code) = diagnostics.get(&spec.id) {
                    code
                } else {
                    "pending"
                },
                "fresh": fresh,
                "lastSuccessAt": last_success_at,
                "lastErrorCode": last_error_code,
                "configGeneration": generation,
                "publishedGeneration": published_generation,
                "endpointHash": spec.endpoint_hash(),
            }));
        }
        json!({
            "configGeneration": generation,
            "configError": self.config_diagnostic().await,
            "sources": rows,
            "sessionState": "managed",
        })
    }
}
impl McpManager {
    /// Stops new dispatch, marks every in-flight call indeterminate, and closes sessions.
    pub async fn shutdown(&self) {
        if self.shutting_down.swap(true, Ordering::SeqCst) {
            return;
        }
        {
            let mut stopped = self.stopped.write().await;
            let sources = self.config.read().await.clone();
            for source in &sources.sources {
                stopped.insert(source.id.clone());
            }
        }
        {
            let mut watchers = self.watchers.lock().await;
            for (_, handle) in watchers.drain() {
                handle.abort();
            }
        }
        self.pool.shutdown().await;
        let _ = self.writer.write(|connection| {
            mcp_repository::purge_expired_results(connection, now_ms())
                .map_err(|error| error.to_string())?;
            Ok(())
        });
    }
}
impl McpManager {
    pub fn is_shutting_down(&self) -> bool {
        self.shutting_down.load(Ordering::SeqCst)
    }
}
impl McpManager {
    /// Session state for diagnostics; the manager owns the pool.
    pub async fn session_state(&self, source_id: &str) -> Option<SessionState> {
        let spec = self.source_spec(source_id).await?;
        let session = self
            .pool
            .session_for(&spec, self.config_generation())
            .await
            .ok()?;
        Some(session.state().await)
    }
}
/// Canonical `loopback:{port}{path}` for a loopback HTTP URL. `localhost`, `127.0.0.1` and `[::1]`
/// are treated as the same host, so a D4 source cannot reach the gateway's own listener by
/// respelling the address.
pub(crate) fn normalize_loopback(raw: &str) -> Option<String> {
    let parsed = url::Url::parse(raw).ok()?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return None;
    }
    let host = match parsed.host()? {
        url::Host::Ipv4(address) if address.is_loopback() => "loopback",
        url::Host::Ipv6(address) if address.is_loopback() => "loopback",
        url::Host::Domain(domain) if domain.eq_ignore_ascii_case("localhost") => "loopback",
        _ => return None,
    };
    let port = parsed.port_or_known_default()?;
    // Treat a trailing slash as the same endpoint, so `.../mcp/` cannot slip past the guard.
    let mut path = parsed.path().to_string();
    if path.len() > 1 && path.ends_with('/') {
        path.pop();
    }
    Some(format!("{host}:{port}{path}"))
}
