//! Shared SQLite mediation. The host owns the connection and the transaction.
use rusqlite::{Connection, Transaction};
use saaa_larm_session::media::{MediaError, MediaKind, MediaResult};
use saaa_provider_routing::{accepted, attempt, validate_active, RegistrySnapshot, ResolvedRoute};
use serde_json::Value;
use std::sync::{Arc, Mutex};

use crate::{
    contracts::{allowed_mime, ArtifactBytes, HistoryQuery, MediaHostError},
    database_error, initialize,
    ledger::{
        self, cache, cached, finish, finish_query, get, history_limited, mark_absent_cancelled,
        phase, release_unsent, reserve, CancelRecord, FinishOutcome,
    },
};

pub trait DbOwner: Send + Sync {
    fn read<T>(
        &self,
        operation: impl FnOnce(&Connection) -> Result<T, String> + Send,
    ) -> Result<T, String>;
    fn write<T>(
        &self,
        operation: impl FnOnce(&mut Connection) -> Result<T, String> + Send,
    ) -> Result<T, String>;
}

pub struct MutexDb {
    connection: Mutex<Connection>,
}

impl MutexDb {
    pub fn new(connection: Connection) -> Self {
        Self {
            connection: Mutex::new(connection),
        }
    }
}

impl DbOwner for MutexDb {
    fn read<T>(
        &self,
        operation: impl FnOnce(&Connection) -> Result<T, String> + Send,
    ) -> Result<T, String> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| "Database writer unavailable".to_string())?;
        operation(&connection)
    }

    fn write<T>(
        &self,
        operation: impl FnOnce(&mut Connection) -> Result<T, String> + Send,
    ) -> Result<T, String> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| "Database writer unavailable".to_string())?;
        operation(&mut connection)
    }
}

type RegistryFn = dyn Fn(&Connection) -> Result<RegistrySnapshot, String> + Send + Sync;

pub struct SqlStore<O> {
    owner: O,
    registry: Arc<RegistryFn>,
}

impl<O> SqlStore<O> {
    pub fn new(owner: O, registry: Arc<RegistryFn>) -> Self {
        Self { owner, registry }
    }

    fn storage(error: String) -> MediaHostError {
        MediaHostError::storage(error)
    }
}

impl<O: DbOwner> crate::ports::MediaStore for SqlStore<O> {
    fn load_registry(&self) -> Result<RegistrySnapshot, MediaHostError> {
        self.owner
            .read(|connection| (self.registry)(connection))
            .map_err(Self::storage)
    }

    fn validate_route(&self, route: &ResolvedRoute) -> Result<(), MediaHostError> {
        self.owner
            .read(|connection| {
                let snapshot = (self.registry)(connection)?;
                validate_active(connection, route, &snapshot)
            })
            .map_err(|error| {
                if error.contains("SQLite") {
                    MediaHostError::storage(error)
                } else {
                    MediaHostError::route(error)
                }
            })
    }

    fn reserve(
        &self,
        now: &str,
        run: &str,
        kind: &MediaKind,
        route: &ResolvedRoute,
    ) -> Result<(), MediaHostError> {
        self.owner
            .write(|connection| {
                initialize(connection)?;
                in_transaction(connection, |tx| {
                    let snapshot = (self.registry)(tx)?;
                    reserve(tx, now, run, kind, route, |db| {
                        validate_active(db, route, &snapshot)
                    })
                })
            })
            .map_err(Self::storage)
    }

    fn record_phase(
        &self,
        now: &str,
        run: &str,
        phase_name: &str,
        job: Option<&str>,
    ) -> Result<(), MediaHostError> {
        self.owner
            .write(|connection| phase(connection, now, run, phase_name, job))
            .map_err(Self::storage)
    }

    fn record_attempt(
        &self,
        now: &str,
        run: &str,
        route: &ResolvedRoute,
        attempt_id: &str,
        success: Option<bool>,
    ) -> Result<(), MediaHostError> {
        self.owner
            .write(|connection| attempt(connection, run, route, attempt_id, success, now))
            .map_err(Self::storage)
    }

    fn finish(
        &self,
        now: &str,
        run: &str,
        route: &ResolvedRoute,
        result: &Result<MediaResult, MediaError>,
    ) -> Result<FinishOutcome, MediaHostError> {
        self.finish_with(now, run, route, result, false)
    }

