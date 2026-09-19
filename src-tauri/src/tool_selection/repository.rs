//! SQL access for the tool-selection ledger. Every function takes a `&Connection` so it can run
//! inside the existing `SqliteWriter` transaction; there is no second connection pool.

use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;

use super::contracts::*;

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

/// Resolves a tool by source + backend key, the same identity rule used for L-Lang bindings.
pub fn tool_by_backend_key(
    connection: &Connection,
    source_id: &str,
    backend_key: &str,
) -> rusqlite::Result<Option<ToolRow>> {
    connection
        .query_row(
            "SELECT id, source_id, backend_key, current_revision_id, enabled
               FROM tool_selection_catalog WHERE source_id = ?1 AND backend_key = ?2",
            params![source_id, backend_key],
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

fn revision_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RevisionRow> {
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
/// this query so neither lexical nor vector top-K can see an unauthorized tool.
pub fn eligible_revisions(
    connection: &Connection,
    principal_id: &str,
    project_id: Option<&str>,
) -> rusqlite::Result<Vec<EligibleRevision>> {
    let mut statement = connection.prepare(
        "SELECT r.id, r.tool_id, r.schema_hash, r.description_hash, r.input_schema_json,
                r.output_schema_json, r.search_text, r.operations_json, r.objects_json, r.effect,
                r.backend_binding_json, r.created_at, c.enabled
           FROM tool_selection_revisions r
           JOIN tool_selection_catalog c ON c.id = r.tool_id
           JOIN tool_selection_sources s ON s.id = c.source_id
          WHERE c.enabled = 1 AND s.enabled = 1 AND c.current_revision_id = r.id
            AND EXISTS (
              SELECT 1 FROM tool_selection_grants g
               WHERE g.principal_id = ?1 AND g.tool_id = c.id
                 AND ((g.scope_kind = 'user' AND g.scope_id = ?1)
                   OR (g.scope_kind = 'project' AND ?2 IS NOT NULL AND g.scope_id = ?2))
            )
          ORDER BY r.tool_id ASC, r.id ASC",
    )?;
    let rows = statement.query_map(params![principal_id, project_id], |row| {
        Ok(EligibleRevision {
            revision: revision_from_row(row)?,
            tool_enabled: row.get::<_, i64>(12)? == 1,
        })
    })?;
    rows.collect()
}

/// Lexical top-K using FTS5 trigram BM25. `order by bm25` is ascending (lower is better).
pub fn lexical_candidates(
    connection: &Connection,
    query: &str,
    principal_id: &str,
    project_id: Option<&str>,
    limit: usize,
) -> rusqlite::Result<Vec<(String, f64)>> {
    let escaped = format!("\"{}\"", query.replace('"', "\"\""));
    let mut statement = connection.prepare(
        "SELECT f.revision_id, bm25(tool_selection_fts) AS score
           FROM tool_selection_fts f
           JOIN tool_selection_revisions r ON r.id = f.revision_id
           JOIN tool_selection_catalog c ON c.id = r.tool_id
           JOIN tool_selection_sources s ON s.id = c.source_id
          WHERE f.search_text MATCH ?1
            AND c.enabled = 1 AND s.enabled = 1 AND c.current_revision_id = r.id
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
        params![escaped, principal_id, project_id, limit as i64],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?)),
    )?;
    rows.collect()
}

pub fn load_embeddings(
    connection: &Connection,
    model_hash: &str,
    principal_id: &str,
    project_id: Option<&str>,
) -> rusqlite::Result<Vec<(String, Vec<f32>)>> {
    let mut statement = connection.prepare(
        "SELECT e.revision_id, e.dimension, e.vector
           FROM tool_selection_embeddings e
           JOIN tool_selection_revisions r ON r.id = e.revision_id
           JOIN tool_selection_catalog c ON c.id = r.tool_id
           JOIN tool_selection_sources s ON s.id = c.source_id
          WHERE e.model_hash = ?1 AND c.enabled = 1 AND s.enabled = 1
            AND c.current_revision_id = r.id
            AND EXISTS (
              SELECT 1 FROM tool_selection_grants g
               WHERE g.principal_id = ?2 AND g.tool_id = c.id
                 AND ((g.scope_kind = 'user' AND g.scope_id = ?2)
                   OR (g.scope_kind = 'project' AND ?3 IS NOT NULL AND g.scope_id = ?3))
            )",
    )?;
    let rows = statement.query_map(params![model_hash, principal_id, project_id], |row| {
        let dimension: i64 = row.get(1)?;
        let bytes: Vec<u8> = row.get(2)?;
        let mut vector = Vec::with_capacity(dimension as usize);
        for chunk in bytes.chunks_exact(4) {
            vector.push(f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
        }
        Ok((row.get::<_, String>(0)?, vector))
    })?;
    rows.collect()
}

pub fn embedding_model_hash(
    connection: &Connection,
    revision_id: &str,
) -> rusqlite::Result<Option<String>> {
    connection
        .query_row(
            "SELECT model_hash FROM tool_selection_embeddings WHERE revision_id = ?1",
            params![revision_id],
            |row| row.get(0),
        )
        .optional()
}

pub fn insert_decision(connection: &Connection, decision: &DecisionRecord) -> rusqlite::Result<()> {
    connection.execute(
        "INSERT INTO tool_selection_decisions(
           id, principal_id, conversation_id, run_id, message_id, scenario_json, catalog_epoch,
           acl_epoch, rule_epoch, model_hash, status, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            decision.id,
            decision.principal_id,
            decision.conversation_id,
            decision.run_id,
            decision.message_id,
            serde_json::to_string(&decision.scenario).unwrap_or_else(|_| "{}".into()),
            decision.catalog_epoch,
            decision.acl_epoch,
            decision.rule_epoch,
            decision.model_hash,
            decision.status.as_str(),
            decision.created_at,
        ],
    )?;
    for candidate in &decision.candidates {
        connection.execute(
            "INSERT INTO tool_selection_candidates(
               decision_id, revision_id, lex_rank, vec_rank, raw_score, base_score, final_score,
               rule_ids_json, final_rank)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                decision.id,
                candidate.revision_id,
                candidate.lex_rank,
                candidate.vec_rank,
                candidate.raw_score,
                candidate.base_score,
                candidate.final_score,
                serde_json::to_string(&candidate.rule_ids).unwrap_or_else(|_| "[]".into()),
                candidate.final_rank,
            ],
        )?;
    }
    Ok(())
}

