use super::*;
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
/// The current endpoint hash to bind into a new correction rule for this tool, or `None` when the
/// tool is not a remote MCP tool. An MCP tool whose source has no endpoint yet returns the empty
/// string, which keeps a rule learned while the source is unconfirmed inapplicable until a new
/// correction is recorded against a confirmed endpoint.
pub fn source_binding_hash(
    connection: &Connection,
    tool_id: &str,
) -> rusqlite::Result<Option<String>> {
    connection
        .query_row(
            "SELECT s.kind, m.endpoint_hash
               FROM tool_selection_catalog c
               JOIN tool_selection_sources s ON s.id = c.source_id
               LEFT JOIN tool_selection_mcp_sources m ON m.source_id = c.source_id
              WHERE c.id = ?1",
            params![tool_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
        )
        .optional()
        .map(|row| match row {
            Some((kind, hash)) if kind == "mcp_http" => Some(hash.unwrap_or_default()),
            _ => None,
        })
}
/// Records the endpoint a correction rule was learned on for one referenced remote tool.
pub fn insert_rule_source_binding(
    connection: &Connection,
    rule_id: &str,
    tool_id: &str,
    endpoint_hash: &str,
) -> rusqlite::Result<()> {
    connection.execute(
        "INSERT OR REPLACE INTO tool_selection_rule_source_bindings(rule_id, tool_id, endpoint_hash)
         VALUES (?1, ?2, ?3)",
        params![rule_id, tool_id, endpoint_hash],
    )?;
    Ok(())
}
/// Clears the endpoint of every binding for the tools of one source, so once a source endpoint
/// changes its learned rules stay unconfirmed even if the original URL is configured again.
/// Returns the number of bindings invalidated.
pub fn invalidate_source_rule_bindings(
    connection: &Connection,
    source_id: &str,
) -> rusqlite::Result<usize> {
    connection.execute(
        "UPDATE tool_selection_rule_source_bindings SET endpoint_hash = ''
          WHERE endpoint_hash <> ''
            AND tool_id IN (SELECT id FROM tool_selection_catalog WHERE source_id = ?1)",
        params![source_id],
    )
}
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
            AND NOT EXISTS (
              SELECT 1 FROM tool_selection_rule_source_bindings b
                LEFT JOIN tool_selection_catalog c ON c.id = b.tool_id
                LEFT JOIN tool_selection_mcp_sources s ON s.source_id = c.source_id
               WHERE b.rule_id = tool_selection_rules.id
                 AND (b.endpoint_hash = '' OR s.endpoint_hash IS NULL
                      OR s.endpoint_hash <> b.endpoint_hash)
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
/// Active rules for the same principal and a matching explicit revoke scope. Explicit revoke is
/// the only way to lift a forbid, so forbids are included here.
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
            AND action IN ('avoid', 'prefer', 'pairwise', 'forbid')
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
/// Exact-tuple grant check used when deciding whether a config-declared grant is new or was
/// already created by another management path.
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
pub fn recent_decisions(
    connection: &Connection,
    principal_id: &str,
    conversation_id: &str,
    limit: usize,
) -> rusqlite::Result<Vec<(String, String, Vec<String>)>> {
    let mut statement = connection.prepare(
        "SELECT d.id, d.scenario_json,
                (SELECT group_concat(r.tool_id, ',')
                   FROM tool_selection_candidates c
                   JOIN tool_selection_revisions r ON r.id = c.revision_id
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