    fn finish_query(
        &self,
        now: &str,
        run: &str,
        route: &ResolvedRoute,
        result: &Result<MediaResult, MediaError>,
    ) -> Result<FinishOutcome, MediaHostError> {
        self.finish_with(now, run, route, result, true)
    }

    fn request_cancel(&self, now: &str, run: &str) -> Result<CancelRecord, MediaHostError> {
        self.owner
            .write(|connection| ledger::request_cancel(connection, now, run))
            .map_err(Self::storage)
    }

    fn mark_absent_cancelled(&self, now: &str, run: &str) -> Result<(), MediaHostError> {
        self.owner
            .write(|connection| {
                initialize(connection)?;
                mark_absent_cancelled(connection, now, run)
            })
            .map_err(Self::storage)
    }

    fn history(&self, query: &HistoryQuery) -> Result<Vec<Value>, MediaHostError> {
        self.owner
            .write(|connection| initialize(connection))
            .map_err(Self::storage)?;
        self.owner
            .read(|connection| match query {
                HistoryQuery::Latest { limit } => history_limited(connection, *limit, None),
                HistoryQuery::ByRunId(run) => history_limited(connection, 1, Some(run)),
            })
            .map_err(Self::storage)
    }

    fn get(&self, run: &str) -> Result<Option<Value>, MediaHostError> {
        self.owner
            .write(|connection| initialize(connection))
            .map_err(Self::storage)?;
        self.owner
            .read(|connection| get(connection, run))
            .map_err(Self::storage)
    }

    fn cache(&self, run: &str, index: usize, bytes: &[u8]) -> Result<(), MediaHostError> {
        self.owner
            .write(|connection| cache(connection, run, index, bytes))
            .map_err(Self::storage)
    }

    fn cached(&self, run: &str, index: usize) -> Result<Option<ArtifactBytes>, MediaHostError> {
        self.owner
            .write(|connection| initialize(connection))
            .map_err(Self::storage)?;
        self.owner
            .read(|connection| {
                let Some(bytes) = cached(connection, run, index)? else {
                    return Ok(None);
                };
                let stored =
                    get(connection, run)?.ok_or_else(|| "生成の記録がありません".to_string())?;
                let mime = stored["result"]["artifacts"][index]["mimeType"]
                    .as_str()
                    .ok_or_else(|| "成果物の形式が記録されていません".to_string())?;
                if !allowed_mime(mime) {
                    return Err("成果物の形式を確認できません".into());
                }
                Ok(Some(ArtifactBytes {
                    bytes,
                    mime_type: mime.to_string(),
                }))
            })
            .map_err(Self::storage)
    }

    fn reconcile_interrupted(&self, now: &str) -> Result<(), MediaHostError> {
        self.owner
            .write(|connection| ledger::reconcile_interrupted(connection, now).map(|_| ()))
            .map_err(Self::storage)
    }

    fn release_unsent(&self, run: &str) -> Result<(), MediaHostError> {
        self.owner
            .write(|connection| release_unsent(connection, run))
            .map_err(Self::storage)
    }
}

impl<O: DbOwner> SqlStore<O> {
    fn finish_with(
        &self,
        now: &str,
        run: &str,
        route: &ResolvedRoute,
        result: &Result<MediaResult, MediaError>,
        replace_unknown: bool,
    ) -> Result<FinishOutcome, MediaHostError> {
        self.owner
            .write(|connection| {
                in_transaction(connection, |tx| {
                    let snapshot = (self.registry)(tx)?;
                    let validate = |db: &Connection| validate_active(db, route, &snapshot);
                    let accept =
                        |db: &Connection, used: &ResolvedRoute| accepted(db, run, used, now);
                    if replace_unknown {
                        finish_query(tx, now, run, route, result, validate, accept)
                    } else {
                        finish(tx, now, run, route, result, validate, accept)
                    }
                })
            })
            .map_err(Self::storage)
    }
}

fn in_transaction<T>(
    connection: &mut Connection,
    operation: impl FnOnce(&Transaction<'_>) -> Result<T, String>,
) -> Result<T, String> {
    let transaction = connection.transaction().map_err(database_error)?;
    let value = operation(&transaction)?;
    transaction.commit().map_err(database_error)?;
    Ok(value)
}
