use super::*;

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
