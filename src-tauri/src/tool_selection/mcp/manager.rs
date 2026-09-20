//! External MCP source lifecycle: configuration reload, atomic sync, config-derived grants,
//! embedding backfill, freshness, notification-driven polling and shutdown.
//!
//! The manager is the only place that knows the host-managed source list. The LLM never sees a
//! source id, URL, token or grant, and no management tool is exposed to the model.

use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, Notify, RwLock};

use super::super::contracts::now_ms;
use super::super::inference::{EmbedKind, EmbeddingProvider};
use super::super::repository;
use super::config::{McpGrantScope, McpSources};
use super::descriptors;
use super::repository as mcp_repository;
use super::session::{CallError, McpSessionPool, SessionState};
use super::sync::{self, SyncError, SyncOutcome};
use super::{MCP_NOTIFICATION_DEBOUNCE, MCP_SOURCE_POLL_INTERVAL, MCP_SOURCE_STALE_AFTER_MILLIS};
use crate::persistence::SqliteWriter;

pub struct McpManager {
    writer: Arc<SqliteWriter>,
    principal_id: String,
    config_path: Option<PathBuf>,
    config: RwLock<Arc<McpSources>>,
    generation: AtomicI64,
    pool: Arc<McpSessionPool>,
    embedder: Option<Arc<dyn EmbeddingProvider>>,
    syncing: Mutex<HashSet<String>>,
    ready: RwLock<HashSet<String>>,
    stopped: RwLock<HashSet<String>>,
    gates: Mutex<HashMap<String, Arc<RwLock<()>>>>,
    dirty: Arc<Mutex<HashSet<String>>>,
    dirty_notify: Arc<Notify>,
    watchers: Mutex<HashMap<String, tokio::task::JoinHandle<()>>>,
    source_diagnostics: RwLock<HashMap<String, &'static str>>,
    config_diagnostic: RwLock<Option<&'static str>>,
    shutting_down: AtomicBool,
}

impl McpManager {
    pub fn new(
        writer: Arc<SqliteWriter>,
        principal_id: String,
        config_path: Option<PathBuf>,
        sources: McpSources,
        embedder: Option<Arc<dyn EmbeddingProvider>>,
        diagnostic: Option<&'static str>,
    ) -> Arc<Self> {
        Arc::new(Self {
            writer,
            principal_id,
            config_path,
            config: RwLock::new(Arc::new(sources)),
            generation: AtomicI64::new(1),
            pool: Arc::new(McpSessionPool::new()),
            embedder,
            syncing: Mutex::new(HashSet::new()),
            ready: RwLock::new(HashSet::new()),
            stopped: RwLock::new(HashSet::new()),
            gates: Mutex::new(HashMap::new()),
            dirty: Arc::new(Mutex::new(HashSet::new())),
            dirty_notify: Arc::new(Notify::new()),
            watchers: Mutex::new(HashMap::new()),
            source_diagnostics: RwLock::new(HashMap::new()),
            config_diagnostic: RwLock::new(diagnostic),
            shutting_down: AtomicBool::new(false),
        })
    }

    pub fn principal_id(&self) -> &str {
        &self.principal_id
    }

    pub fn config_generation(&self) -> i64 {
        self.generation.load(Ordering::SeqCst)
    }

    pub async fn sources(&self) -> Arc<McpSources> {
        self.config.read().await.clone()
    }

    pub async fn source_spec(&self, source_id: &str) -> Option<super::config::McpSourceSpec> {
        self.config.read().await.get(source_id).cloned()
    }

    pub async fn is_ready(&self, source_id: &str) -> bool {
        self.ready.read().await.contains(source_id)
    }