pub fn decision_by_id(
    connection: &Connection,
    decision_id: &str,
) -> rusqlite::Result<Option<DecisionRecord>> {
    let header = connection
        .query_row(
            "SELECT id, principal_id, conversation_id, run_id, message_id, scenario_json,
                    catalog_epoch, acl_epoch, rule_epoch, model_hash, status, created_at
               FROM tool_selection_decisions WHERE id = ?1",
            params![decision_id],
            |row| {
                let scenario: String = row.get(5)?;
                let status: String = row.get(10)?;
                Ok(DecisionRecord {
                    id: row.get(0)?,
                    principal_id: row.get(1)?,
                    conversation_id: row.get(2)?,
                    run_id: row.get(3)?,
                    message_id: row.get(4)?,
                    scenario: serde_json::from_str(&scenario)
                        .unwrap_or_else(|_| Scenario::degraded("")),
                    catalog_epoch: row.get(6)?,
                    acl_epoch: row.get(7)?,
                    rule_epoch: row.get(8)?,
                    model_hash: row.get(9)?,
                    status: DecisionStatus::parse(&status).unwrap_or(DecisionStatus::Degraded),
                    created_at: row.get(11)?,
                    candidates: Vec::new(),
                })
            },
        )
        .optional()?;
    let Some(mut decision) = header else {
        return Ok(None);
    };
    let mut statement = connection.prepare(
        "SELECT revision_id, lex_rank, vec_rank, raw_score, base_score, final_score, rule_ids_json,
                final_rank
           FROM tool_selection_candidates WHERE decision_id = ?1 ORDER BY final_rank ASC, revision_id ASC",
    )?;
    let rows = statement.query_map(params![decision_id], |row| {
        let rules: String = row.get(6)?;
        Ok(CandidateRecord {
            revision_id: row.get(0)?,
            tool_id: String::new(),
            lex_rank: row.get(1)?,
            vec_rank: row.get(2)?,
            raw_score: row.get(3)?,
            base_score: row.get(4)?,
            final_score: row.get(5)?,
            rule_ids: serde_json::from_str(&rules).unwrap_or_default(),
            final_rank: row.get(7)?,
        })
    })?;
    for row in rows {
        decision.candidates.push(row?);
    }
    // Fill in tool IDs from catalog so downstream code does not re-resolve revisions.
    for candidate in &mut decision.candidates {
        if let Some(revision) = revision_by_id(connection, &candidate.revision_id)? {
            candidate.tool_id = revision.tool_id;
        }
    }
    Ok(Some(decision))
}

