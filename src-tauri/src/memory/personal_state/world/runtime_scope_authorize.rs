//! Authorizes the whole scope and target set before reading owners.
use super::*;
/// R2: verify the trusted request, current run context, project linkage and
/// every requested resource/task scope against the same DB snapshot. Any
/// failure denies the whole request before any owner id lookup.
pub(crate) fn authorize_frame_request(
    c: &Connection,
    request: &FrameRequest<'_>,
) -> Result<AuthorizedFrame, FrameError> {
    let access = &request.access;
    if !access.authorized
        || access.scope != "primary"
        || access.task_request != Some(request.project_scope)
        || !matches!(access.purpose, Purpose::Reasoning | Purpose::Diagnostics)
        || access.max_classification < Classification::Confidential
    {
        return Err(FrameError::ScopeDenied);
    }
    let project = request.project_scope;
    let is_user = project.starts_with("user:");
    let project_id = project
        .strip_prefix("project:")
        .or_else(|| project.strip_prefix("user:"))
        .ok_or(FrameError::ScopeDenied)?;
    if is_user && (!request.runtime_refs.is_empty() || request.graph_request.is_some()) {
        return Err(FrameError::ScopeDenied);
    }
    if !saaa_personal_state_core::world::runtime_frame::validate_frame_identifier(project_id)
        || crate::validate_identifier(request.run_id, "run id").is_err()
    {
        return Err(FrameError::ScopeDenied);
    }
    let header = load_header(c)?;
    if header.principal != access.principal || header.policy_revision != access.policy_revision {
        return Err(FrameError::ScopeDenied);
    }
    let (conversation_id, run_status, input_message_id): (String, String, Option<String>) = c
        .query_row(
            "SELECT conversation_id,status,input_message_id FROM runtime_runs WHERE id=?1",
            [&request.run_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(|_| FrameError::ScopeDenied)?;
    if run_status != "running" {
        return Err(FrameError::ScopeDenied);
    }
    let Some(input_message_id) = input_message_id else {
        return Err(FrameError::ScopeDenied);
    };
    let (content, source_ok): (String, bool) = c
        .query_row(
            "SELECT m.content,
                    (m.conversation_id=?2 AND m.role IN ('user','transcript')
                     AND NOT EXISTS(SELECT 1 FROM personal_tombstones t WHERE t.source_id=m.id))
             FROM conversation_messages m WHERE m.id=?1",
            params![input_message_id, conversation_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|_| FrameError::ScopeDenied)?;
    if !source_ok {
        return Err(FrameError::ScopeDenied);
    }
    let snapshot = scope::load(c, request.run_id).map_err(|_| FrameError::ScopeDenied)?;
    if snapshot.status != "resolved" {
        return Err(FrameError::ScopeDenied);
    }
    // The requested Project must be an explicit focus/parent of this run and
    // still active with a matching epoch.
    let project_scope = snapshot
        .scopes
        .iter()
        .find(|s| s.key == project && matches!(s.relation.as_str(), "focus" | "parent"))
        .ok_or(FrameError::ScopeDenied)?
        .clone();
    let project_epoch = active_epoch(c, project)?.ok_or(FrameError::ScopeDenied)?;
    if project_epoch != project_scope.epoch {
        return Err(FrameError::ScopeDenied);
    }
    if scope_kind(c, project)?.0 != if is_user { "user" } else { "project" }
        || (is_user && project_id != header.principal)
    {
        return Err(FrameError::ScopeDenied);
    }

    let mut targets = Vec::new();
    let mut digest_parts = vec![
        serde_json::Value::from(request.run_id),
        serde_json::Value::from(input_message_id.as_str()),
        serde_json::Value::from(hash(&[serde_json::Value::from(content.as_str())])),
    ];
    // Project participation in the digest, including the direct-link check.
    digest_parts.push(serde_json::json!([
        project,
        project_scope.relation,
        project_scope.epoch,
        project_epoch,
    ]));
    for reference in &request.runtime_refs {
        let scope_key = runtime_scope_key(reference);
        let (kind, opaque_id) = scope_kind(c, &scope_key)?;
        let expected_kind = match reference.kind {
            RuntimeKind::CodingJob => "task",
        };
        if kind != expected_kind || opaque_id != reference.id {
            return Err(FrameError::ScopeDenied);
        }
        let recorded = snapshot
            .scopes
            .iter()
            .find(|s| {
                s.key == scope_key && matches!(s.relation.as_str(), "focus" | "current" | "parent")
            })
            .ok_or(FrameError::ScopeDenied)?
            .clone();
        let current = active_epoch(c, &scope_key)?.ok_or(FrameError::ScopeDenied)?;
        if current != recorded.epoch {
            return Err(FrameError::ScopeDenied);
        }
        if !direct_link(c, project, &scope_key)? {
            return Err(FrameError::ScopeDenied);
        }
        digest_parts.push(serde_json::json!([
            scope_key,
            kind,
            recorded.relation,
            recorded.epoch,
            current,
            true,
        ]));
        targets.push(AuthorizedTarget {
            reference: reference.clone(),
        });
    }
    let mut allowed_scope_keys = Vec::new();
    for item in &snapshot.scopes {
        if item.key != project
            && !(item.kind == "user" && item.key == format!("user:{}", header.principal))
            && (is_user || !direct_link(c, project, &item.key)?)
        {
            continue;
        }
        if active_epoch(c, &item.key)? != Some(item.epoch) {
            return Err(FrameError::ScopeDenied);
        }
        allowed_scope_keys.push(item.key.clone());
        digest_parts.push(serde_json::json!([item.key, item.epoch, item.relation]));
    }
    allowed_scope_keys.sort();
    allowed_scope_keys.dedup();
    let scope_digest = hash(&digest_parts);
    let scope = saaa_personal_state_core::world::frame_sources::WorldScope {
        focus_scope_key: (!is_user).then(|| project.to_string()),
        allowed_scope_keys,
        digest: scope_digest.clone(),
    };
    Ok(AuthorizedFrame {
        scope,
        run_id: request.run_id.to_string(),
        project_scope: project.to_string(),
        conversation_id,
        ledger_revision: header.ledger_revision,
        input_epoch: header.input_epoch,
        policy_revision: header.policy_revision,
        scope_digest,
        targets,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use saaa_personal_state_core::world::runtime_frame::RuntimeRef;

    #[test]
    fn hash_is_stable_and_order_sensitive() {
        let a = hash(&[serde_json::Value::from("a"), serde_json::Value::from(1)]);
        let b = hash(&[serde_json::Value::from("a"), serde_json::Value::from(1)]);
        let c = hash(&[serde_json::Value::from(1), serde_json::Value::from("a")]);
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn runtime_scope_key_matches_kind() {
        assert_eq!(
            runtime_scope_key(&RuntimeRef {
                kind: RuntimeKind::CodingJob,
                id: "j1".into()
            }),
            "task:j1"
        );
    }
}
