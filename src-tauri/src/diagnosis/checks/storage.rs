use super::{evidence, pass, CheckFuture};
use crate::diagnosis::contract::{Capability, Outcome, Reason, Tier};
use crate::persistence::schema::DATABASE_SCHEMA_VERSION;
use crate::AppState;
use std::time::Instant;

const RECORDS_DB_SOFT_LIMIT_BYTES: f64 = 10.0 * 1024.0 * 1024.0 * 1024.0;

pub(super) fn run(state: &AppState) -> CheckFuture<'_> {
    Box::pin(async move { collect(state).await })
}

async fn collect(state: &AppState) -> Vec<crate::diagnosis::contract::Evidence> {
    let cap = Capability::Storage;
    let mut out = Vec::new();

    out.push(
        match state.sqlite_readers.read(|connection| {
            connection
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .map_err(crate::database_error)
        }) {
            Ok(version) if version == DATABASE_SCHEMA_VERSION => {
                pass("sqlite.schema", cap, Tier::Static)
            }
            Ok(version) => evidence(
                "sqlite.schema",
                cap,
                Tier::Static,
                Outcome::Degraded,
                Reason::SchemaMismatch,
            )
            .detail(&format!(
                "schema {version}, expected {DATABASE_SCHEMA_VERSION}"
            )),
            Err(error) => evidence(
                "sqlite.schema",
                cap,
                Tier::Static,
                Outcome::Fail,
                Reason::Internal,
            )
            .detail(&error),
        },
    );

    // The writer queue is the path every save takes; a stuck writer is invisible to readers.
    // It blocks, so it runs off the async workers where the deadline can still fire.
    let started = Instant::now();
    let writer = std::sync::Arc::clone(&state.sqlite_writer);
    let probe = tokio::task::spawn_blocking(move || {
        writer.read_serialized(|connection| {
            connection
                .query_row("SELECT 1", [], |row| row.get::<_, i64>(0))
                .map_err(crate::database_error)
        })
    });
    out.push(
        match tokio::time::timeout(std::time::Duration::from_secs(5), probe).await {
            Ok(Ok(Ok(_))) => pass("sqlite.writer", cap, Tier::Probe)
                .latency(Some(started.elapsed().as_millis() as u64)),
            Ok(Ok(Err(error))) => evidence(
                "sqlite.writer",
                cap,
                Tier::Probe,
                Outcome::Fail,
                Reason::Internal,
            )
            .detail(&error),
            Ok(Err(_)) => evidence(
                "sqlite.writer",
                cap,
                Tier::Probe,
                Outcome::Fail,
                Reason::Internal,
            ),
            Err(_) => evidence(
                "sqlite.writer",
                cap,
                Tier::Probe,
                Outcome::Fail,
                Reason::Timeout,
            ),
        },
    );

    out.push(
        match state.sqlite_readers.read(|connection| {
            connection
                .query_row("SELECT COALESCE(SUM(raw_bytes), 0) FROM blobs", [], |row| {
                    row.get::<_, i64>(0)
                })
                .map_err(crate::database_error)
        }) {
            Ok(bytes) => {
                let ratio = bytes.max(0) as f64 / RECORDS_DB_SOFT_LIMIT_BYTES;
                let item = if ratio >= 0.8 {
                    evidence(
                        "records.capacity",
                        cap,
                        Tier::Static,
                        Outcome::Degraded,
                        Reason::CapacityHigh,
                    )
                } else {
                    pass("records.capacity", cap, Tier::Static)
                };
                item.advisory().detail(&format!("{:.0}", ratio * 100.0))
            }
            Err(error) => evidence(
                "records.capacity",
                cap,
                Tier::Static,
                Outcome::Unverified,
                Reason::Internal,
            )
            .advisory()
            .detail(&error),
        },
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn fresh() -> AppState {
        let connection = Connection::open_in_memory().expect("database opens");
        crate::initialize_database(&connection).expect("database initializes");
        crate::test_support::app_state(connection)
    }

    #[tokio::test]
    async fn initialized_database_passes_every_storage_check() {
        let items = collect(&fresh()).await;
        assert_eq!(items.len(), 3);
        assert!(items.iter().all(|item| item.outcome == Outcome::Pass));
    }

    #[tokio::test]
    async fn schema_version_mismatch_degrades_instead_of_failing() {
        let state = fresh();
        state
            .sqlite_writer
            .write(|connection| {
                connection
                    .pragma_update(None, "user_version", 1i64)
                    .map_err(crate::database_error)?;
                Ok(())
            })
            .expect("version changes");
        let items = collect(&state).await;
        let schema = items
            .iter()
            .find(|item| item.source == "sqlite.schema")
            .unwrap();
        assert_eq!(schema.outcome, Outcome::Degraded);
        assert_eq!(schema.reason, Reason::SchemaMismatch);
    }
}