pub fn insert_invocation(
    connection: &Connection,
    invocation_id: &str,
    decision_id: Option<&str>,
    revision_id: &str,
    started_at: i64,
) -> rusqlite::Result<()> {
    connection.execute(
        "INSERT INTO tool_selection_invocations(
           id, decision_id, revision_id, backend_call_id, technical_status, satisfaction,
           started_at, finished_at, error_code)
         VALUES (?1, ?2, ?3, NULL, 'running', 'unknown', ?4, NULL, NULL)",
        params![invocation_id, decision_id, revision_id, started_at],
    )?;
    Ok(())
}

pub fn finish_invocation(
    connection: &Connection,
    invocation_id: &str,
    status: &str,
    error_code: Option<&str>,
    finished_at: i64,
) -> rusqlite::Result<()> {
    connection.execute(
        "UPDATE tool_selection_invocations
            SET technical_status = ?2, error_code = ?3, finished_at = ?4
          WHERE id = ?1",
        params![invocation_id, status, error_code, finished_at],
    )?;
    Ok(())
}

pub fn set_satisfaction_for_decision(
    connection: &Connection,
    decision_id: &str,
    satisfaction: &str,
) -> rusqlite::Result<usize> {
    connection.execute(
        "UPDATE tool_selection_invocations SET satisfaction = ?2 WHERE decision_id = ?1",
        params![decision_id, satisfaction],
    )
}

pub fn invocation_decision_id(
    connection: &Connection,
    invocation_id: &str,
) -> rusqlite::Result<Option<Option<String>>> {
    connection
        .query_row(
            "SELECT decision_id FROM tool_selection_invocations WHERE id = ?1",
            params![invocation_id],
            |row| row.get(0),
        )
        .optional()
}

pub fn recent_invocations(
    connection: &Connection,
    conversation_id: &str,
    limit: usize,
) -> rusqlite::Result<Vec<(String, String, Option<String>)>> {
    let mut statement = connection.prepare(
        "SELECT i.id, i.revision_id, i.decision_id
           FROM tool_selection_invocations i
           JOIN tool_selection_decisions d ON d.id = i.decision_id
          WHERE d.conversation_id = ?1
          ORDER BY i.started_at DESC, i.id DESC
          LIMIT ?2",
    )?;
    let rows = statement.query_map(params![conversation_id, limit as i64], |row| {
        Ok((row.get(0)?, row.get(1)?, row.get(2)?))
    })?;
    rows.collect()
}

/// Inserts feedback unless the idempotency key already exists. Returns `false` when a duplicate
/// was found so the caller can replay the previous result without moving the rule epoch.
pub fn insert_feedback_if_absent(
    connection: &Connection,
    feedback: &NewFeedback<'_>,
) -> rusqlite::Result<bool> {
    let changed = connection.execute(
        "INSERT OR IGNORE INTO tool_selection_feedback(
           id, principal_id, message_id, decision_id, kind, evidence_json, proposal_json, status,
           idempotency_key, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            feedback.id,
            feedback.principal_id,
            feedback.message_id,
            feedback.decision_id,
            feedback.kind,
            feedback.evidence_json.to_string(),
            feedback.proposal_json.to_string(),
            feedback.status,
            feedback.idempotency_key,
            feedback.created_at,
        ],
    )?;
    Ok(changed == 1)
}

