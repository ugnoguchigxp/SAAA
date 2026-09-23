use super::*;
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedScope {
    pub(crate) key: String,
    pub(crate) kind: String,
    pub(crate) relation: String,
    pub(crate) epoch: u64,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ScopeSnapshot {
    pub(crate) status: String,
    pub(crate) focus_scope_key: Option<String>,
    pub(crate) digest: String,
    pub(crate) reason_code: Option<String>,
    pub(crate) scopes: Vec<ResolvedScope>,
}
impl ScopeSnapshot {
    pub(crate) fn is_user_only(&self) -> bool {
        self.scopes
            .iter()
            .filter(|scope| scope.relation == "focus")
            .all(|scope| scope.kind == "user")
    }

    pub(crate) fn keys_json(&self) -> Result<String, String> {
        serde_json::to_string(
            &self
                .scopes
                .iter()
                .map(|scope| scope.key.as_str())
                .collect::<Vec<_>>(),
        )
        .map_err(|_| "Context scope snapshot could not be encoded".to_string())
    }
}
pub(crate) fn resolve(
    connection: &Connection,
    input: &StartTurnInput,
    message_id: &str,
    new_message: bool,
) -> Result<ScopeSnapshot, String> {
    let principal: String = connection
        .query_row(
            "SELECT principal FROM personal_scope WHERE id='primary'",
            [],
            |row| row.get(0),
        )
        .map_err(database_error)?;
    let user_key = ensure_scope(connection, "user", &principal)?;
    let request_key = ensure_scope(connection, "request", message_id)?;
    let mut resolved = vec![ResolvedScope {
        key: user_key.clone(),
        kind: "user".into(),
        relation: "shared".into(),
        epoch: epoch(connection, &user_key)?,
    }];
    let mut missing = false;
    let mut explicit_keys = BTreeSet::new();
    for reference in &input.scope_refs {
        if !matches!(
            reference.kind.as_str(),
            "user" | "project" | "task" | "resource" | "request"
        ) || !matches!(
            reference.relation.as_str(),
            "shared" | "parent" | "focus" | "current"
        ) {
            return Err("Context scope kind or relation is invalid".into());
        }
        crate::validate_identifier(&reference.id, "context scope id")?;
        let expected = format!("{}:{}", reference.kind, reference.id);
        let allowed_runtime = (reference.kind == "user" && expected == user_key)
            || (reference.kind == "request" && expected == request_key);
        let exists: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM context_scopes
                 WHERE scope_key=?1 AND kind=?2 AND opaque_id=?3 AND state='active')",
                params![expected, reference.kind, reference.id],
                |row| row.get(0),
            )
            .map_err(database_error)?;
        if !exists && !allowed_runtime {
            missing = true;
            continue;
        }
        if explicit_keys.insert((expected.clone(), reference.relation.clone()))
            && expected != user_key
        {
            resolved.push(ResolvedScope {
                key: expected.clone(),
                kind: reference.kind.clone(),
                relation: reference.relation.clone(),
                epoch: epoch(connection, &expected)?,
            });
        }
    }
    // A registered coding workspace is an explicit user selection made in the normal UI. Use its
    // durable Project → resource → active Task links only when this turn did not name a competing
    // scope. No text similarity or title matching participates in this resolution.
    if input.scope_refs.is_empty() {
        append_registered_coding_scope(connection, &input.conversation_id, &mut resolved)?;
    }
    let local = resolved
        .iter()
        .filter(|scope| matches!(scope.kind.as_str(), "project" | "task" | "resource"))
        .collect::<Vec<_>>();
    let explicit_focus = local
        .iter()
        .filter(|scope| scope.relation == "focus")
        .collect::<Vec<_>>();
    let ambiguous = explicit_focus.len() > 1
        || (local.len() > 1 && explicit_focus.len() != 1)
        || explicit_focus.first().is_some_and(|focus| {
            local
                .iter()
                .any(|scope| scope.key != focus.key && !linked(connection, &focus.key, &scope.key))
        });
    let focus_key = if missing || ambiguous {
        None
    } else if let Some(focus) = explicit_focus.first() {
        Some(focus.key.clone())
    } else if let Some(single) = local.first() {
        Some(single.key.clone())
    } else {
        Some(user_key.clone())
    };
    if let Some(focus) = &focus_key {
        if focus == &user_key {
            resolved[0].relation = "focus".into();
        } else if let Some(scope) = resolved.iter_mut().find(|scope| &scope.key == focus) {
            scope.relation = "focus".into();
        }
    }
    resolved.push(ResolvedScope {
        key: request_key.clone(),
        kind: "request".into(),
        relation: "current".into(),
        epoch: epoch(connection, &request_key)?,
    });
    let status = if missing {
        "missing"
    } else if ambiguous {
        "ambiguous"
    } else {
        "resolved"
    };
    let reason = match status {
        "missing" => Some("scope-not-registered"),
        "ambiguous" => Some("scope-focus-ambiguous"),
        _ => None,
    };
    if new_message && status == "resolved" {
        let changed_key = focus_key.as_deref().unwrap_or(&user_key);
        bump_epoch(connection, changed_key)?;
        bump_epoch(connection, &request_key)?;
        for scope in &mut resolved {
            scope.epoch = epoch(connection, &scope.key)?;
        }
    }
    let digest = snapshot_digest(status, focus_key.as_deref(), &resolved);
    connection
        .execute(
            "INSERT INTO runtime_scope_resolutions(
               run_id,status,focus_scope_key,scope_digest,reason_code,resolved_at
             ) VALUES(?1,?2,?3,?4,?5,?6)",
            params![input.run_id, status, focus_key, digest, reason, now_iso()],
        )
        .map_err(database_error)?;
    for scope in &resolved {
        connection
            .execute(
                "INSERT INTO runtime_run_scopes(run_id,scope_key,relation,source,epoch)
                 VALUES(?1,?2,?3,?4,?5)",
                params![
                    input.run_id,
                    scope.key,
                    scope.relation,
                    if input.scope_refs.is_empty() {
                        "default"
                    } else {
                        "explicit"
                    },
                    scope.epoch
                ],
            )
            .map_err(database_error)?;
        if focus_key.as_deref() == Some(user_key.as_str()) || scope.relation != "shared" {
            connection
                .execute(
                    "INSERT OR IGNORE INTO conversation_message_scopes(message_id,scope_key,relation)
                     VALUES(?1,?2,?3)",
                    params![message_id, scope.key, scope.relation],
                )
                .map_err(database_error)?;
        }
    }
    connection
        .execute(
            "INSERT OR IGNORE INTO personal_source_scope_refs(source_id,version,scope_key)
             SELECT p.message_id,p.version,m.scope_key
             FROM personal_sources p JOIN conversation_message_scopes m ON m.message_id=p.message_id
             WHERE p.message_id=?1",
            [message_id],
        )
        .map_err(database_error)?;
    if let Some(focus) = focus_key.as_deref() {
        connection
            .execute(
                "UPDATE personal_jobs
                 SET scope_key=?2,claim_scope_epoch=(SELECT epoch FROM context_scope_epochs WHERE scope_key=?2)
                 WHERE source_sequence IN (
                   SELECT sequence FROM personal_sources WHERE message_id=?1 AND available=1
                 )",
                params![message_id, focus],
            )
            .map_err(database_error)?;
    }
    Ok(ScopeSnapshot {
        status: status.into(),
        focus_scope_key: focus_key,
        digest,
        reason_code: reason.map(str::to_string),
        scopes: resolved,
    })
}
pub(super) fn append_registered_coding_scope(
    connection: &Connection,
    conversation_id: &str,
    resolved: &mut Vec<ResolvedScope>,
) -> Result<(), String> {
    let workspace: Option<String> = connection
        .query_row(
            "SELECT id FROM coding_workspaces WHERE conversation_id=?1",
            [conversation_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(database_error)?;
    let Some(workspace) = workspace else {
        return Ok(());
    };
    for (kind, id, relation) in [
        ("project", workspace.as_str(), "focus"),
        ("resource", workspace.as_str(), "parent"),
    ] {
        let key = format!("{kind}:{id}");
        let active: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM context_scopes
                 WHERE scope_key=?1 AND kind=?2 AND opaque_id=?3 AND state='active')",
                params![key, kind, id],
                |row| row.get(0),
            )
            .map_err(database_error)?;
        if active {
            resolved.push(ResolvedScope {
                key: key.clone(),
                kind: kind.into(),
                relation: relation.into(),
                epoch: epoch(connection, &key)?,
            });
        }
    }
    let task: Option<String> = connection
        .query_row(
            "SELECT id FROM coding_jobs WHERE conversation_id=?1
             AND state IN ('queued','running','cancel_requested') ORDER BY rowid DESC LIMIT 1",
            [conversation_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(database_error)?;
    if let Some(task) = task {
        let key = format!("task:{task}");
        let active: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM context_scopes
                 WHERE scope_key=?1 AND kind='task' AND opaque_id=?2 AND state='active')",
                params![key, task],
                |row| row.get(0),
            )
            .map_err(database_error)?;
        if active {
            resolved.push(ResolvedScope {
                key: key.clone(),
                kind: "task".into(),
                relation: "current".into(),
                epoch: epoch(connection, &key)?,
            });
        }
    }
    Ok(())
}
pub(crate) fn load(connection: &Connection, run_id: &str) -> Result<ScopeSnapshot, String> {
    let (status, focus_scope_key, digest, reason_code) = connection
        .query_row(
            "SELECT status,focus_scope_key,scope_digest,reason_code
             FROM runtime_scope_resolutions WHERE run_id=?1",
            [run_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .map_err(database_error)?;
    let mut statement = connection
        .prepare_cached(
            "SELECT r.scope_key,s.kind,r.relation,r.epoch
             FROM runtime_run_scopes r JOIN context_scopes s ON s.scope_key=r.scope_key
             WHERE r.run_id=?1 ORDER BY s.kind,r.scope_key,r.relation",
        )
        .map_err(database_error)?;
    let scopes = statement
        .query_map([run_id], |row| {
            Ok(ResolvedScope {
                key: row.get(0)?,
                kind: row.get(1)?,
                relation: row.get(2)?,
                epoch: row.get(3)?,
            })
        })
        .map_err(database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error)?;
    Ok(ScopeSnapshot {
        status,
        focus_scope_key,
        digest,
        reason_code,
        scopes,
    })
}
pub(crate) fn attach_output(
    connection: &Connection,
    run_id: &str,
    message_id: &str,
) -> Result<(), String> {
    connection
        .execute(
            "INSERT INTO conversation_message_scopes(message_id,scope_key,relation)
             SELECT ?2,s.scope_key,s.relation FROM runtime_run_scopes s
             JOIN runtime_scope_resolutions r ON r.run_id=s.run_id
             WHERE s.run_id=?1 AND (s.relation!='shared' OR r.focus_scope_key=s.scope_key)",
            params![run_id, message_id],
        )
        .map_err(database_error)?;
    connection
        .execute(
            "INSERT OR IGNORE INTO personal_source_scope_refs(source_id,version,scope_key)
             SELECT p.message_id,p.version,m.scope_key
             FROM personal_sources p JOIN conversation_message_scopes m ON m.message_id=p.message_id
             WHERE p.message_id=?2",
            params![run_id, message_id],
        )
        .map_err(database_error)?;
    connection
        .execute(
            "UPDATE personal_jobs
             SET scope_key=(SELECT focus_scope_key FROM runtime_scope_resolutions WHERE run_id=?1),
                 claim_scope_epoch=(SELECT e.epoch FROM runtime_scope_resolutions r
                   JOIN context_scope_epochs e ON e.scope_key=r.focus_scope_key WHERE r.run_id=?1)
             WHERE source_sequence IN (
               SELECT sequence FROM personal_sources WHERE message_id=?2 AND available=1
             )",
            params![run_id, message_id],
        )
        .map_err(database_error)?;
    Ok(())
}
pub(super) fn ensure_scope(connection: &Connection, kind: &str, id: &str) -> Result<String, String> {
    let key = format!("{kind}:{id}");
    connection
        .execute(
            "INSERT OR IGNORE INTO context_scopes(scope_key,kind,opaque_id,state,created_at)
             VALUES(?1,?2,?3,'active',?4)",
            params![key, kind, id, now_iso()],
        )
        .map_err(database_error)?;
    connection
        .execute(
            "INSERT OR IGNORE INTO context_scope_epochs(scope_key,epoch) VALUES(?1,0)",
            [&key],
        )
        .map_err(database_error)?;
    Ok(key)
}
pub(crate) fn register(connection: &Connection, kind: &str, id: &str) -> Result<String, String> {
    if !matches!(kind, "project" | "task" | "resource") {
        return Err("Only durable work scopes can be registered explicitly".into());
    }
    crate::validate_identifier(id, "context scope id")?;
    ensure_scope(connection, kind, id)
}
pub(crate) fn link(
    connection: &Connection,
    parent_scope_key: &str,
    child_scope_key: &str,
) -> Result<(), String> {
    if parent_scope_key == child_scope_key {
        return Err("Context scope cannot link to itself".into());
    }
    let both_exist: bool = connection
        .query_row(
            "SELECT (SELECT COUNT(*) FROM context_scopes
                     WHERE scope_key IN (?1,?2) AND state='active')=2",
            params![parent_scope_key, child_scope_key],
            |row| row.get(0),
        )
        .map_err(database_error)?;
    if !both_exist {
        return Err("Context scope link references a missing scope".into());
    }
    connection
        .execute(
            "INSERT OR IGNORE INTO context_scope_links(
               parent_scope_key,child_scope_key,relation,created_at
             ) VALUES(?1,?2,'parent',?3)",
            params![parent_scope_key, child_scope_key, now_iso()],
        )
        .map_err(database_error)?;
    Ok(())
}
pub(crate) fn revoke(connection: &Connection, scope_key: &str) -> Result<(), String> {
    let changed = connection
        .execute(
            "UPDATE context_scopes SET state='revoked'
             WHERE scope_key=?1 AND state='active'",
            [scope_key],
        )
        .map_err(database_error)?;
    if changed == 1 {
        bump_epoch(connection, scope_key)?;
    }
    Ok(())
}
pub(super) fn epoch(connection: &Connection, key: &str) -> Result<u64, String> {
    connection
        .query_row(
            "SELECT epoch FROM context_scope_epochs WHERE scope_key=?1",
            [key],
            |row| row.get(0),
        )
        .map_err(database_error)
}
pub(super) fn bump_epoch(connection: &Connection, key: &str) -> Result<(), String> {
    connection
        .execute(
            "UPDATE context_scope_epochs SET epoch=epoch+1 WHERE scope_key=?1",
            [key],
        )
        .map_err(database_error)?;
    Ok(())
}
pub(super) fn linked(connection: &Connection, left: &str, right: &str) -> bool {
    connection
        .query_row(
            "WITH RECURSIVE linked(scope_key) AS (
               VALUES(?1)
               UNION
               SELECT CASE WHEN l.parent_scope_key=linked.scope_key
                           THEN l.child_scope_key ELSE l.parent_scope_key END
               FROM context_scope_links l JOIN linked
                 ON l.parent_scope_key=linked.scope_key OR l.child_scope_key=linked.scope_key
             ) SELECT EXISTS(SELECT 1 FROM linked WHERE scope_key=?2)",
            params![left, right],
            |row| row.get(0),
        )
        .unwrap_or(false)
}
pub(super) fn snapshot_digest(status: &str, focus: Option<&str>, scopes: &[ResolvedScope]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(status.as_bytes());
    hasher.update(focus.unwrap_or_default().as_bytes());
    for scope in scopes {
        hasher.update(scope.key.as_bytes());
        hasher.update(scope.relation.as_bytes());
        hasher.update(scope.epoch.to_le_bytes());
    }
    format!("{:x}", hasher.finalize())
}
