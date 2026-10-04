use rusqlite::{Connection, OptionalExtension};
use serde_json::{json, Value};

use crate::tool_selection::catalog::{register_revision, CatalogEntry, UsagePage};

pub(crate) fn ensure_registered(connection: &Connection, principal_id: &str) -> Result<(), String> {
    ensure_registered_with_note(connection, principal_id, "")
}

/// `note` is part of the revision hash. An empty note matches the production catalog.
pub(crate) fn ensure_registered_with_note(
    connection: &Connection,
    principal_id: &str,
    note: &str,
) -> Result<(), String> {
    let schema = json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "operation": {
                "type": "string",
                "enum": ["next_tab", "previous_tab", "select_tab", "scroll", "close_tab", "close_all_tabs"]
            },
            "index": { "type": "integer", "minimum": 0 }
        },
        "required": ["operation"]
    });
    let entry = CatalogEntry {
        tool_id: "artifact_webview".into(),
        backend_key: "artifact_webview".into(),
        title: "Website artifact controls".into(),
        purpose: "Move, scroll, and close website tabs in the current conversation artifact panel.".into(),
        operations: vec!["next_tab".into(), "previous_tab".into(), "select_tab".into(), "scroll".into(), "close_tab".into(), "close_all_tabs".into()],
        objects: vec!["website tab".into()],
        suitable: vec!["the user asks to switch, scroll, or close the open website tabs".into()],
        unsuitable: vec!["no website tab is open".into(), "the target is a semantic UI or interactive preview".into()],
        required_inputs: vec!["operation".into()],
        input_schema: schema.clone(),
        output_schema: None,
        effect: "write",
        usage_pages: vec![UsagePage {
            section: "usage",
            page: 1,
            text: format!(
                "Use next_tab, previous_tab, select_tab with a zero-based index, scroll, close_tab, or close_all_tabs. These affect only website tabs in the current conversation.{note}"
            ),
        }],
        backend_binding: json!({"kind": "artifact_webview", "operation": "dispatch"}),
    };
    // A revision includes its guidance as well as its input shape. Otherwise a wording
    // update silently leaves the previously stored usage page current.
    let revision_id = crate::tool_selection::catalog::hex_sha256(
        json!({"schema": schema, "search": entry.search_text(), "usage": entry.usage_pages[0].text})
            .to_string()
            .as_bytes(),
    );
    register_revision(
        connection,
        principal_id,
        "artifact-webview",
        &entry,
        &revision_id,
        crate::schedule::tick::now_ms(),
    )
    .map_err(|error| error.to_string())?;
    crate::tool_selection::repository::upsert_grant(
        connection,
        principal_id,
        "artifact_webview",
        "user",
        principal_id,
    )
    .map_err(|error| error.to_string())?;
    Ok(())
}

#[path = "webview_catalog/offered.rs"]
mod offered;
pub(crate) use offered::offered_definition;
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registers_webview_tool_in_sqlite() {
        let connection = Connection::open_in_memory().unwrap();
        crate::persistence::schema::initialize_database(&connection).unwrap();
        ensure_registered(&connection, "principal").unwrap();
        let kind: String = connection
            .query_row(
                "SELECT s.kind FROM tool_selection_sources s JOIN tool_selection_catalog c ON c.source_id=s.id WHERE c.id='artifact_webview'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(kind, "artifact_webview");
        assert!(offered_definition(&connection, "principal")
            .unwrap()
            .is_some());
        assert!(offered_definition(&connection, "other").unwrap().is_none());
    }
}
