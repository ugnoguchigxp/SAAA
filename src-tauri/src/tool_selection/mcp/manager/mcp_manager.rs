use super::*;
pub struct McpManager {
    pub(super) writer: Arc<SqliteWriter>,
    pub(super) principal_id: String,
    pub(super) config_path: Option<PathBuf>,
    pub(super) config: RwLock<Arc<McpSources>>,
    pub(super) generation: AtomicI64,
    pub(super) pool: Arc<McpSessionPool>,
    pub(super) embedder: Option<Arc<dyn EmbeddingProvider>>,
    pub(super) syncing: Mutex<HashSet<String>>,
    pub(super) ready: RwLock<HashSet<String>>,
    pub(super) stopped: RwLock<HashSet<String>>,
    pub(super) gates: Mutex<HashMap<String, Arc<RwLock<()>>>>,
    pub(super) dirty: Arc<Mutex<HashSet<String>>>,
    pub(super) dirty_notify: Arc<Notify>,
    pub(super) watchers: Mutex<HashMap<String, tokio::task::JoinHandle<()>>>,
    pub(super) source_diagnostics: RwLock<HashMap<String, &'static str>>,
    pub(super) config_diagnostic: RwLock<Option<&'static str>>,
    /// Canonical loopback endpoint of this process's own MCP listener, when one is running. A D4
    /// source pointing at it would call the gateway back into itself and is refused.
    pub(super) self_endpoint: RwLock<Option<String>>,
    pub(super) shutting_down: AtomicBool,
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
            self_endpoint: RwLock::new(None),
            shutting_down: AtomicBool::new(false),
        })
    }
}
impl McpManager {
/// Records this process's own MCP endpoint so a D4 source that points at it can be refused.
    pub async fn set_self_endpoint(&self, endpoint: Option<String>) {
        *self.self_endpoint.write().await = endpoint;
    }
}
impl McpManager {
pub(super) async fn is_self_endpoint(&self, url: &str) -> bool {
        let Some(self_endpoint) = self.self_endpoint.read().await.clone() else {
            return false;
        };
        normalize_loopback(url).as_deref() == Some(self_endpoint.as_str())
    }
}
impl McpManager {
pub fn principal_id(&self) -> &str {
        &self.principal_id
    }
}
impl McpManager {
pub fn config_generation(&self) -> i64 {
        self.generation.load(Ordering::SeqCst)
    }
}
impl McpManager {
pub async fn sources(&self) -> Arc<McpSources> {
        self.config.read().await.clone()
    }
}
impl McpManager {
pub async fn source_spec(&self, source_id: &str) -> Option<super::super::config::McpSourceSpec> {
        self.config.read().await.get(source_id).cloned()
    }
}
impl McpManager {
pub async fn is_ready(&self, source_id: &str) -> bool {
        self.ready.read().await.contains(source_id)
    }
}
impl McpManager {
pub async fn config_diagnostic(&self) -> Option<&'static str> {
        *self.config_diagnostic.read().await
    }
}
impl McpManager {
pub(super) async fn gate_for(&self, source_id: &str) -> Arc<RwLock<()>> {
        let mut gates = self.gates.lock().await;
        gates
            .entry(source_id.to_string())
            .or_insert_with(|| Arc::new(RwLock::new(())))
            .clone()
    }
}
impl McpManager {
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
}
impl McpManager {
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
}
impl McpManager {
pub(super) async fn register_configured_sources(&self) {
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
            let mut rules_invalidated = false;
            for (id, endpoint_hash) in &register {
                let previous_endpoint = mcp_repository::source(&transaction, id)
                    .ok()
                    .flatten()
                    .map(|row| row.endpoint_hash);
                repository::upsert_source(&transaction, id, "mcp_http", &owner, true)
                    .map_err(|_| "storage".to_string())?;
                mcp_repository::upsert_source(&transaction, id, generation, endpoint_hash)
                    .map_err(|_| "storage".to_string())?;
                // A changed endpoint invalidates the corrections learned on the old one. They stay
                // unconfirmed even if the original URL is configured again, so a re-created
                // correction is required before they affect selection.
                if let Some(previous) = previous_endpoint {
                    if previous != *endpoint_hash && !previous.is_empty() {
                        let changed = repository::invalidate_source_rule_bindings(&transaction, id)
                            .map_err(|_| "storage".to_string())?;
                        rules_invalidated |= changed > 0;
                    }
                }
            }
            if rules_invalidated {
                repository::bump_epochs(&transaction, false, false, true)
                    .map_err(|_| "storage".to_string())?;
            }
            transaction.commit().map_err(crate::database_error)?;
            Ok(())
        });
    }
}
impl McpManager {
pub(super) async fn disable_sources(&self, ids: &[String], generation: i64) {
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
}
impl McpManager {
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
        if self.is_self_endpoint(&spec.url).await {
            return Err(SyncError {
                code: "source-self-reference",
            });
        }
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
}
impl McpManager {
pub async fn sync_all(&self) {
        let sources = self.config.read().await.clone();
        for source in sources.sources.iter().filter(|source| source.enabled) {
            let _ = self.sync_source(&source.id).await;
        }
    }
}
impl McpManager {
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
}
impl McpManager {
pub async fn mark_dirty(&self, source_id: &str) {
        self.dirty.lock().await.insert(source_id.to_string());
        self.dirty_notify.notify_waiters();
    }
}
impl McpManager {
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
                    super::super::results::cleanup(connection)
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
}
