use rusqlite::Connection;

use super::{source_id, tool_id};
use crate::generated_capabilities::errors::*;
use crate::tool_selection::catalog;

pub fn unpublish_tool_for_capability(
    connection: &Connection,
    principal_id: &str,
    capability_id: &str,
) -> CapabilityResult<()> {
    let source = source_id(principal_id);
    let tool = tool_id(&source, capability_id);
    catalog::unpublish_tool(connection, &tool).map_err(|_| {
        CapabilityError::new(
            CapabilityErrorCode::StorageError,
            "tool catalog unpublish failed",
        )
    })
}

/// Disables every tool_selection row bound to this generated capability. Missing tools are skipped.
pub fn unpublish_catalog_for_capability(
    connection: &Connection,
    capability_id: &str,
) -> CapabilityResult<()> {
    let mut statement = connection
        .prepare(
            "SELECT DISTINCT tool_id FROM tool_selection_revisions
             WHERE json_extract(backend_binding_json, '$.capabilityId') = ?1",
        )
        .map_err(|_| {
            CapabilityError::new(
                CapabilityErrorCode::StorageError,
                "could not look up generated catalog tools",
            )
        })?;
    let ids = statement
        .query_map(rusqlite::params![capability_id], |row| {
            row.get::<_, String>(0)
        })
        .map_err(|_| {
            CapabilityError::new(
                CapabilityErrorCode::StorageError,
                "could not look up generated catalog tools",
            )
        })?
        .collect::<rusqlite::Result<Vec<String>>>()
        .map_err(|_| {
            CapabilityError::new(
                CapabilityErrorCode::StorageError,
                "could not look up generated catalog tools",
            )
        })?;
    for tool in ids {
        catalog::unpublish_tool(connection, &tool).map_err(|_| {
            CapabilityError::new(
                CapabilityErrorCode::StorageError,
                "tool catalog unpublish failed",
            )
        })?;
    }
    Ok(())
}
