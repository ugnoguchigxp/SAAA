use super::*;
#[derive(Debug, Clone, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RoutingSnapshotInput {
    pub(crate) conversation_id: String,
}
#[derive(Debug, Clone, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RoutingEventReplayInput {
    pub(crate) root_id: String,
    pub(crate) after_seq: i64,
}
#[derive(Debug, Clone, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RoutingCancelInput {
    pub(crate) root_id: String,
}
#[derive(Debug, Clone, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RoutingProposalDecisionInput {
    pub(crate) proposal_id: String,
    pub(crate) candidate_id: String,
    pub(crate) approve: bool,
}
#[derive(Debug, Clone, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AdaptiveRollbackInput {
    pub(crate) artifact_id: String,
}
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AdaptiveArtifactSnapshot {
    pub(crate) id: String,
    pub(crate) domain: String,
    pub(crate) scope_key: String,
    pub(crate) state: String,
    pub(crate) eligible_examples: i64,
    pub(crate) best_observed_score: Option<f64>,
    pub(crate) policy_revision: Option<i64>,
    pub(crate) reason: String,
}
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RoutingLearningSnapshot {
    pub(crate) dirty_roots: i64,
    pub(crate) ready_datasets: i64,
    pub(crate) active_artifacts: i64,
    pub(crate) invalidated_datasets: i64,
    pub(crate) pending_cleanups: i64,
    pub(crate) adaptive_artifacts: Vec<AdaptiveArtifactSnapshot>,
}
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RoutingRootSnapshot {
    pub(crate) root_id: String,
    pub(crate) runtime_run_id: Option<String>,
    pub(crate) phase: String,
    pub(crate) revision: u32,
    pub(crate) active_slot: Option<String>,
    pub(crate) cancel_requested: bool,
    pub(crate) last_event_seq: i64,
    pub(crate) selected_recipe_id: Option<String>,
    pub(crate) decision_reason_codes: Vec<String>,
}
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RoutingSnapshot {
    pub(crate) active: Option<RoutingRootSnapshot>,
    pub(crate) queued: Vec<RoutingRootSnapshot>,
    pub(crate) recent_root_ids: Vec<String>,
    pub(crate) proposals: Vec<RoutingProposalSnapshot>,
}
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RoutingProposalSnapshot {
    pub(crate) id: String,
    pub(crate) root_id: String,
    pub(crate) candidate_id: String,
    pub(crate) estimated_cost_micros: Option<i64>,
    pub(crate) expires_at_ms: i64,
    pub(crate) status: String,
    pub(crate) consumed: bool,
}
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RoutingEventRecord {
    pub(crate) root_id: String,
    pub(crate) seq: i64,
    pub(crate) kind: String,
    pub(crate) data_json: String,
    pub(crate) created_at_ms: i64,
}
#[tauri::command]
pub(crate) fn get_routing_snapshot(
    state: tauri::State<'_, AppState>,
    input: RoutingSnapshotInput,
) -> Result<RoutingSnapshot, String> {
    crate::validate_identifier(&input.conversation_id, "conversation id")?;
    state
        .sqlite_readers
        .read(|connection| snapshot(connection, &input.conversation_id))
}
#[tauri::command]
pub(crate) fn replay_routing_events(
    state: tauri::State<'_, AppState>,
    input: RoutingEventReplayInput,
) -> Result<Vec<RoutingEventRecord>, String> {
    crate::validate_identifier(&input.root_id, "routing root id")?;
    if input.after_seq < 0 {
        return Err("Routing event sequence cannot be negative".into());
    }
    state
        .sqlite_readers
        .read(|connection| replay(connection, &input.root_id, input.after_seq))
}
/// Persists cancellation before signalling the local run.  A reconnect can therefore always
/// observe the cancellation even when the process stops immediately after this command returns.
#[tauri::command]
pub(crate) fn cancel_routing_root(
    state: tauri::State<'_, crate::AppState>,
    app: tauri::AppHandle,
    input: RoutingCancelInput,
) -> Result<RoutingRootSnapshot, String> {
    crate::validate_identifier(&input.root_id, "routing root id")?;
    let (runtime_run_id, root) = state.sqlite_writer.write(|connection| {
        let runtime_run_id: Option<String> = connection
            .query_row(
                "SELECT runtime_run_id FROM rr_roots WHERE root_id=?1",
                [&input.root_id],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "Role-routing root was not found".to_string())?;
        crate::role_routing::coordinator::apply(
            connection,
            &input.root_id,
            crate::role_routing::reducer::Event::Cancel,
            now_ms(),
        )?;
        Ok((runtime_run_id, root_snapshot(connection, &input.root_id)?))
    })?;
    if let Some(run_id) = runtime_run_id {
        if let Ok(active) = state.active_runs.lock() {
            if let Some(cancellation) = active.get(&run_id) {
                cancellation.cancel();
            }
        }
        state.streaming_tts.cancel(&run_id);
    }
    // The durable snapshot is already committed. Event delivery is best-effort; reporting an
    // emitter failure here would incorrectly tell the caller that its cancellation failed.
    let _ = app.emit(
        "role-routing-updated",
        serde_json::json!({"rootId":root.root_id}),
    );
    Ok(root)
}
#[tauri::command]
pub(crate) fn decide_routing_proposal(
    state: tauri::State<'_, crate::AppState>,
    app: tauri::AppHandle,
    input: RoutingProposalDecisionInput,
) -> Result<RoutingProposalSnapshot, String> {
    crate::validate_identifier(&input.proposal_id, "routing proposal id")?;
    crate::validate_identifier(&input.candidate_id, "routing proposal candidate id")?;
    let proposal = state.sqlite_writer.write(|connection| {
        let stored_candidate: String = connection
            .query_row(
                "SELECT candidate_id FROM rr_premium_proposals WHERE id=?1",
                [&input.proposal_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "Role-routing premium proposal is unavailable".to_string())?;
        if stored_candidate != input.candidate_id {
            return Err("Role-routing premium proposal candidate does not match".into());
        }
        if input.approve {
            let policy_id: String = connection
                .query_row(
                    "SELECT policy_id FROM rr_premium_proposals WHERE id=?1",
                    [&input.proposal_id],
                    |row| row.get(0),
                )
                .map_err(|error| error.to_string())?;
            let cloud_allowed = crate::role_routing::proposals::candidate_available(
                connection,
                &policy_id,
                &input.candidate_id,
            )?;
            crate::role_routing::proposals::approve(
                connection,
                &crate::role_routing::proposals::Approval {
                    proposal_id: input.proposal_id.clone(),
                    candidate_id: input.candidate_id.clone(),
                },
                now_ms(),
                cloud_allowed,
            )?;
        } else {
            crate::role_routing::proposals::decline(connection, &input.proposal_id, now_ms())?;
        }
        proposal_snapshot(connection, &input.proposal_id)
    })?;
    let _ = app.emit(
        "role-routing-updated",
        serde_json::json!({"rootId":proposal.root_id}),
    );
    Ok(proposal)
}
/// Runs one locally authorized materialization batch. It is intentionally a database-only
/// operation: no provider, labeler, or artifact activation is performed on the UI thread.
#[tauri::command]
pub(crate) fn run_routing_learning_once(
    state: tauri::State<'_, AppState>,
) -> Result<RoutingLearningSnapshot, String> {
    state.sqlite_writer.write(|connection| {
        let settings = crate::persistence::load_role_routing_settings(connection)?;
        if !settings.learning.enabled {
            return Err("Role-routing learning is disabled".into());
        }
        let _ = crate::role_routing::learning::repository::materialize_dirty_roots(
            connection,
            now_ms(),
            settings.learning.batch_size,
        )?;
        let adaptive_domains_enabled = settings.adaptive_improvement.provider_recipe
            || settings.adaptive_improvement.tool
            || settings.adaptive_improvement.plan
            || settings.adaptive_improvement.notification;
        if settings.adaptive_improvement.enabled && adaptive_domains_enabled {
            let now = now_ms();
            if let Some(dataset_id) = crate::adaptive_improvement::materialize_dirty(
                connection,
                settings.learning.batch_size as usize,
                now,
            )? {
                // Training only writes an immutable candidate artifact. It cannot activate a
                // policy or contact a provider from this foreground settings action.
                let _ = crate::adaptive_improvement::train_candidate_artifacts(
                    connection,
                    &dataset_id,
                    now,
                )?;
            }
        }
        learning_snapshot(connection)
    })
}
#[tauri::command]
pub(crate) fn get_routing_learning_snapshot(
    state: tauri::State<'_, AppState>,
) -> Result<RoutingLearningSnapshot, String> {
    state.sqlite_readers.read(learning_snapshot)
}
/// Stops use of exactly one active learned policy. Explicit user corrections are separate and
/// remain effective; this operation only returns the learned choice to the fixed rules fallback.
#[tauri::command]
pub(crate) fn rollback_adaptive_artifact(
    state: tauri::State<'_, AppState>,
    input: AdaptiveRollbackInput,
) -> Result<RoutingLearningSnapshot, String> {
    if input.artifact_id.is_empty() || input.artifact_id.len() > 256 {
        return Err("Adaptive artifact id is invalid".into());
    }
    state.sqlite_writer.write(|connection| {
        crate::adaptive_improvement::rollback_active_to_rules(
            connection,
            &input.artifact_id,
            now_ms(),
        )?;
        learning_snapshot(connection)
    })
}
pub(super) fn learning_snapshot(connection: &Connection) -> Result<RoutingLearningSnapshot, String> {
    let (dirty_roots, ready_datasets, active_artifacts, invalidated_datasets, pending_cleanups) =
        connection
            .query_row(
                "SELECT
                (SELECT count(*) FROM rr_learning_dirty),
                (SELECT count(*) FROM rr_datasets WHERE state='ready'),
                (SELECT count(*) FROM rr_ranker_artifacts WHERE state IN ('candidate','shadow')),
                (SELECT count(*) FROM rr_datasets WHERE state='invalidated'),
                (SELECT count(*) FROM rr_cleanup_journal WHERE state='pending')",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .map_err(|error| error.to_string())?;
    let adaptive_schema_present: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='ai_artifacts')",
            [],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    let adaptive_artifacts = if adaptive_schema_present {
        adaptive_artifact_snapshots(connection)?
    } else {
        Vec::new()
    };
    Ok(RoutingLearningSnapshot {
        dirty_roots,
        ready_datasets,
        active_artifacts,
        invalidated_datasets,
        pending_cleanups,
        adaptive_artifacts,
    })
}
pub(super) fn adaptive_artifact_snapshots(
    connection: &Connection,
) -> Result<Vec<AdaptiveArtifactSnapshot>, String> {
    let mut statement = connection
        .prepare(
            "SELECT a.id,a.domain,a.scope_key,a.state,a.scores_json,a.source_event_upper_seq,
                    (SELECT x.policy_revision FROM ai_activations x WHERE x.artifact_id=a.id AND x.active=1)
             FROM ai_artifacts a
             WHERE a.state IN ('candidate','evaluated','shadow','eligible','active')
             ORDER BY CASE a.state WHEN 'active' THEN 0 WHEN 'eligible' THEN 1 WHEN 'shadow' THEN 2 WHEN 'evaluated' THEN 3 ELSE 4 END,
                      a.created_at_ms DESC, a.id
             LIMIT 24",
        )
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, Option<i64>>(6)?,
            ))
        })
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    rows.into_iter()
        .map(
            |(id, domain, scope_key, state, scores_json, upper_event_seq, policy_revision)| {
                let eligible_examples: i64 = connection
                    .query_row(
                        "SELECT count(*) FROM ai_examples e JOIN ai_datasets d ON d.id=e.dataset_id
                     WHERE e.eligible=1 AND d.state='ready' AND d.upper_event_seq<=?1
                       AND json_extract(e.features_json,'$.domain')=?2
                       AND json_extract(e.features_json,'$.scope')=?3",
                        params![upper_event_seq, &domain, &scope_key],
                        |row| row.get(0),
                    )
                    .map_err(|error| error.to_string())?;
                let best_observed_score = serde_json::from_str::<serde_json::Value>(&scores_json)
                    .ok()
                    .and_then(|value| {
                        value.as_object().and_then(|scores| {
                            scores
                                .values()
                                .filter_map(serde_json::Value::as_f64)
                                .reduce(f64::max)
                        })
                    });
                let reason = match state.as_str() {
                    "candidate" => "比較評価前の候補です。次の選択にはまだ使われません。",
                    "evaluated" => "比較評価の確認中です。次の選択にはまだ使われません。",
                    "shadow" => "安全性を確認中です。次の選択にはまだ使われません。",
                    "eligible" => "評価済みですが、まだ有効化されていません。",
                    "active" => "検証済みの改善として、次の一致する選択に使われます。",
                    _ => "状態を確認できません。",
                }
                .to_string();
                Ok(AdaptiveArtifactSnapshot {
                    id,
                    domain,
                    scope_key,
                    state,
                    eligible_examples,
                    best_observed_score,
                    policy_revision,
                    reason,
                })
            },
        )
        .collect()
}
pub(crate) fn snapshot(
    connection: &Connection,
    conversation_id: &str,
) -> Result<RoutingSnapshot, String> {
    let mut statement = connection
        .prepare(
            "SELECT root_id,runtime_run_id,phase,revision,active_slot,cancel_requested,
                    COALESCE((SELECT MAX(seq) FROM rr_events WHERE root_id=rr_roots.root_id),0),
                    (SELECT selected_id FROM rr_decisions WHERE root_id=rr_roots.root_id ORDER BY created_at_ms DESC LIMIT 1),
                    (SELECT reason_codes_json FROM rr_decisions WHERE root_id=rr_roots.root_id ORDER BY created_at_ms DESC LIMIT 1)
             FROM rr_roots WHERE conversation_id=?1 AND phase IN ('queued','responding','draining')
             ORDER BY CASE phase WHEN 'queued' THEN 1 ELSE 0 END, started_at_ms, root_id",
        )
        .map_err(|error| error.to_string())?;
    let roots = statement
        .query_map([conversation_id], root_snapshot_row)
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    let mut active = None;
    let mut queued = Vec::new();
    for root in roots {
        if root.phase == "queued" {
            queued.push(root);
        } else if active.is_none() {
            active = Some(root);
        }
    }
    let proposals = connection
        .prepare(
            "SELECT p.id,p.root_id,p.candidate_id,p.estimated_cost_micros,p.expires_at_ms,p.status,p.consumed_at_ms IS NOT NULL
             FROM rr_premium_proposals p JOIN rr_roots r ON r.root_id=p.root_id
             WHERE r.conversation_id=?1 ORDER BY p.created_at_ms DESC,p.id LIMIT 16",
        )
        .map_err(|error| error.to_string())?
        .query_map([conversation_id], proposal_snapshot_row)
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    let recent_root_ids = connection
        .prepare(
            "SELECT root_id FROM rr_roots WHERE conversation_id=?1 ORDER BY started_at_ms DESC,root_id DESC LIMIT 8",
        )
        .map_err(|error| error.to_string())?
        .query_map([conversation_id], |row| row.get::<_, String>(0))
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    Ok(RoutingSnapshot {
        active,
        queued,
        recent_root_ids,
        proposals,
    })
}
pub(super) fn root_snapshot_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RoutingRootSnapshot> {
    let reason_codes_json: Option<String> = row.get(8)?;
    Ok(RoutingRootSnapshot {
        root_id: row.get(0)?,
        runtime_run_id: row.get(1)?,
        phase: row.get(2)?,
        revision: row.get(3)?,
        active_slot: row.get(4)?,
        cancel_requested: row.get::<_, i64>(5)? != 0,
        last_event_seq: row.get(6)?,
        selected_recipe_id: row.get(7)?,
        decision_reason_codes: reason_codes_json
            .as_deref()
            .and_then(|value| serde_json::from_str(value).ok())
            .unwrap_or_default(),
    })
}
pub(super) fn proposal_snapshot_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RoutingProposalSnapshot> {
    Ok(RoutingProposalSnapshot {
        id: row.get(0)?,
        root_id: row.get(1)?,
        candidate_id: row.get(2)?,
        estimated_cost_micros: row.get(3)?,
        expires_at_ms: row.get(4)?,
        status: row.get(5)?,
        consumed: row.get::<_, bool>(6)?,
    })
}
pub(super) fn proposal_snapshot(
    connection: &Connection,
    proposal_id: &str,
) -> Result<RoutingProposalSnapshot, String> {
    connection
        .query_row(
            "SELECT id,root_id,candidate_id,estimated_cost_micros,expires_at_ms,status,consumed_at_ms IS NOT NULL FROM rr_premium_proposals WHERE id=?1",
            [proposal_id],
            proposal_snapshot_row,
        )
        .optional()
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "Role-routing premium proposal is unavailable".into())
}
pub(super) fn root_snapshot(connection: &Connection, root_id: &str) -> Result<RoutingRootSnapshot, String> {
    connection
        .query_row(
            "SELECT root_id,runtime_run_id,phase,revision,active_slot,cancel_requested,
                    COALESCE((SELECT MAX(seq) FROM rr_events WHERE root_id=rr_roots.root_id),0),
                    (SELECT selected_id FROM rr_decisions WHERE root_id=rr_roots.root_id ORDER BY created_at_ms DESC LIMIT 1),
                    (SELECT reason_codes_json FROM rr_decisions WHERE root_id=rr_roots.root_id ORDER BY created_at_ms DESC LIMIT 1)
             FROM rr_roots WHERE root_id=?1",
            [root_id],
            root_snapshot_row,
        )
        .optional()
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "Role-routing root was not found".into())
}
pub(crate) fn replay(
    connection: &Connection,
    root_id: &str,
    after_seq: i64,
) -> Result<Vec<RoutingEventRecord>, String> {
    connection
        .prepare("SELECT root_id,seq,kind,data_json,created_at_ms FROM rr_events WHERE root_id=?1 AND seq>?2 ORDER BY seq LIMIT 256")
        .map_err(|error| error.to_string())?
        .query_map(params![root_id, after_seq], |row| Ok(RoutingEventRecord { root_id: row.get(0)?, seq: row.get(1)?, kind: row.get(2)?, data_json: row.get(3)?, created_at_ms: row.get(4)? }))
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())
}