pub fn feedback_by_idempotency(
    connection: &Connection,
    idempotency_key: &str,
) -> rusqlite::Result<Option<(String, String)>> {
    connection
        .query_row(
            "SELECT id, status FROM tool_selection_feedback WHERE idempotency_key = ?1",
            params![idempotency_key],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
}

pub fn update_feedback_status(
    connection: &Connection,
    feedback_id: &str,
    status: &str,
) -> rusqlite::Result<()> {
    connection.execute(
        "UPDATE tool_selection_feedback SET status = ?2 WHERE id = ?1",
        params![feedback_id, status],
    )?;
    Ok(())
}

pub fn insert_rule(connection: &Connection, rule: &NewRule<'_>) -> rusqlite::Result<()> {
    connection.execute(
        "INSERT INTO tool_selection_rules(
           id, feedback_id, principal_id, scope_kind, scope_id, operation, object_type, phase,
           input_kind, source_constraint, target_tool_id, target_revision_id, preferred_tool_id,
           action, strength, expires_at, state, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
        params![
            rule.id,
            rule.feedback_id,
            rule.principal_id,
            rule.scope_kind,
            rule.scope_id,
            rule.operation,
            rule.object_type,
            rule.phase,
            rule.input_kind,
            rule.source_constraint,
            rule.target_tool_id,
            rule.target_revision_id,
            rule.preferred_tool_id,
            rule.action,
            rule.strength,
            rule.expires_at,
            rule.state,
            rule.created_at,
        ],
    )?;
    Ok(())
}

/// Marks older soft rules for the same target and condition as superseded, so a repeated
/// correction does not accumulate and shift the score twice.
pub fn supersede_soft_rules(
    connection: &Connection,
    target_tool_id: &str,
    action: &str,
    condition: &FeedbackCondition,
    scope_kind: &str,
    scope_id: &str,
    keep_rule_id: &str,
) -> rusqlite::Result<usize> {
    let updated = connection.execute(
        "UPDATE tool_selection_rules SET state = 'superseded'
          WHERE state = 'active' AND action = ?1 AND target_tool_id = ?2
            AND scope_kind = ?3 AND scope_id = ?4
            AND operation = ?5 AND object_type = ?6
            AND phase IS ?7 AND input_kind IS ?8
            AND id <> ?9",
        params![
            action,
            target_tool_id,
            scope_kind,
            scope_id,
            condition
                .operation
                .map(Operation::as_str)
                .unwrap_or("unknown"),
            condition
                .object_type
                .map(ObjectType::as_str)
                .unwrap_or("unknown"),
            condition.phase.map(Phase::as_str),
            condition.input_kind.map(InputKind::as_str),
            keep_rule_id,
        ],
    )?;
    Ok(updated)
}

pub fn active_rules(
    connection: &Connection,
    principal_id: &str,
    conversation_id: &str,
    project_id: Option<&str>,
    task_id: Option<&str>,
    now: i64,
) -> rusqlite::Result<Vec<StoredRule>> {
    let mut statement = connection.prepare(
        "SELECT id, principal_id, scope_kind, scope_id, operation, object_type, phase, input_kind,
                source_constraint, target_tool_id, target_revision_id, preferred_tool_id, action,
                strength, state, created_at
           FROM tool_selection_rules
          WHERE principal_id = ?1 AND state = 'active'
            AND (expires_at IS NULL OR expires_at > ?2)
            AND (
              (scope_kind = 'user' AND scope_id = ?1)
              OR (scope_kind = 'conversation' AND scope_id = ?3)
              OR (scope_kind = 'project' AND ?4 IS NOT NULL AND scope_id = ?4)
              OR (scope_kind = 'task' AND ?5 IS NOT NULL AND scope_id = ?5)
            )
          ORDER BY strength DESC, id ASC",
    )?;
    let rows = statement.query_map(
        params![principal_id, now, conversation_id, project_id, task_id],
        |row| {
            let action: String = row.get(12)?;
            let state: String = row.get(14)?;
            Ok(StoredRule {
                id: row.get(0)?,
                principal_id: row.get(1)?,
                scope_kind: ScopeKind::parse(&row.get::<_, String>(2)?).unwrap_or(ScopeKind::User),
                scope_id: row.get(3)?,
                operation: row.get(4)?,
                object_type: row.get(5)?,
                phase: row.get(6)?,
                input_kind: row.get(7)?,
                source_constraint: row.get(8)?,
                target_tool_id: row.get(9)?,
                target_revision_id: row.get(10)?,
                preferred_tool_id: row.get(11)?,
                action: match action.as_str() {
                    "prefer" => RuleAction::Prefer,
                    "pairwise" => RuleAction::Pairwise,
                    "forbid" => RuleAction::Forbid,
                    _ => RuleAction::Avoid,
                },
                strength: row.get(13)?,
                state: match state.as_str() {
                    "revoked" => RuleState::Revoked,
                    "superseded" => RuleState::Superseded,
                    _ => RuleState::Active,
                },
                created_at: row.get(15)?,
            })
        },
    )?;
    rows.collect()
}

pub fn rule_by_id(connection: &Connection, rule_id: &str) -> rusqlite::Result<Option<StoredRule>> {
    let mut statement = connection.prepare(
        "SELECT id, principal_id, scope_kind, scope_id, operation, object_type, phase, input_kind,
                source_constraint, target_tool_id, target_revision_id, preferred_tool_id, action,
                strength, state, created_at
           FROM tool_selection_rules WHERE id = ?1",
    )?;
    let mut rows = statement.query(params![rule_id])?;
    let Some(row) = rows.next()? else {
        return Ok(None);
    };
    let action: String = row.get(12)?;
    let state: String = row.get(14)?;
    Ok(Some(StoredRule {
        id: row.get(0)?,
        principal_id: row.get(1)?,
        scope_kind: ScopeKind::parse(&row.get::<_, String>(2)?).unwrap_or(ScopeKind::User),
        scope_id: row.get(3)?,
        operation: row.get(4)?,
        object_type: row.get(5)?,
        phase: row.get(6)?,
        input_kind: row.get(7)?,
        source_constraint: row.get(8)?,
        target_tool_id: row.get(9)?,
        target_revision_id: row.get(10)?,
        preferred_tool_id: row.get(11)?,
        action: match action.as_str() {
            "prefer" => RuleAction::Prefer,
            "pairwise" => RuleAction::Pairwise,
            "forbid" => RuleAction::Forbid,
            _ => RuleAction::Avoid,
        },
        strength: row.get(13)?,
        state: match state.as_str() {
            "revoked" => RuleState::Revoked,
            "superseded" => RuleState::Superseded,
            _ => RuleState::Active,
        },
        created_at: row.get(15)?,
    }))
}

pub fn revoke_rule(connection: &Connection, rule_id: &str) -> rusqlite::Result<usize> {
    connection.execute(
        "UPDATE tool_selection_rules SET state = 'revoked' WHERE id = ?1 AND state = 'active'",
        params![rule_id],
    )
}

/// Active soft rules for the same principal and a matching explicit revoke scope. Returns the
/// tool IDs to release when a user says "the previous instruction is withdrawn".
pub fn revoke_matching_soft_rules(
    connection: &Connection,
    principal_id: &str,
    scope_kind: &str,
    scope_id: &str,
    target_tool_id: Option<&str>,
) -> rusqlite::Result<usize> {
    connection.execute(
        "UPDATE tool_selection_rules SET state = 'revoked'
          WHERE principal_id = ?1 AND state = 'active'
            AND action IN ('avoid', 'prefer', 'pairwise')
            AND scope_kind = ?2 AND scope_id = ?3
            AND (?4 IS NULL OR target_tool_id = ?4)",
        params![principal_id, scope_kind, scope_id, target_tool_id],
    )
}

pub fn grant_exists(
    connection: &Connection,
    principal_id: &str,
    tool_id: &str,
    project_id: Option<&str>,
) -> rusqlite::Result<bool> {
    connection
        .query_row(
            "SELECT EXISTS(
           SELECT 1 FROM tool_selection_grants g
            WHERE g.principal_id = ?1 AND g.tool_id = ?2
              AND ((g.scope_kind = 'user' AND g.scope_id = ?1)
                OR (g.scope_kind = 'project' AND ?3 IS NOT NULL AND g.scope_id = ?3)))",
            params![principal_id, tool_id, project_id],
            |row| row.get::<_, i64>(0),
        )
        .map(|value| value == 1)
}