    pub async fn config_diagnostic(&self) -> Option<&'static str> {
        *self.config_diagnostic.read().await
    }

    async fn gate_for(&self, source_id: &str) -> Arc<RwLock<()>> {
        let mut gates = self.gates.lock().await;
        gates
            .entry(source_id.to_string())
            .or_insert_with(|| Arc::new(RwLock::new(())))
            .clone()
    }

    /// Registers the configured sources in the ledger (without creating grants), revokes config
    /// grants for removed or disabled sources, and keeps the previous valid configuration when a
    /// reload fails.
    pub async fn reload_config(&self) -> Result<(), &'static str> {
        let Some(path) = self.config_path.clone() else {
            return Err("mcp-sources-not-configured");
        };
        match McpSources::load(&path) {
            Ok(sources) => {
                self.apply_sources(sources).await;
                *self.config_diagnostic.write().await = None;
                Ok(())
            }
            Err(diagnostic) => {
                // Keep the previous valid configuration and expose only the diagnostic code.
                *self.config_diagnostic.write().await = Some(diagnostic);
                Err(diagnostic)
            }
        }
    }

    /// Applies a new source list. Disabled or removed sources stop dispatch, are disabled in the
    /// ledger with a catalog epoch bump, and lose only the grants this configuration owned.
    pub async fn apply_sources(&self, sources: McpSources) {
        let previous: HashSet<String> = self
            .config
            .read()
            .await
            .sources
            .iter()
            .filter(|source| source.enabled)
            .map(|source| source.id.clone())
            .collect();
        let next: HashSet<String> = sources
            .sources
            .iter()
            .filter(|source| source.enabled)
            .map(|source| source.id.clone())
            .collect();
        let removed: Vec<String> = previous.difference(&next).cloned().collect();
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        {
            let mut config = self.config.write().await;
            *config = Arc::new(sources.clone());
        }
        {
            let mut stopped = self.stopped.write().await;
            for id in &removed {
                stopped.insert(id.clone());
            }
            for id in &next {
                stopped.remove(id);
            }
        }
        {
            let mut ready = self.ready.write().await;
            for id in &removed {
                ready.remove(id);
            }
        }
        if !removed.is_empty() {
            self.disable_sources(&removed, generation).await;
            let keep: HashSet<String> = next.clone();
            self.pool.forget_sources(&keep).await;
            let mut watchers = self.watchers.lock().await;
            for id in &removed {
                if let Some(handle) = watchers.remove(id) {
                    handle.abort();
                }
            }
        }
        // Register the surviving sources' bookkeeping rows and current generation. Grants and
        // embeddings are only applied after a successful sync.
        let _ = generation;
        self.register_configured_sources().await;
    }

    async fn register_configured_sources(&self) {
        let sources = self.config.read().await.clone();
        let generation = self.config_generation();
        let owner = self.principal_id.clone();
        let writer = self.writer.clone();
        let register: Vec<(String, String)> = sources
            .sources
            .iter()
            .filter(|source| source.enabled)
            .map(|source| (source.id.clone(), source.endpoint_hash()))
            .collect();
        let _ = writer.write(move |connection| {
            let transaction = connection
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                .map_err(crate::database_error)?;
            for (id, endpoint_hash) in &register {
                repository::upsert_source(&transaction, id, "mcp_http", &owner, true)
                    .map_err(|_| "storage".to_string())?;
                mcp_repository::upsert_source(&transaction, id, generation, endpoint_hash)
                    .map_err(|_| "storage".to_string())?;
            }
            transaction.commit().map_err(crate::database_error)?;
            Ok(())
        });
    }

    async fn disable_sources(&self, ids: &[String], generation: i64) {
        let writer = self.writer.clone();
        let ids = ids.to_vec();
        let owner = self.principal_id.clone();
        let _ = writer.write(move |connection| {
            let transaction = connection
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                .map_err(crate::database_error)?;
            let mut catalog_changed = false;
            let mut acl_changed = false;
            for id in &ids {
                // Create (or refresh) the ledger row with the new generation even for a source
                // that was never successfully synced. A sync that is still running with the old
                // generation then fails its in-transaction generation check instead of
                // re-publishing a removed source.
                repository::upsert_source(&transaction, id, "mcp_http", &owner, false)
                    .map_err(|_| "storage".to_string())?;
                let endpoint_hash = mcp_repository::source(&transaction, id)
                    .ok()
                    .flatten()
                    .map(|row| row.endpoint_hash)
                    .unwrap_or_else(|| "0".repeat(64));
                mcp_repository::upsert_source(&transaction, id, generation, &endpoint_hash)
                    .map_err(|_| "storage".to_string())?;
                let disabled = mcp_repository::set_source_tools_enabled(&transaction, id, false)
                    .map_err(|_| "storage".to_string())?;
                let grants = mcp_repository::managed_grants_for_source(&transaction, id)
                    .map_err(|_| "storage".to_string())?;
                for grant in grants {
                    if mcp_repository::delete_managed_grant(&transaction, &grant)
                        .map_err(|_| "storage".to_string())?
                    {
                        acl_changed = true;
                    }
                }
                catalog_changed |= disabled > 0;
            }
            if catalog_changed {
                repository::bump_epochs(&transaction, true, false, false)
                    .map_err(|_| "storage".to_string())?;
            }
            if acl_changed {
                repository::bump_epochs(&transaction, false, true, false)
                    .map_err(|_| "storage".to_string())?;
            }
            transaction.commit().map_err(crate::database_error)?;
            Ok(())
        });
    }

    /// Runs one source sync under a per-source single-flight guard. Config-derived grants are
    /// applied in a separate transaction only after the publish transaction committed.
    pub async fn sync_source(&self, source_id: &str) -> Result<SyncOutcome, SyncError> {
        if self.shutting_down.load(Ordering::SeqCst) {
            return Err(SyncError {
                code: "shutting-down",
            });
        }
        let Some(spec) = self.source_spec(source_id).await else {
            return Err(SyncError {
                code: "source-unknown",
            });
        };
        if !spec.enabled {
            return Err(SyncError {
                code: "source-disabled",
            });
        }
        {
            let mut syncing = self.syncing.lock().await;
            if !syncing.insert(source_id.to_string()) {
                return Err(SyncError {
                    code: "sync-in-progress",
                });
            }
        }
        let generation = self.config_generation();
        let endpoint_hash = spec.endpoint_hash();
        let outcome = sync::sync_source(
            &self.writer,
            &self.pool,
            &spec,
            generation,
            &self.principal_id,
            &endpoint_hash,
        )
        .await;
        self.syncing.lock().await.remove(source_id);
        match outcome {
            Ok(outcome) => {
                self.ready.write().await.insert(source_id.to_string());
                self.source_diagnostics.write().await.remove(source_id);
                self.apply_config_grants(source_id, generation).await;
                // A failed embedding pass degrades to BM25 and is retried on the next sync.
                let _ = self.index_missing_embeddings().await;
                self.watch_notifications(source_id).await;
                Ok(outcome)
            }
            Err(error) => {
                self.ready.write().await.remove(source_id);
                let code = error.code;
                let writer = self.writer.clone();
                let id = source_id.to_string();
                let _ = writer.write(move |connection| {
                    mcp_repository::mark_error(connection, &id, code)
                        .map_err(|_| "storage".to_string())
                });
                self.source_diagnostics
                    .write()
                    .await
                    .insert(source_id.to_string(), code);
                Err(error)
            }
        }
    }

    pub async fn sync_all(&self) {
        let sources = self.config.read().await.clone();
        for source in sources.sources.iter().filter(|source| source.enabled) {
            let _ = self.sync_source(&source.id).await;
        }
    }

    /// Spawns a small watcher for the optional GET stream. A `tools/list_changed` notification
    /// marks the source dirty; the polling loop re-syncs after the debounce. A server that does
    /// not offer GET (405) simply returns no stream.
    async fn watch_notifications(&self, source_id: &str) {
        // At most one GET watcher per source. The watcher reconnects on its own, so a periodic
        // resync does not open an additional stream.
        {
            let mut watchers = self.watchers.lock().await;
            if let Some(handle) = watchers.get(source_id) {
                if !handle.is_finished() {
                    return;
                }
            }
            watchers.remove(source_id);
        }
        let Some(spec) = self.source_spec(source_id).await else {
            return;
        };
        let Ok(session) = self.pool.session_for(&spec, self.config_generation()).await else {
            return;
        };
        let notify = self.dirty_notify.clone();
        let dirty = self.dirty.clone();
        let id = source_id.to_string();
        let handle = tokio::spawn(async move {
            // A server without GET support (405) returns `None` and monitoring stops; the 60s
            // poll still catches changes. A stream that closes is re-opened after a short pause.
            while let Some(mut receiver) = session.open_event_stream().await {
                while let Some(message) = receiver.recv().await {
                    if message.get("method").and_then(Value::as_str)
                        == Some("notifications/tools/list_changed")
                    {
                        dirty.lock().await.insert(id.clone());
                        notify.notify_waiters();
                    }
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        });
        self.watchers
            .lock()
            .await
            .insert(source_id.to_string(), handle);
    }

    pub async fn mark_dirty(&self, source_id: &str) {
        self.dirty.lock().await.insert(source_id.to_string());
        self.dirty_notify.notify_waiters();
    }

    /// Spawns the periodic poll loop and performs one immediate sync so a restart never serves a
    /// remote invoke before this process has observed a successful sync. Tests call the
    /// individual methods instead.
    pub fn start_background(self: &Arc<Self>) {
        let manager = self.clone();
        tokio::spawn(async move {
            manager.sync_all().await;
            loop {
                if manager.shutting_down.load(Ordering::SeqCst) {
                    break;
                }
                tokio::select! {
                    _ = tokio::time::sleep(MCP_SOURCE_POLL_INTERVAL) => {}
                    _ = manager.dirty_notify.notified() => {
                        tokio::time::sleep(MCP_NOTIFICATION_DEBOUNCE).await;
                    }
                }
                if manager.shutting_down.load(Ordering::SeqCst) {
                    break;
                }
                let dirty: Vec<String> = {
                    let mut dirty = manager.dirty.lock().await;
                    dirty.drain().collect()
                };
                // Expired continuation rows are removed on the periodic pass.
                let writer = manager.writer.clone();
                let _ = writer.write(|connection| {
                    super::results::cleanup(connection)
                        .map(|_| ())
                        .map_err(|error| error.code.as_str().to_string())
                });
                if dirty.is_empty() {
                    manager.sync_all().await;
                } else {
                    for id in dirty {
                        let _ = manager.sync_source(&id).await;
                    }
                }
            }
        });
    }

    /// Applies the config-derived grants for one source in its own transaction. Unknown tool
    /// names stay pending until a later sync publishes them.
    async fn apply_config_grants(&self, source_id: &str, generation: i64) {
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
                let manually_granted = super::super::source_lookup::exact_grant_exists(
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

    async fn desired_grants(
        &self,
        source_id: &str,
        grants: &[super::config::McpGrantSpec],
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

    fn project_exists(&self, project_id: &str) -> bool {
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

    /// Embeds only revisions that do not yet have a vector for the configured model. Old revision
    /// vectors are never moved to a new revision.
    async fn index_missing_embeddings(&self) -> Result<usize, ()> {
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
                    super::super::repository::truncate_utf8(
                        text,
                        super::super::contracts::SEARCH_TEXT_MAX_BYTES,
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

    async fn check_locked(
        &self,
        source_id: &str,
        endpoint_hash: &str,
    ) -> Result<super::config::McpSourceSpec, CallError> {
        if self.stopped.read().await.contains(source_id) {
            return Err(CallError::Closed);
        }
        let Some(spec) = self.source_spec(source_id).await else {
            return Err(CallError::Closed);
        };
        if !spec.enabled || spec.endpoint_hash() != endpoint_hash {
            return Err(CallError::Unavailable("source-changed"));
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

    pub fn is_shutting_down(&self) -> bool {
        self.shutting_down.load(Ordering::SeqCst)
    }

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
