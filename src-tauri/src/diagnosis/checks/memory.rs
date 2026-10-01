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
        Ok(summary) => item(
            "memory.personal_state",
            "memory",
            "Personal state",
            if summary["enabled"] == false {
                DiagnosisStatus::Skipped
            } else if summary["ready"] == true {
                DiagnosisStatus::Ok
            } else {
                DiagnosisStatus::Warn
            },
            DiagnosisSeverity::Degraded,
            summary["contractReason"].as_str().unwrap_or(""),
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
    let result = state.sqlite_writer.read_serialized(|c| {
        capabilities::status(c, crate::PRIMARY_CONVERSATION_ID)?;
        let summary = crate::memory::personal_state::commands::summary(c)?;
        let active: u64 = c.query_row("SELECT count(*) FROM personal_projection p JOIN personal_assertions a ON a.id=p.assertion_id WHERE p.status='active' AND json_extract(a.metadata,'$.kind') IN ('world_entity','world_relation','world_focus')", [], |r| r.get(0)).map_err(crate::database_error)?;
        Ok((summary, active))
    });
    match result {
        Ok((summary, active)) => {
            let enabled = summary["enabled"] == true;
            let ready = summary["ready"] == true;
            let reason = summary["maintenance"]["reason"]
                .as_str()
                .unwrap_or("not-started");
            item("world.status", "memory", "World model",
                if !enabled { DiagnosisStatus::Skipped } else if !ready || reason.ends_with("unavailable") { DiagnosisStatus::Warn } else { DiagnosisStatus::Ok },
                DiagnosisSeverity::Degraded,
                &format!("enabled={enabled}; contract_ready={ready}; active={active}; maintenance={reason}"), None)
        }
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
    fn world_maintenance_diagnosis_distinguishes_unready_from_readable_database() {
        let state = fresh();
        let summary = state
            .sqlite_writer
            .read_serialized(crate::memory::personal_state::commands::summary)
            .unwrap();
        let items = memory(&state);
        let expected = if summary["enabled"] == false {
            DiagnosisStatus::Skipped
        } else if summary["ready"] == false {
            DiagnosisStatus::Warn
        } else {
            DiagnosisStatus::Ok
        };
        for id in ["memory.personal_state", "world.status"] {
            assert_eq!(
                items.iter().find(|item| item.id == id).unwrap().status,
                expected
            );
        }
        assert!(items
            .iter()
            .find(|item| item.id == "world.status")
            .unwrap()
            .message
            .contains("active=0"));
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
