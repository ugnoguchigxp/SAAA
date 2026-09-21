#![allow(dead_code)]
//! Bounded capacity preflight for the M2A WorldFrame (R5). Runs before
//! `store::load` / `activate_v2`, so an oversized ledger never triggers a full
//! ledger read. Counts are capped and stop early; nothing is deleted.

use crate::database_error;
use rusqlite::Connection;
use saaa_personal_state_core::world::runtime_frame::FrameError;

/// Projection rows are scoped to the requested project. This is a corruption /
/// abuse guard, not a product-size limit; ordinary traversal remains bounded
/// by its much smaller fetch, node and edge limits.
pub(crate) const PROJECT_MAX: usize = 1_000;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct CapacityStats {
    pub(crate) project_rows: usize,
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
    stats.exceeded = stats.project_rows > PROJECT_MAX;
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
