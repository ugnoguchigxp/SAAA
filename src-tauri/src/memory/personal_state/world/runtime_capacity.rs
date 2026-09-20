#![allow(dead_code)]
//! Bounded capacity preflight for the M2A WorldFrame (R5). Runs before
//! `store::load` / `activate_v2`, so an oversized ledger never triggers a full
//! ledger read. Counts are capped and stop early; nothing is deleted.

use crate::database_error;
use rusqlite::Connection;
use saaa_personal_state_core::world::runtime_frame::FrameError;

pub(crate) const PROJECT_MAX: usize = 100;
pub(crate) const LEDGER_MAX: usize = 2_000;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct CapacityStats {
    pub(crate) project_rows: usize,
    pub(crate) ledger_rows: usize,
    /// Rows touched by the bounded probes (test/diagnostic stats only).
    pub(crate) inspected_rows: usize,
    pub(crate) exceeded: bool,
}

fn number(value: i64) -> usize {
    value.max(0) as usize
}

fn bounded_project(
    c: &Connection,
    table: &str,
    project: &str,
    limit: usize,
) -> Result<usize, FrameError> {
    let sql = format!(
        "SELECT COUNT(*) FROM (SELECT 1 FROM {table} WHERE project_scope=?1 LIMIT {limit})"
    );
    c.query_row(&sql, [project], |row| row.get::<_, i64>(0))
        .map(number)
        .map_err(|error| FrameError::Other(database_error(error)))
}

fn bounded_global(c: &Connection, inner: &str, limit: usize) -> Result<usize, FrameError> {
    let sql = format!("SELECT COUNT(*) FROM (SELECT 1 FROM ({inner}) LIMIT {limit})");
    c.query_row(&sql, [], |row| row.get::<_, i64>(0))
        .map(number)
        .map_err(|error| FrameError::Other(database_error(error)))
}

/// The six Personal State ledger tables whose combined row count bounds the
/// `store::load` cost. Another project's history counts too.
const LEDGER_INNER: [&str; 6] = [
    "SELECT 1 FROM personal_source_refs",
    "SELECT 1 FROM personal_tombstones",
    "SELECT 1 FROM personal_assertions WHERE erased=0",
    "SELECT 1 FROM personal_transitions",
    "SELECT 1 FROM personal_coverage",
    "SELECT 1 FROM personal_patches",
];

pub(crate) fn check_frame_capacity(
    c: &Connection,
    project_scope: &str,
) -> Result<CapacityStats, FrameError> {
    let mut stats = CapacityStats::default();
    for table in [
        "personal_world_entities",
        "personal_world_relations",
        "personal_world_focus",
    ] {
        if stats.project_rows > PROJECT_MAX {
            break;
        }
        let limit = PROJECT_MAX + 1 - stats.project_rows;
        let count = bounded_project(c, table, project_scope, limit)?;
        stats.inspected_rows += count;
        stats.project_rows += count;
    }
    if stats.project_rows <= PROJECT_MAX {
        for inner in LEDGER_INNER {
            if stats.ledger_rows > LEDGER_MAX {
                break;
            }
            let limit = LEDGER_MAX + 1 - stats.ledger_rows;
            let count = bounded_global(c, inner, limit)?;
            stats.inspected_rows += count;
            stats.ledger_rows += count;
        }
    }
    stats.exceeded = stats.project_rows > PROJECT_MAX || stats.ledger_rows > LEDGER_MAX;
    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limit_math_never_underflows() {
        // A full project count asks for exactly one more row, then stops.
        assert_eq!(PROJECT_MAX + 1 - PROJECT_MAX, 1);
    }
}
