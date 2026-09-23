use rusqlite::{Connection, OptionalExtension};
use serde_json::{json, Value};

use crate::tool_selection::catalog::{register_revision, CatalogEntry, UsagePage};

pub(crate) fn ensure_registered(connection: &Connection, principal_id: &str) -> Result<(), String> {
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
            text: "Use next_tab, previous_tab, select_tab with a zero-based index, scroll, close_tab, or close_all_tabs. These affect only website tabs in the current conversation.".into(),
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

pub(crate) fn offered_definition(
    connection: &Connection,
    principal_id: &str,
) -> Result<Option<Value>, String> {
    let row: Option<(String, String, String)> = connection
        .query_row(
            "SELECT r.input_schema_json,r.search_text,COALESCE(u.text,'') \
             FROM tool_selection_catalog c \
             JOIN tool_selection_sources s ON s.id=c.source_id \
             JOIN tool_selection_revisions r ON r.id=c.current_revision_id \
             JOIN tool_selection_grants g ON g.tool_id=c.id AND g.principal_id=?1 \
               AND g.scope_kind='user' AND g.scope_id=?1 \
             LEFT JOIN tool_selection_usage_pages u ON u.revision_id=r.id \
               AND u.section='usage' AND u.page=1 \
             WHERE c.id='artifact_webview' AND c.enabled=1 AND s.enabled=1 LIMIT 1",
            [principal_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    let Some((schema, search_text, usage)) = row else {
        return Ok(None);
    };
    let parameters: Value = serde_json::from_str(&schema).map_err(|error| error.to_string())?;
    Ok(Some(json!({
        "type": "function",
        "function": {
            "name": "artifact_webview",
            "description": format!("{search_text}\nUsage: {usage}"),
            "parameters": parameters
        }
    })))
}

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
