use super::*;
/// Returns the trusted side-effect classification of the currently-published revision of a tool,
/// looked up by its backend key. `None` means the tool is not published, which callers must treat
/// as mutating (fail closed).
pub(crate) fn effect_for_backend_key(
    connection: &Connection,
    backend_key: &str,
) -> Result<Option<String>, String> {
    connection
        .query_row(
            "SELECT r.effect FROM tool_selection_catalog c JOIN tool_selection_revisions r ON r.tool_id=c.id AND r.id=c.current_revision_id WHERE c.backend_key=?1 AND c.enabled=1 LIMIT 1",
            [backend_key],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| error.to_string())
}
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Epochs {
    pub catalog: i64,
    pub acl: i64,
    pub rule: i64,
}
#[derive(Clone, Debug)]
pub struct ToolRow {
    pub id: String,
    pub source_id: String,
    pub backend_key: String,
    pub current_revision_id: Option<String>,
    pub enabled: bool,
}
impl ToolRow {
    pub fn is_published(&self) -> bool {
        self.enabled && self.current_revision_id.is_some()
    }
}
#[derive(Clone, Debug)]
pub struct RevisionRow {
    pub id: String,
    pub tool_id: String,
    pub schema_hash: String,
    pub description_hash: String,
    pub input_schema: Value,
    pub output_schema: Option<Value>,
    pub search_text: String,
    pub operations: Vec<String>,
    pub objects: Vec<String>,
    pub effect: String,
    pub backend_binding: Value,
    pub created_at: i64,
}
/// An enabled, current revision visible to one principal/project pair.
#[derive(Clone, Debug)]
pub struct EligibleRevision {
    pub revision: RevisionRow,
    pub tool_enabled: bool,
}
pub struct NewTool<'a> {
    pub source_id: &'a str,
    pub tool_id: &'a str,
    pub backend_key: &'a str,
    pub enabled: bool,
}
pub struct NewRevision<'a> {
    pub revision_id: &'a str,
    pub tool_id: &'a str,
    pub schema_hash: &'a str,
    pub description_hash: &'a str,
    pub input_schema: &'a Value,
    pub output_schema: Option<&'a Value>,
    pub search_text: &'a str,
    pub operations: &'a [String],
    pub objects: &'a [String],
    pub effect: &'a str,
    pub backend_binding: &'a Value,
    pub created_at: i64,
}
pub struct NewFeedback<'a> {
    pub id: &'a str,
    pub principal_id: &'a str,
    pub message_id: &'a str,
    pub decision_id: Option<&'a str>,
    pub kind: &'a str,
    pub evidence_json: &'a Value,
    pub proposal_json: &'a Value,
    pub status: &'a str,
    pub idempotency_key: &'a str,
    pub created_at: i64,
}
pub struct NewRule<'a> {
    pub id: &'a str,
    pub feedback_id: &'a str,
    pub principal_id: &'a str,
    pub scope_kind: &'a str,
    pub scope_id: &'a str,
    pub operation: &'a str,
    pub object_type: &'a str,
    pub phase: Option<&'a str>,
    pub input_kind: Option<&'a str>,
    pub source_constraint: Option<&'a str>,
    pub target_tool_id: Option<&'a str>,
    pub target_revision_id: Option<&'a str>,
    pub preferred_tool_id: Option<&'a str>,
    pub action: &'a str,
    pub strength: f64,
    pub expires_at: Option<i64>,
    pub state: &'a str,
    pub created_at: i64,
}
pub fn epochs(connection: &Connection) -> rusqlite::Result<Epochs> {
    connection.query_row(
        "SELECT catalog_epoch, acl_epoch, rule_epoch FROM tool_selection_meta WHERE singleton = 1",
        [],
        |row| {
            Ok(Epochs {
                catalog: row.get(0)?,
                acl: row.get(1)?,
                rule: row.get(2)?,
            })
        },
    )
}
/// Advances the requested epochs. Ordinary decision/invocation writes never move an epoch.
pub fn bump_epochs(
    connection: &Connection,
    catalog: bool,
    acl: bool,
    rule: bool,
) -> rusqlite::Result<Epochs> {
    connection.execute(
        "UPDATE tool_selection_meta
            SET catalog_epoch = catalog_epoch + ?1,
                acl_epoch = acl_epoch + ?2,
                rule_epoch = rule_epoch + ?3
          WHERE singleton = 1",
        params![catalog as i64, acl as i64, rule as i64],
    )?;
    epochs(connection)
}
pub fn upsert_source(
    connection: &Connection,
    id: &str,
    kind: &str,
    owner_principal: &str,
    enabled: bool,
) -> rusqlite::Result<()> {
    connection.execute(
        "INSERT INTO tool_selection_sources(id, kind, owner_principal, enabled)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(id) DO UPDATE SET kind = excluded.kind,
           owner_principal = excluded.owner_principal, enabled = excluded.enabled",
        params![id, kind, owner_principal, enabled as i64],
    )?;
    Ok(())
}
pub fn upsert_tool(connection: &Connection, tool: &NewTool<'_>) -> rusqlite::Result<()> {
    connection.execute(
        "INSERT INTO tool_selection_catalog(id, source_id, backend_key, current_revision_id, enabled)
         VALUES (?1, ?2, ?3, NULL, ?4)
         ON CONFLICT(id) DO UPDATE SET source_id = excluded.source_id,
           backend_key = excluded.backend_key, enabled = excluded.enabled",
        params![
            tool.tool_id,
            tool.source_id,
            tool.backend_key,
            tool.enabled as i64
        ],
    )?;
    Ok(())
}
pub fn insert_revision(
    connection: &Connection,
    revision: &NewRevision<'_>,
) -> rusqlite::Result<()> {
    connection.execute(
        "INSERT INTO tool_selection_revisions(
           id, tool_id, schema_hash, description_hash, input_schema_json, output_schema_json,
           search_text, operations_json, objects_json, effect, backend_binding_json, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            revision.revision_id,
            revision.tool_id,
            revision.schema_hash,
            revision.description_hash,
            revision.input_schema.to_string(),
            revision.output_schema.map(|value| value.to_string()),
            revision.search_text,
            serde_json::to_string(revision.operations).unwrap_or_else(|_| "[]".into()),
            serde_json::to_string(revision.objects).unwrap_or_else(|_| "[]".into()),
            revision.effect,
            revision.backend_binding.to_string(),
            revision.created_at,
        ],
    )?;
    Ok(())
}
pub fn set_current_revision(
    connection: &Connection,
    tool_id: &str,
    revision_id: &str,
) -> rusqlite::Result<()> {
    connection.execute(
        "UPDATE tool_selection_catalog SET current_revision_id = ?1 WHERE id = ?2",
        params![revision_id, tool_id],
    )?;
    Ok(())
}
pub fn upsert_usage_page(
    connection: &Connection,
    revision_id: &str,
    section: &str,
    page: i64,
    text: &str,
) -> rusqlite::Result<()> {
    connection.execute(
        "INSERT INTO tool_selection_usage_pages(revision_id, section, page, text)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(revision_id, section, page) DO UPDATE SET text = excluded.text",
        params![revision_id, section, page, text],
    )?;
    Ok(())
}
pub fn upsert_usage_page_bounded(
    connection: &Connection,
    revision_id: &str,
    section: &str,
    page: i64,
    text: &str,
) -> rusqlite::Result<()> {
    let trimmed = truncate_utf8(text, USAGE_PAGE_MAX_BYTES);
    upsert_usage_page(connection, revision_id, section, page, trimmed)
}
pub fn upsert_grant(
    connection: &Connection,
    principal_id: &str,
    tool_id: &str,
    scope_kind: &str,
    scope_id: &str,
) -> rusqlite::Result<()> {
    connection.execute(
        "INSERT OR IGNORE INTO tool_selection_grants(principal_id, tool_id, scope_kind, scope_id)
         VALUES (?1, ?2, ?3, ?4)",
        params![principal_id, tool_id, scope_kind, scope_id],
    )?;
    Ok(())
}
pub fn upsert_fts(
    connection: &Connection,
    revision_id: &str,
    search_text: &str,
) -> rusqlite::Result<()> {
    connection.execute(
        "DELETE FROM tool_selection_fts WHERE revision_id = ?1",
        params![revision_id],
    )?;
    connection.execute(
        "INSERT INTO tool_selection_fts(revision_id, search_text) VALUES (?1, ?2)",
        params![revision_id, search_text],
    )?;
    Ok(())
}
pub fn delete_fts(connection: &Connection, revision_id: &str) -> rusqlite::Result<()> {
    connection.execute(
        "DELETE FROM tool_selection_fts WHERE revision_id = ?1",
        params![revision_id],
    )?;
    Ok(())
}
pub fn upsert_embedding(
    connection: &Connection,
    revision_id: &str,
    model_hash: &str,
    vector: &[f32],
) -> rusqlite::Result<()> {
    let mut bytes = Vec::with_capacity(vector.len() * 4);
    for value in vector {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    connection.execute(
        "INSERT INTO tool_selection_embeddings(revision_id, model_hash, dimension, vector)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(revision_id, model_hash) DO UPDATE SET
           dimension = excluded.dimension, vector = excluded.vector",
        params![revision_id, model_hash, vector.len() as i64, bytes],
    )?;
    Ok(())
}
pub fn set_source_enabled(
    connection: &Connection,
    id: &str,
    enabled: bool,
) -> rusqlite::Result<usize> {
    connection.execute(
        "UPDATE tool_selection_sources SET enabled = ?2 WHERE id = ?1",
        params![id, enabled as i64],
    )
}
pub fn tool_by_id(connection: &Connection, tool_id: &str) -> rusqlite::Result<Option<ToolRow>> {
    connection
        .query_row(
            "SELECT id, source_id, backend_key, current_revision_id, enabled
               FROM tool_selection_catalog WHERE id = ?1",
            params![tool_id],
            |row| {
                Ok(ToolRow {
                    id: row.get(0)?,
                    source_id: row.get(1)?,
                    backend_key: row.get(2)?,
                    current_revision_id: row.get(3)?,
                    enabled: row.get::<_, i64>(4)? == 1,
                })
            },
        )
        .optional()
}
pub fn tool_id_by_name(connection: &Connection, name: &str) -> rusqlite::Result<Option<String>> {
    // D0–D3 has a single in-process source; the display name is the backend key. A future
    // multi-source resolver must qualify with the source.
    connection
        .query_row(
            "SELECT id FROM tool_selection_catalog WHERE backend_key = ?1 LIMIT 1",
            params![name],
            |row| row.get(0),
        )
        .optional()
}
pub fn revision_by_id(
    connection: &Connection,
    revision_id: &str,
) -> rusqlite::Result<Option<RevisionRow>> {
    connection
        .query_row(
            "SELECT id, tool_id, schema_hash, description_hash, input_schema_json,
                    output_schema_json, search_text, operations_json, objects_json, effect,
                    backend_binding_json, created_at
               FROM tool_selection_revisions WHERE id = ?1",
            params![revision_id],
            revision_from_row,
        )
        .optional()
}
pub(super) fn revision_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RevisionRow> {
    let input_schema: String = row.get(4)?;
    let output_schema: Option<String> = row.get(5)?;
    let operations: String = row.get(7)?;
    let objects: String = row.get(8)?;
    let backend_binding: String = row.get(10)?;
    Ok(RevisionRow {
        id: row.get(0)?,
        tool_id: row.get(1)?,
        schema_hash: row.get(2)?,
        description_hash: row.get(3)?,
        input_schema: serde_json::from_str(&input_schema).unwrap_or(Value::Null),
        output_schema: output_schema.and_then(|text| serde_json::from_str(&text).ok()),
        search_text: row.get(6)?,
        operations: serde_json::from_str(&operations).unwrap_or_default(),
        objects: serde_json::from_str(&objects).unwrap_or_default(),
        effect: row.get(9)?,
        backend_binding: serde_json::from_str(&backend_binding).unwrap_or(Value::Null),
        created_at: row.get(11)?,
    })
}
/// Enabled, current revisions authorized for the principal/project pair. ACL is applied inside
/// this query so neither lexical nor vector top-K can see an unauthorized tool. For `mcp_http`
/// sources the last successful sync must also be within the freshness window.
pub fn eligible_revisions(
    connection: &Connection,
    principal_id: &str,
    project_id: Option<&str>,
    now: i64,
) -> rusqlite::Result<Vec<EligibleRevision>> {
    let stale_before = now - super::super::mcp::MCP_SOURCE_STALE_AFTER_MILLIS;
    let mut statement = connection.prepare(
        "SELECT r.id, r.tool_id, r.schema_hash, r.description_hash, r.input_schema_json,
                r.output_schema_json, r.search_text, r.operations_json, r.objects_json, r.effect,
                r.backend_binding_json, r.created_at, c.enabled
           FROM tool_selection_revisions r
           JOIN tool_selection_catalog c ON c.id = r.tool_id
           JOIN tool_selection_sources s ON s.id = c.source_id
          WHERE c.enabled = 1 AND s.enabled = 1 AND c.current_revision_id = r.id
            AND (s.kind <> 'mcp_http' OR EXISTS (
              SELECT 1 FROM tool_selection_mcp_sources ms
               WHERE ms.source_id = s.id AND ms.last_success_at IS NOT NULL
                 AND ms.last_success_at >= ?3))
            AND EXISTS (
              SELECT 1 FROM tool_selection_grants g
               WHERE g.principal_id = ?1 AND g.tool_id = c.id
                 AND ((g.scope_kind = 'user' AND g.scope_id = ?1)
                   OR (g.scope_kind = 'project' AND ?2 IS NOT NULL AND g.scope_id = ?2))
            )
          ORDER BY r.tool_id ASC, r.id ASC",
    )?;
    let rows = statement.query_map(params![principal_id, project_id, stale_before], |row| {
        Ok(EligibleRevision {
            revision: revision_from_row(row)?,
            tool_enabled: row.get::<_, i64>(12)? == 1,
        })
    })?;
    rows.collect()
}
/// Lexical top-K using FTS5 trigram BM25. `match_expression` must already be a safe MATCH
/// expression built by `retrieval::fts_match_query`; `order by bm25` is ascending (lower is
/// better).
pub fn lexical_candidates(
    connection: &Connection,
    match_expression: &str,
    principal_id: &str,
    project_id: Option<&str>,
    limit: usize,
    now: i64,
) -> rusqlite::Result<Vec<(String, f64)>> {
    let stale_before = now - super::super::mcp::MCP_SOURCE_STALE_AFTER_MILLIS;
    let mut statement = connection.prepare(
        "SELECT f.revision_id, bm25(tool_selection_fts) AS score
           FROM tool_selection_fts f
           JOIN tool_selection_revisions r ON r.id = f.revision_id
           JOIN tool_selection_catalog c ON c.id = r.tool_id
           JOIN tool_selection_sources s ON s.id = c.source_id
          WHERE f.search_text MATCH ?1
            AND c.enabled = 1 AND s.enabled = 1 AND c.current_revision_id = r.id
            AND (s.kind <> 'mcp_http' OR EXISTS (
              SELECT 1 FROM tool_selection_mcp_sources ms
               WHERE ms.source_id = s.id AND ms.last_success_at IS NOT NULL
                 AND ms.last_success_at >= ?5))
            AND EXISTS (
              SELECT 1 FROM tool_selection_grants g
               WHERE g.principal_id = ?2 AND g.tool_id = c.id
                 AND ((g.scope_kind = 'user' AND g.scope_id = ?2)
                   OR (g.scope_kind = 'project' AND ?3 IS NOT NULL AND g.scope_id = ?3))
            )
          ORDER BY score ASC, f.revision_id ASC
          LIMIT ?4",
    )?;
    let rows = statement.query_map(
        params![
            match_expression,
            principal_id,
            project_id,
            limit as i64,
            stale_before
        ],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?)),
    )?;
    rows.collect()
}
pub fn load_embeddings(
    connection: &Connection,
    model_hash: &str,
    principal_id: &str,
    project_id: Option<&str>,
    now: i64,
) -> rusqlite::Result<Vec<(String, Vec<f32>)>> {
    let stale_before = now - super::super::mcp::MCP_SOURCE_STALE_AFTER_MILLIS;
    let mut statement = connection.prepare(
        "SELECT e.revision_id, e.dimension, e.vector
           FROM tool_selection_embeddings e
           JOIN tool_selection_revisions r ON r.id = e.revision_id
           JOIN tool_selection_catalog c ON c.id = r.tool_id
           JOIN tool_selection_sources s ON s.id = c.source_id
          WHERE e.model_hash = ?1 AND c.enabled = 1 AND s.enabled = 1
            AND c.current_revision_id = r.id
            AND (s.kind <> 'mcp_http' OR EXISTS (
              SELECT 1 FROM tool_selection_mcp_sources ms
               WHERE ms.source_id = s.id AND ms.last_success_at IS NOT NULL
                 AND ms.last_success_at >= ?4))
            AND EXISTS (
              SELECT 1 FROM tool_selection_grants g
               WHERE g.principal_id = ?2 AND g.tool_id = c.id
                 AND ((g.scope_kind = 'user' AND g.scope_id = ?2)
                   OR (g.scope_kind = 'project' AND ?3 IS NOT NULL AND g.scope_id = ?3))
            )",
    )?;
    let rows = statement.query_map(
        params![model_hash, principal_id, project_id, stale_before],
        |row| {
            let dimension: i64 = row.get(1)?;
            let bytes: Vec<u8> = row.get(2)?;
            let mut vector = Vec::with_capacity(dimension as usize);
            for chunk in bytes.chunks_exact(4) {
                vector.push(f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
            }
            Ok((row.get::<_, String>(0)?, vector))
        },
    )?;
    rows.collect()
}
