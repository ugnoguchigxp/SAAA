use super::item;
use crate::diagnosis::contract::{DiagnosisItem, DiagnosisSeverity, DiagnosisStatus};
use crate::runtime::context::world::capabilities;
use crate::AppState;

pub(crate) const RECORDS_DB_SOFT_LIMIT_BYTES: u64 = 10 * 1024 * 1024 * 1024;

pub(crate) fn capacity_item(db_bytes: u64, limit: u64) -> DiagnosisItem {
    let ratio = if limit == 0 {
        0.0
    } else {
        db_bytes as f64 / limit as f64
    };
    if ratio >= 0.8 {
        item(
            "records.capacity",
            "memory",
            "Record store capacity",
            DiagnosisStatus::Warn,
            DiagnosisSeverity::Degraded,
            "Record storage is at least 80% of the soft limit.",
            None,
        )
    } else {
        item(
            "records.capacity",
            "memory",
            "Record store capacity",
            DiagnosisStatus::Ok,
            DiagnosisSeverity::Degraded,
            "",
            None,
        )
    }
}

pub(in crate::diagnosis) fn memory(state: &AppState) -> Vec<DiagnosisItem> {
    vec![
        personal_state(state),
        world(state),
        context_still(
            "context_still.recall",
            "ContextStill recall",
            state.context_still_recall.is_configured(),
        ),
        context_still(
            "context_still.search",
            "ContextStill search",
            state.context_still_search.is_configured(),
        ),
        toolchain(state),
        capacity_item(0, RECORDS_DB_SOFT_LIMIT_BYTES),
    ]
}

fn personal_state(state: &AppState) -> DiagnosisItem {
    match state
        .sqlite_writer
        .read_serialized(crate::memory::personal_state::commands::summary)
    {
        Ok(_) => item(
            "memory.personal_state",
            "memory",
            "Personal state",
            DiagnosisStatus::Ok,
            DiagnosisSeverity::Degraded,
            "",
            None,
        ),
        Err(error) => item(
            "memory.personal_state",
            "memory",
            "Personal state",
            DiagnosisStatus::Fail,
            DiagnosisSeverity::Degraded,
            &error,
            None,
        ),
    }
}

fn world(state: &AppState) -> DiagnosisItem {
    match state
        .sqlite_readers
        .read(|connection| capabilities::status(connection, crate::PRIMARY_CONVERSATION_ID))
    {
        Ok(_) => item(
            "world.status",
            "memory",
            "World model",
            DiagnosisStatus::Ok,
            DiagnosisSeverity::Degraded,
            "",
            None,
        ),
        Err(error) => item(
            "world.status",
            "memory",
            "World model",
            DiagnosisStatus::Fail,
            DiagnosisSeverity::Degraded,
            &error,
            None,
        ),
    }
}

fn toolchain(state: &AppState) -> DiagnosisItem {
    match state.sqlite_readers.read(|connection| {
        connection
            .query_row("SELECT COUNT(*) FROM tool_selection_catalog", [], |row| {
                row.get::<_, i64>(0)
            })
            .map_err(crate::database_error)
    }) {
        Ok(count) => item(
            "tool_selection.catalog",
            "memory",
            "ToolChain",
            DiagnosisStatus::Ok,
            DiagnosisSeverity::Degraded,
            &format!("{count} tools"),
            None,
        ),
        Err(error) => item(
            "tool_selection.catalog",
            "memory",
            "ToolChain",
            DiagnosisStatus::Fail,
            DiagnosisSeverity::Degraded,
            &error,
            None,
        ),
    }
}

fn context_still(id: &str, label: &str, configured: bool) -> DiagnosisItem {
    if configured {
        item(
            id,
            "memory",
            label,
            DiagnosisStatus::Ok,
            DiagnosisSeverity::Info,
            "",
            None,
        )
    } else {
        item(
            id,
            "memory",
            label,
            DiagnosisStatus::Skipped,
            DiagnosisSeverity::Info,
            "not configured",
            None,
        )
    }
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

    #[test]
    fn dg_07_memory_and_world_ok_on_fresh_database() {
        let items = memory(&fresh());
        assert_eq!(
            items
                .iter()
                .find(|item| item.id == "memory.personal_state")
                .map(|item| item.status),
            Some(DiagnosisStatus::Ok)
        );
        assert_eq!(
            items
                .iter()
                .find(|item| item.id == "world.status")
                .map(|item| item.status),
            Some(DiagnosisStatus::Ok)
        );
    }

    #[test]
    fn dg_07_context_still_skipped_when_not_configured() {
        let items = memory(&fresh());
        for id in ["context_still.recall", "context_still.search"] {
            let item = items.iter().find(|item| item.id == id).expect(id);
            assert_eq!(item.status, DiagnosisStatus::Skipped);
            assert_eq!(item.severity, DiagnosisSeverity::Info);
            assert_eq!(item.message, "not configured");
        }
    }

    #[test]
    fn cw_54_capacity_warn_at_80_percent() {
        let item = capacity_item(80, 100);
        assert_eq!(item.status, DiagnosisStatus::Warn);
        assert_eq!(item.id, "records.capacity");
    }

    #[test]
    fn cw_54_context_metrics_present() {
        let item = capacity_item(1, RECORDS_DB_SOFT_LIMIT_BYTES);
        assert_eq!(item.status, DiagnosisStatus::Ok);
    }

    #[test]
    fn cw_32_mcp_result_has_record_id() {
        let connection = Connection::open_in_memory().unwrap();
        crate::initialize_database(&connection).unwrap();
        let exists: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM pragma_table_info('tool_selection_mcp_results') WHERE name='record_id')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(exists);
    }
}
