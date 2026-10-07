//! Durable submission identity. The SQL lives in `saaa-media`.
//! This module owns the desktop writer transaction and does not open another database.
use super::*;
use crate::providers::service_registry::ResolvedRoute;

pub(super) fn initialize(db: &rusqlite::Connection) -> Result<(), String> {
    saaa_media::initialize(db)
}

pub(super) fn reserve(
    state: &AppState,
    run: &str,
    kind: MediaKind,
    route: &ResolvedRoute,
) -> Result<(), String> {
    state.sqlite_writer.write(|db| {
        initialize(db)?;
        let tx = db.transaction().map_err(crate::database_error)?;
        saaa_media::reserve(&tx, &crate::now_iso(), run, &kind, route, |connection| {
            crate::providers::service_registry::validate_active(connection, route)
        })?;
        tx.commit().map_err(crate::database_error)
    })
}

pub(super) fn phase(
    state: &AppState,
    run: &str,
    phase: &str,
    job: Option<&str>,
) -> Result<(), String> {
    state
        .sqlite_writer
        .write(|db| saaa_media::phase(db, &crate::now_iso(), run, phase, job))
}

pub(super) fn finish(
    state: &AppState,
    run: &str,
    route: &ResolvedRoute,
    result: &Result<MediaResult, MediaError>,
) -> Result<(), String> {
    state.sqlite_writer.write(|db| {
        let tx = db.transaction().map_err(crate::database_error)?;
        saaa_media::finish(
            &tx,
            &crate::now_iso(),
            run,
            route,
            result,
            |connection| crate::providers::service_registry::validate_active(connection, route),
            |connection, used| {
                crate::providers::service_registry::operations::accepted(connection, run, used)
            },
        )?;
        tx.commit().map_err(crate::database_error)
    })
}

pub(super) fn get(state: &AppState, run: &str) -> Result<Option<serde_json::Value>, String> {
    state.sqlite_writer.write(|db| initialize(db))?;
    state.sqlite_readers.read(|db| saaa_media::get(db, run))
}

pub(super) fn cached(state: &AppState, run: &str, index: usize) -> Result<Option<Vec<u8>>, String> {
    state.sqlite_writer.write(|db| initialize(db))?;
    state
        .sqlite_readers
        .read(|db| saaa_media::cached(db, run, index))
}

pub(super) fn cache(state: &AppState, run: &str, index: usize, bytes: &[u8]) -> Result<(), String> {
    state
        .sqlite_writer
        .write(|db| saaa_media::cache(db, run, index, bytes))
}
