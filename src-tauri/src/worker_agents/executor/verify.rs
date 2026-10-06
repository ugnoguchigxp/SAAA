//! Host-side check of a finished attempt output. The runner's own claim of success is never used:
//! the host re-checks kind, schema, item count and (for web claims) the recorded sources.
use super::store::db;
use crate::worker_agents::contracts::*;
use rusqlite::{params, Connection};

/// `Ok(false)` means "completion not met": the attempt is retried like any other unmet completion.
/// `Err` is a database fault.
pub(super) fn verify_output(
    connection: &Connection,
    task_id: &str,
    revision: &LoadedRevision,
    output: &WorkerOutput,
) -> Result<bool, String> {
    let met = match (revision.output_kind, output) {
        (OutputKind::WebClaimsV1, WorkerOutput::WebClaimsV1(claims)) => {
            if claims.claims.len() < revision.completion.min_items as usize {
                return Ok(false);
            }
            if revision.completion.sources_must_be_host_recorded {
                for claim in &claims.claims {
                    let usable: bool = connection
                        .query_row(
                            "SELECT EXISTS(SELECT 1 FROM worker_sources
                              WHERE task_id = ?1 AND url = ?2 AND status = 'usable')",
                            params![task_id, claim.source_url],
                            |row| row.get(0),
                        )
                        .map_err(db)?;
                    if !usable {
                        return Ok(false);
                    }
                }
            }
            true
        }
        (OutputKind::JsonV1, WorkerOutput::JsonV1(value)) => match &revision.output_schema {
            Some(schema) => jsonschema::validator_for(schema)
                .map(|validator| validator.is_valid(value))
                .unwrap_or(false),
            None => true,
        },
        _ => false,
    };
    Ok(met)
}