pub fn usage_page(
    connection: &Connection,
    revision_id: &str,
    section: &str,
    page: i64,
) -> rusqlite::Result<Option<String>> {
    connection
        .query_row(
            "SELECT text FROM tool_selection_usage_pages
              WHERE revision_id = ?1 AND section = ?2 AND page = ?3",
            params![revision_id, section, page],
            |row| row.get(0),
        )
        .optional()
}

pub fn usage_page_count(
    connection: &Connection,
    revision_id: &str,
    section: &str,
) -> rusqlite::Result<i64> {
    connection.query_row(
        "SELECT COUNT(*) FROM tool_selection_usage_pages
          WHERE revision_id = ?1 AND section = ?2",
        params![revision_id, section],
        |row| row.get(0),
    )
}

pub fn recent_decisions(
    connection: &Connection,
    principal_id: &str,
    conversation_id: &str,
    limit: usize,
) -> rusqlite::Result<Vec<(String, String, Vec<String>)>> {
    let mut statement = connection.prepare(
        "SELECT d.id, d.scenario_json,
                (SELECT group_concat(c.tool_id, ',') FROM tool_selection_candidates c
                  WHERE c.decision_id = d.id)
           FROM tool_selection_decisions d
          WHERE d.principal_id = ?1 AND d.conversation_id = ?2
          ORDER BY d.created_at DESC, d.id DESC
          LIMIT ?3",
    )?;
    let rows = statement.query_map(
        params![principal_id, conversation_id, limit as i64],
        |row| {
            let scenario: String = row.get(1)?;
            let tools: Option<String> = row.get(2)?;
            Ok((
                row.get::<_, String>(0)?,
                scenario,
                tools
                    .unwrap_or_default()
                    .split(',')
                    .filter(|value| !value.is_empty())
                    .map(str::to_string)
                    .collect(),
            ))
        },
    )?;
    rows.collect()
}

pub fn catalog_count(connection: &Connection) -> rusqlite::Result<i64> {
    connection.query_row("SELECT COUNT(*) FROM tool_selection_catalog", [], |row| {
        row.get(0)
    })
}

pub fn truncate_utf8(text: &str, max_bytes: usize) -> &str {
    if text.len() <= max_bytes {
        return text;
    }
    let mut end = max_bytes;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}
