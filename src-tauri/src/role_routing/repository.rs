//! Persistence helpers for immutable policy snapshots.

#[path = "repository_policy.rs"]
mod repository_policy;
#[path = "repository_turns/mod.rs"]
#[cfg_attr(not(test), allow(dead_code))]
mod repository_turns;

pub(crate) use repository_policy::capture_current_policy;
#[allow(unused_imports)]
pub(crate) use repository_policy::capture_policy_version;
pub(crate) use repository_turns::{
    accept_provider_turn, accept_provider_turn_with_status, accept_reviewed_draft, advance_provider_step,
    advance_provider_step_with_usage, advance_review_step, cancel_all_for_disable,
    disable_drain_in_progress, disabled_runtime_run_ids, record_actor_activity,
    record_input_receipt, record_provider_turn_finish, record_provider_turn_start_in_transaction,
    record_step_usage, ReviewStepOutcome,
};

use rusqlite::{params, Connection, OptionalExtension};
use serde_json::json;
use sha2::{Digest, Sha256};

/// Records host-validated feedback. The caller must identify both message ids; text matching is
/// intentionally not used because a quoted old answer is not evidence about the current one.
#[allow(clippy::too_many_arguments)] // Feedback evidence is committed as one immutable record.
pub(crate) fn record_feedback(
    connection: &Connection,
    conversation_id: &str,
    source_message_id: &str,
    target_answer_id: &str,
    kind: &str,
    evidence_start: usize,
    evidence_end: usize,
    now_ms: i64,
) -> Result<bool, String> {
    if !matches!(
        kind,
        "answer_challenge" | "explicit_positive" | "explicit_negative"
    ) {
        return Err("Unsupported role-routing feedback kind".into());
    }
    let source: String = connection.query_row(
        "SELECT content FROM conversation_messages WHERE id=?1 AND conversation_id=?2 AND role='user'",
        params![source_message_id, conversation_id], |r| r.get(0),
    ).map_err(|_| "Feedback source message is unavailable".to_string())?;
    if evidence_start > evidence_end
        || evidence_end > source.len()
        || !source.is_char_boundary(evidence_start)
        || !source.is_char_boundary(evidence_end)
    {
        return Err("Feedback evidence range is invalid".into());
    }
    let target_root: Option<Option<String>> = connection
        .query_row(
            "SELECT r.root_id FROM conversation_messages m LEFT JOIN rr_roots r ON r.result_message_id=m.id WHERE m.id=?1 AND m.conversation_id=?2 AND m.role='assistant'",
            params![target_answer_id, conversation_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| "Feedback target answer is unavailable".to_string())?;
    let root_id = target_root
        .ok_or_else(|| "Feedback target answer is unavailable".to_string())?
        .ok_or_else(|| "Feedback target is not a completed role-routing answer".to_string())?;
    let digest = format!(
        "{:x}",
        Sha256::digest(format!("{target_answer_id}:{source_message_id}:{kind}").as_bytes())
    );
    let feedback_id = format!("rr-feedback-{}", &digest[..24]);
    let evidence_json = json!({"start":evidence_start,"end":evidence_end}).to_string();
    let inserted = connection.execute(
        "INSERT OR IGNORE INTO rr_feedback(id,target_answer_id,target_root_id,source_message_id,kind,evidence_json,label_source,confidence,extractor_version,status,created_at_ms) VALUES(?1,?2,?3,?4,?5,?6,'host',1.0,'host-v1','recorded',?7)",
        params![feedback_id,target_answer_id,root_id,source_message_id,kind,evidence_json,now_ms],
    ).map_err(|e| e.to_string())? == 1;
    if !inserted {
        let existing: Option<(String, String, String, String)> = connection
            .query_row(
                "SELECT target_answer_id,source_message_id,kind,evidence_json FROM rr_feedback WHERE id=?1",
                [&feedback_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()
            .map_err(|error| error.to_string())?;
        if existing
            != Some((
                target_answer_id.to_string(),
                source_message_id.to_string(),
                kind.to_string(),
                evidence_json,
            ))
        {
            return Err("Role-routing feedback receipt conflicts with the original".into());
        }
    }
    if inserted {
        crate::role_routing::learning::repository::mark_root_dirty(connection, &root_id)?;
    }
    Ok(inserted)
}

/// Resolves only the currently visible assistant answer. A feedback-looking follow-up must not
/// attach to an older routing answer when a later legacy answer is on screen, and a legacy answer
/// is never retroactively made into role-routing training data.
pub(crate) fn record_feedback_for_latest_answer(
    connection: &Connection,
    conversation_id: &str,
    source_message_id: &str,
    kind: &str,
    evidence_start: usize,
    evidence_end: usize,
    now_ms: i64,
) -> Result<bool, String> {
    let target_answer_id: Option<String> = connection
        .query_row(
            "SELECT id FROM conversation_messages WHERE conversation_id=?1 AND role='assistant' ORDER BY created_at DESC, id DESC LIMIT 1",
            [conversation_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    let Some(target_answer_id) = target_answer_id else {
        return Ok(false);
    };
    let is_routing_answer: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM rr_roots WHERE result_message_id=?1 AND conversation_id=?2 AND phase='completed')",
            params![target_answer_id, conversation_id],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if !is_routing_answer {
        return Ok(false);
    }
    record_feedback(
        connection,
        conversation_id,
        source_message_id,
        &target_answer_id,
        kind,
        evidence_start,
        evidence_end,
        now_ms,
    )
}

/// Persists a host-validated independent review.  The author and reviewer are resolved from
/// their steps so callers cannot forge independence by naming another actor in JSON.
pub(crate) fn record_review_response(
    connection: &Connection,
    review_step_id: &str,
    response: &crate::role_routing::review::ReviewResponse,
    now_ms: i64,
) -> Result<bool, String> {
    let (root_id, revision, reviewer_actor, purpose): (String, i64, String, String) = connection
        .query_row(
            "SELECT root_id,revision,actor_id,purpose FROM rr_steps WHERE id=?1",
            [review_step_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .map_err(|_| "Role-routing review step is unavailable".to_string())?;
    if purpose != "review" {
        return Err("Role-routing review output requires a review step".into());
    }
    let author_actor: String = connection
        .query_row(
            "SELECT actor_id FROM rr_steps WHERE root_id=?1 AND purpose='respond' ORDER BY ordinal ASC LIMIT 1",
            [&root_id],
            |row| row.get(0),
        )
        .map_err(|_| "Role-routing review author step is unavailable".to_string())?;
    let allowed_evidence_refs = accepted_evidence_refs(connection, &root_id, revision)?;
    crate::role_routing::review::validate(
        &author_actor,
        &reviewer_actor,
        &response.issues,
        &allowed_evidence_refs,
    )?;
    let encoded = serde_json::to_string(response)
        .map_err(|error| format!("Could not encode role-routing review: {error}"))?;
    let existing: Option<String> = connection
        .query_row(
            "SELECT payload_json FROM rr_outputs WHERE step_id=?1 AND revision=?2 AND kind='review' AND accepted=1 ORDER BY created_at_ms,id LIMIT 1",
            params![review_step_id, revision],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    if let Some(existing) = existing {
        return if existing == encoded {
            Ok(false)
        } else {
            Err("Role-routing review step already has a different accepted output".into())
        };
    }
    let digest = format!(
        "{:x}",
        Sha256::digest(format!("{review_step_id}:{encoded}").as_bytes())
    );
    connection.execute(
        "INSERT INTO rr_outputs(id,step_id,revision,kind,payload_json,accepted,created_at_ms) VALUES(?1,?2,?3,'review',?4,1,?5)",
        params![format!("rr-review-{}", &digest[..24]), review_step_id, revision, encoded, now_ms],
    ).map_err(|error| error.to_string()).map(|changed| changed == 1)
}

/// Returns the accepted output ids that a review may cite as evidence: same root, not newer than
/// the review revision. A model cannot invent a reference outside this set.
fn accepted_evidence_refs(
    connection: &Connection,
    root_id: &str,
    revision: i64,
) -> Result<Vec<String>, String> {
    let refs = {
        let mut statement = connection
            .prepare(
                "SELECT o.id FROM rr_outputs o JOIN rr_steps s ON s.id=o.step_id WHERE s.root_id=?1 AND (o.accepted=1 OR o.kind='intermediate') AND o.revision<=?2",
            )
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query_map(params![root_id, revision], |row| row.get::<_, String>(0))
            .map_err(|error| error.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?
    };
    Ok(refs)
}

fn host_verified_evidence_refs(
    connection: &Connection,
    root_id: &str,
    revision: i64,
) -> Result<Vec<String>, String> {
    let mut statement = connection
        .prepare(
            "SELECT o.id FROM rr_outputs o JOIN rr_steps s ON s.id=o.step_id WHERE s.root_id=?1 AND o.revision<=?2 AND json_extract(o.payload_json,'$.hostVerification')='verified' AND length(COALESCE(json_extract(o.payload_json,'$.verifierVersion'),''))>0",
        )
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map(params![root_id, revision], |row| row.get::<_, String>(0))
        .map_err(|error| error.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())
}

/// Stores a deterministic revision decision for an accepted review output.  It deliberately does
/// not change the root revision or dispatch an author: only a future executor may consume an
/// allowed decision after re-checking the current root state.
pub(crate) fn record_review_revision_decision(
    connection: &Connection,
    review_output_id: &str,
    max_rounds: u8,
    now_ms: i64,
) -> Result<crate::role_routing::revision::ReviewRevisionDecision, String> {
    let (root_id, revision, review_step_id, payload): (String, i64, String, String) = connection
        .query_row(
            "SELECT s.root_id,o.revision,o.step_id,o.payload_json FROM rr_outputs o JOIN rr_steps s ON s.id=o.step_id WHERE o.id=?1 AND o.kind='review' AND o.accepted=1 AND s.purpose='review'",
            [review_output_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .map_err(|_| "Role-routing accepted review output is unavailable".to_string())?;
    let decision_id = format!("rr-revision-decision-{review_output_id}");
    let existing: Option<String> = connection
        .query_row(
            "SELECT payload_json FROM rr_outputs WHERE id=?1",
            [&decision_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    if let Some(existing) = existing {
        let persisted =
            serde_json::from_str::<crate::role_routing::revision::ReviewRevisionDecision>(
                &existing,
            )
            .map_err(|_| "Persisted role-routing revision decision is invalid".to_string())?;
        if persisted.max_rounds != max_rounds {
            return Err(
                "Role-routing revision decision conflicts with its persisted receipt".into(),
            );
        }
        return Ok(persisted);
    }
    let review = serde_json::from_str::<crate::role_routing::review::ReviewResponse>(&payload)
        .map_err(|_| "Role-routing review output is invalid".to_string())?;
    let completed_rounds: i64 = connection
        .query_row(
            "SELECT count(*) FROM rr_outputs o JOIN rr_steps s ON s.id=o.step_id WHERE s.root_id=?1 AND o.kind='revision-decision' AND json_extract(o.payload_json,'$.revisionAllowed')=1",
            [&root_id],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    let completed_rounds = u8::try_from(completed_rounds)
        .map_err(|_| "Role-routing revision round count is invalid".to_string())?;
    let allowed_evidence_refs = accepted_evidence_refs(connection, &root_id, revision)?;
    let verified_evidence_refs = host_verified_evidence_refs(connection, &root_id, revision)?;
    let decision = crate::role_routing::revision::decide_from_review(
        &review,
        &allowed_evidence_refs,
        &verified_evidence_refs,
        &[],
        completed_rounds,
        max_rounds,
    );
    let payload = serde_json::to_string(&decision)
        .map_err(|error| format!("Could not encode role-routing revision decision: {error}"))?;
    connection.execute(
        "INSERT INTO rr_outputs(id,step_id,revision,kind,payload_json,accepted,created_at_ms) VALUES(?1,?2,?3,'revision-decision',?4,1,?5)",
        params![decision_id, review_step_id, revision, payload, now_ms],
    ).map_err(|error| error.to_string())?;
    Ok(decision)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    #[test]
    fn identical_policy_is_captured_once() {
        let connection = Connection::open_in_memory().expect("connection");
        connection.execute_batch("CREATE TABLE settings_documents(namespace TEXT, key TEXT, value_json TEXT); CREATE TABLE rr_policy_versions(id TEXT PRIMARY KEY, version INTEGER UNIQUE, config_json TEXT, digest TEXT, created_at_ms INTEGER);").expect("tables");
        connection.execute("INSERT INTO settings_documents VALUES('routing.roles','default','{\"enabled\":false}')", []).expect("settings");
        capture_current_policy(&connection, 1).expect("first capture");
        capture_current_policy(&connection, 2).expect("same capture");
        let count: i64 = connection
            .query_row("SELECT count(*) FROM rr_policy_versions", [], |row| {
                row.get(0)
            })
            .expect("count");
        assert_eq!(count, 1);
    }

    #[test]
    fn policy_snapshot_ignores_json_object_key_order() {
        let connection = Connection::open_in_memory().expect("connection");
        connection.execute_batch("CREATE TABLE settings_documents(namespace TEXT, key TEXT, value_json TEXT); CREATE TABLE rr_policy_versions(id TEXT PRIMARY KEY, version INTEGER UNIQUE, config_json TEXT, digest TEXT, created_at_ms INTEGER);").expect("tables");
        connection.execute("INSERT INTO settings_documents VALUES('routing.roles','default','{\"enabled\":false,\"schemaVersion\":1,\"roles\":{\"frontend\":null,\"reasoner\":null}}')", []).expect("settings");
        capture_current_policy(&connection, 1).expect("first capture");
        connection.execute("UPDATE settings_documents SET value_json='{\"roles\":{\"reasoner\":null,\"frontend\":null},\"schemaVersion\":1,\"enabled\":false}' WHERE namespace='routing.roles' AND key='default'", []).expect("reordered settings");
        capture_current_policy(&connection, 2).expect("reordered capture");
        let (count, snapshot): (i64, String) = connection
            .query_row(
                "SELECT count(*), MIN(config_json) FROM rr_policy_versions",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("snapshot");
        assert_eq!(count, 1);
        assert_eq!(snapshot, "{\"enabled\":false,\"roles\":{\"frontend\":null,\"reasoner\":null},\"schemaVersion\":1}");
    }

    #[test]
    fn rr_23_feedback_requires_a_real_user_span_and_marks_the_root_dirty() {
        let connection = Connection::open_in_memory().expect("connection");
        connection.execute_batch("PRAGMA foreign_keys=ON; CREATE TABLE conversations(id TEXT PRIMARY KEY); CREATE TABLE runtime_runs(id TEXT PRIMARY KEY); CREATE TABLE conversation_messages(id TEXT PRIMARY KEY,conversation_id TEXT,role TEXT,content TEXT,created_at TEXT); INSERT INTO conversations VALUES('c');").expect("base");
        crate::role_routing::schema::migrate(&connection).expect("routing");
        crate::role_routing::learning::schema::migrate(&connection).expect("learning");
        connection
            .execute(
                "INSERT INTO rr_policy_versions VALUES('p',1,'{}','d',1)",
                [],
            )
            .expect("policy");
        connection.execute_batch("INSERT INTO conversation_messages VALUES('a','c','assistant','answer','1'); INSERT INTO conversation_messages VALUES('u','c','user','really?','2');").expect("messages");
        connection.execute("INSERT INTO rr_roots(root_id,conversation_id,policy_id,phase,origin,presentation_mode,started_at_ms,scope_digest,result_message_id) VALUES('r','c','p','completed','text','visual',1,'','a')",[]).expect("root");
        connection
            .execute(
                "INSERT INTO rr_events VALUES('r',1,'answer_committed','{}',1)",
                [],
            )
            .expect("event");
        assert!(
            record_feedback(&connection, "c", "u", "a", "answer_challenge", 0, 7, 2)
                .expect("feedback")
        );
        assert!(
            !record_feedback(&connection, "c", "u", "a", "answer_challenge", 0, 7, 3)
                .expect("exact retry")
        );
        assert!(record_feedback(&connection, "c", "u", "a", "answer_challenge", 0, 6, 4).is_err());
        assert_eq!(
            connection
                .query_row("SELECT root_id FROM rr_learning_dirty", [], |r| r
                    .get::<_, String>(0))
                .expect("dirty"),
            "r"
        );
        assert!(record_feedback(&connection, "c", "u", "a", "answer_challenge", 8, 8, 2).is_err());
    }

    #[test]
    fn rr_23_feedback_targets_only_the_visible_routing_answer() {
        let connection = Connection::open_in_memory().expect("connection");
        connection.execute_batch("PRAGMA foreign_keys=ON; CREATE TABLE conversations(id TEXT PRIMARY KEY); CREATE TABLE runtime_runs(id TEXT PRIMARY KEY); CREATE TABLE conversation_messages(id TEXT PRIMARY KEY,conversation_id TEXT,role TEXT,content TEXT,created_at TEXT); INSERT INTO conversations VALUES('c');").expect("base");
        crate::role_routing::schema::migrate(&connection).expect("routing");
        crate::role_routing::learning::schema::migrate(&connection).expect("learning");
        connection
            .execute(
                "INSERT INTO rr_policy_versions VALUES('p',1,'{}','d',1)",
                [],
            )
            .expect("policy");
        connection.execute_batch("INSERT INTO conversation_messages VALUES('routed','c','assistant','routed answer','1'); INSERT INTO conversation_messages VALUES('legacy','c','assistant','later legacy answer','2'); INSERT INTO conversation_messages VALUES('u','c','user','違う','3'); INSERT INTO rr_roots(root_id,conversation_id,policy_id,phase,origin,presentation_mode,started_at_ms,scope_digest,result_message_id) VALUES('r','c','p','completed','text','visual',1,'','routed'); INSERT INTO rr_events VALUES('r',1,'answer_committed','{}',1);").expect("rows");
        assert!(!record_feedback_for_latest_answer(
            &connection,
            "c",
            "u",
            "answer_challenge",
            0,
            6,
            3
        )
        .expect("legacy ignored"));
        connection
            .execute("DELETE FROM conversation_messages WHERE id='legacy'", [])
            .expect("remove legacy");
        assert!(record_feedback_for_latest_answer(
            &connection,
            "c",
            "u",
            "answer_challenge",
            0,
            6,
            3
        )
        .expect("routed feedback"));
    }

    #[test]
    fn rr_23_wrong_conversation_target_is_rejected() {
        let connection = Connection::open_in_memory().expect("connection");
        connection.execute_batch("PRAGMA foreign_keys=ON; CREATE TABLE conversations(id TEXT PRIMARY KEY); CREATE TABLE runtime_runs(id TEXT PRIMARY KEY); CREATE TABLE conversation_messages(id TEXT PRIMARY KEY,conversation_id TEXT,role TEXT,content TEXT,created_at TEXT); INSERT INTO conversations VALUES('c1'); INSERT INTO conversations VALUES('c2');").expect("base");
        crate::role_routing::schema::migrate(&connection).expect("routing");
        crate::role_routing::learning::schema::migrate(&connection).expect("learning");
        connection
            .execute(
                "INSERT INTO rr_policy_versions VALUES('p',1,'{}','d',1)",
                [],
            )
            .expect("policy");
        connection.execute_batch("INSERT INTO conversation_messages VALUES('a','c2','assistant','answer','1'); INSERT INTO conversation_messages VALUES('u','c1','user','違う','2'); INSERT INTO rr_roots(root_id,conversation_id,policy_id,phase,origin,presentation_mode,started_at_ms,scope_digest,result_message_id) VALUES('r','c2','p','completed','text','visual',1,'','a');").expect("rows");
        assert!(
            record_feedback(&connection, "c1", "u", "a", "explicit_negative", 0, 6, 3).is_err()
        );
        assert_eq!(
            connection
                .query_row("SELECT count(*) FROM rr_feedback", [], |row| row
                    .get::<_, i64>(0))
                .expect("feedback count"),
            0
        );
    }

    #[test]
    fn rr_24_sol_reviewed_by_qwen_and_saved_against_scoped_evidence() {
        let connection = Connection::open_in_memory().expect("connection");
        connection.execute_batch("PRAGMA foreign_keys=ON; CREATE TABLE conversations(id TEXT PRIMARY KEY); CREATE TABLE runtime_runs(id TEXT PRIMARY KEY); CREATE TABLE conversation_messages(id TEXT PRIMARY KEY); INSERT INTO conversations VALUES('c');").expect("base");
        crate::role_routing::schema::migrate(&connection).expect("routing");
        connection
            .execute(
                "INSERT INTO rr_policy_versions VALUES('p',1,'{}','d',1)",
                [],
            )
            .expect("policy");
        connection.execute_batch("INSERT INTO rr_roots(root_id,conversation_id,policy_id,phase,origin,presentation_mode,started_at_ms,scope_digest) VALUES('r','c','p','completed','text','visual',1,''); INSERT INTO rr_steps(id,root_id,revision,ordinal,actor_id,purpose,status,config_fingerprint,adapter_state_json) VALUES('author','r',0,0,'sol','respond','succeeded','{}','{}'),('reviewer','r',0,1,'qwen','review','succeeded','{}','{}'); INSERT INTO rr_outputs(id,step_id,revision,kind,payload_json,accepted,created_at_ms) VALUES('answer-1','author',0,'answer','{}',1,1);").expect("review fixture");
        let response = crate::role_routing::review::ReviewResponse {
            issues: vec![crate::role_routing::review::ReviewIssue {
                kind: "evidence".into(),
                claim: "missing citation".into(),
                severity: "major".into(),
                code: "missing-citation".into(),
                evidence_ref: "answer-1".into(),
                verdict: "verified".into(),
            }],
        };
        assert!(record_review_response(&connection, "reviewer", &response, 2).expect("saved"));
        assert!(
            !record_review_response(&connection, "reviewer", &response, 3).expect("exact retry")
        );
        let mut conflicting = response.clone();
        conflicting.issues[0].claim = "different finding".into();
        assert!(record_review_response(&connection, "reviewer", &conflicting, 4).is_err());
        assert_eq!(
            connection
                .query_row(
                    "SELECT kind FROM rr_outputs WHERE step_id='reviewer'",
                    [],
                    |row| row.get::<_, String>(0)
                )
                .expect("output"),
            "review"
        );
    }

    #[test]
    fn rr_25_review_then_revise_records_only_verified_issues() {
        let connection = Connection::open_in_memory().expect("connection");
        connection.execute_batch("PRAGMA foreign_keys=ON; CREATE TABLE conversations(id TEXT PRIMARY KEY); CREATE TABLE runtime_runs(id TEXT PRIMARY KEY); CREATE TABLE conversation_messages(id TEXT PRIMARY KEY); INSERT INTO conversations VALUES('c');").expect("base");
        crate::role_routing::schema::migrate(&connection).expect("routing");
        connection.execute_batch("INSERT INTO rr_policy_versions VALUES('p',1,'{}','d',1); INSERT INTO rr_roots(root_id,conversation_id,policy_id,phase,origin,presentation_mode,started_at_ms,scope_digest) VALUES('r','c','p','completed','text','visual',1,''); INSERT INTO rr_steps(id,root_id,revision,ordinal,actor_id,purpose,status,config_fingerprint,adapter_state_json) VALUES('author','r',0,0,'sol','respond','succeeded','{}','{}'),('reviewer','r',0,1,'qwen','review','succeeded','{}','{}'); INSERT INTO rr_outputs(id,step_id,revision,kind,payload_json,accepted,created_at_ms) VALUES('answer-1','author',0,'answer','{\"hostVerification\":\"verified\",\"verifierVersion\":\"fixture-v1\"}',1,1);").expect("fixture");
        let review = crate::role_routing::review::ReviewResponse {
            issues: vec![
                crate::role_routing::review::ReviewIssue {
                    kind: "evidence".into(),
                    claim: "citation missing".into(),
                    severity: "major".into(),
                    code: "citation".into(),
                    evidence_ref: "answer-1".into(),
                    verdict: "verified".into(),
                },
                crate::role_routing::review::ReviewIssue {
                    kind: "logic".into(),
                    claim: "style is terse".into(),
                    severity: "minor".into(),
                    code: "style".into(),
                    evidence_ref: "answer-1".into(),
                    verdict: "unresolved".into(),
                },
            ],
        };
        record_review_response(&connection, "reviewer", &review, 2).expect("review");
        let output_id: String = connection
            .query_row(
                "SELECT id FROM rr_outputs WHERE step_id='reviewer' AND kind='review'",
                [],
                |row| row.get(0),
            )
            .expect("review id");
        let decision =
            record_review_revision_decision(&connection, &output_id, 1, 3).expect("decision");
        assert!(decision.revision_allowed);
        assert_eq!(decision.verified_issues.len(), 1);
        assert_eq!(decision.unresolved_issues.len(), 1);
        assert_eq!(
            record_review_revision_decision(&connection, &output_id, 1, 4).expect("exact retry"),
            decision
        );
        assert!(record_review_revision_decision(&connection, &output_id, 2, 5).is_err());
        assert_eq!(
            connection
                .query_row(
                    "SELECT count(*) FROM rr_outputs WHERE kind='revision-decision'",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .expect("stored"),
            1
        );
    }
}
