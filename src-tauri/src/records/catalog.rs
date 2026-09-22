use rusqlite::Connection;
use serde_json::json;

use crate::tool_selection::catalog::{register_revision, CatalogEntry, UsagePage};

pub(crate) fn ensure_registered(connection: &Connection, principal_id: &str) -> Result<(), String> {
    for (tool_id, operation) in [
        ("read_record", "read_record"),
        ("recall_activity", "recall_activity"),
    ] {
        let schema = json!({"type": "object", "properties": {"id": {"type": "string"}, "operation": {"const": operation}}});
        let revision_id =
            crate::generated_capabilities::contracts::sha256_hex(schema.to_string().as_bytes());
        let entry = CatalogEntry {
            tool_id: tool_id.into(),
            backend_key: "records".into(),
            title: tool_id.into(),
            purpose: "Read stored records".into(),
            operations: vec![operation.into()],
            objects: vec!["record".into()],
            suitable: vec!["stored evidence".into()],
            unsuitable: vec!["instructions".into()],
            required_inputs: vec!["id".into()],
            input_schema: schema,
            output_schema: None,
            effect: "read",
            usage_pages: vec![UsagePage {
                section: "usage",
                page: 1,
                text: tool_id.into(),
            }],
            backend_binding: json!({"kind": "records", "operation": operation}),
        };
        let source_id = format!("records-{tool_id}");
        register_revision(
            connection,
            principal_id,
            &source_id,
            &entry,
            &revision_id,
            crate::schedule::tick::now_ms(),
        )
        .map_err(|error| error.to_string())?;
        crate::tool_selection::repository::upsert_grant(
            connection,
            principal_id,
            tool_id,
            "user",
            principal_id,
        )
        .map_err(|error| error.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cw_35_ensure_registered_is_idempotent() {
        let connection = Connection::open_in_memory().unwrap();
        crate::persistence::schema::initialize_database(&connection).unwrap();
        ensure_registered(&connection, "principal").unwrap();
        ensure_registered(&connection, "principal").unwrap();
        let count: i64 = connection
            .query_row("SELECT count(*) FROM tool_selection_revisions", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 2);
    }
}
