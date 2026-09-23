use super::*;
/// Settles any invocation left `running` by a crash so the ledger never reports a call as still
/// in flight after a restart. Returns the number of rows repaired.
pub fn reconcile_interrupted_invocations(writer: &SqliteWriter) -> ToolSelectionResult<usize> {
    writer
        .write(|connection| {
            connection
                .execute(
                    "UPDATE tool_selection_invocations
                        SET technical_status = 'interrupted',
                            finished_at = ?1,
                            error_code = CASE
                              WHEN error_code IS NULL AND EXISTS (
                                SELECT 1 FROM tool_selection_revisions r
                                 WHERE r.id = revision_id
                                   AND json_extract(r.backend_binding_json, '$.kind') = 'mcp_http')
                              THEN 'remote-outcome-unknown'
                              ELSE error_code END
                      WHERE technical_status = 'running'",
                    rusqlite::params![now_ms()],
                )
                .map_err(crate::database_error)
        })
        .map_err(|_| ToolSelectionError::storage())
}
pub(super) fn write_transaction<T>(
    writer: &SqliteWriter,
    action: impl FnOnce(&Connection) -> Result<T, String>,
) -> Result<T, String> {
    writer.write(|connection| {
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(crate::database_error)?;
        let value = action(&transaction)?;
        transaction.commit().map_err(crate::database_error)?;
        Ok(value)
    })
}
pub(super) fn title_and_summary(search_text: &str) -> (String, String) {
    let mut title = String::new();
    let mut purpose = String::new();
    for line in search_text.lines() {
        if title.is_empty() {
            if let Some(rest) = line.strip_prefix("title: ") {
                title = rest.to_string();
            }
        }
        if purpose.is_empty() {
            if let Some(rest) = line.strip_prefix("purpose: ") {
                purpose = rest.to_string();
            }
        }
    }
    if title.is_empty() {
        title = "tool".to_string();
    }
    let summary =
        repository::truncate_utf8(&purpose, SEARCH_CANDIDATE_SUMMARY_MAX_BYTES).to_string();
    (title, summary)
}
pub(super) fn validate_arguments(schema: &Value, arguments: &Value) -> ToolSelectionResult<()> {
    let bytes = serde_json::to_vec(arguments).map_err(|_| ToolSelectionError::invalid())?;
    if bytes.len() > BACKEND_INPUT_MAX_BYTES {
        return Err(ToolSelectionError::invalid());
    }
    let validator = jsonschema::validator_for(schema).map_err(|_| ToolSelectionError::invalid())?;
    if validator.is_valid(arguments) {
        Ok(())
    } else {
        Err(ToolSelectionError::invalid())
    }
}
pub(super) fn encode_cursor(revision_id: &str, section: &str, page: i64) -> String {
    let raw = json!({ "r": revision_id, "s": section, "p": page }).to_string();
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw.as_bytes())
}
pub(super) fn decode_cursor(
    cursor: Option<&str>,
    revision_id: &str,
    section: &str,
) -> ToolSelectionResult<i64> {
    let Some(cursor) = cursor else {
        return Ok(0);
    };
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(cursor)
        .map_err(|_| ToolSelectionError::invalid())?;
    let value: Value = serde_json::from_slice(&bytes).map_err(|_| ToolSelectionError::invalid())?;
    let bound_revision = value.get("r").and_then(Value::as_str);
    let bound_section = value.get("s").and_then(Value::as_str);
    if bound_revision != Some(revision_id) || bound_section != Some(section) {
        return Err(ToolSelectionError::not_found());
    }
    let page = value
        .get("p")
        .and_then(Value::as_i64)
        .filter(|page| *page >= 0)
        .ok_or_else(ToolSelectionError::invalid)?;
    Ok(page)
}
