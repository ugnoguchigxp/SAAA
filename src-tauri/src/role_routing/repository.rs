//! Persistence helpers for immutable policy snapshots.

#[path = "repository_policy.rs"]
mod repository_policy;
#[path = "repository_turns.rs"]
mod repository_turns;

pub(crate) use repository_policy::capture_current_policy;
pub(crate) use repository_turns::{
    accept_provider_turn, record_actor_activity, record_provider_turn_finish,
    record_provider_turn_start, record_provider_turn_start_in_transaction,
};

use rusqlite::{params, Connection, OptionalExtension};
use serde_json::json;
use sha2::{Digest, Sha256};

/// Records host-validated feedback. The caller must identify both message ids; text matching is
/// intentionally not used because a quoted old answer is not evidence about the current one.
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
    let root_id: Option<String> = connection.query_row(
        "SELECT r.root_id FROM conversation_messages m LEFT JOIN rr_roots r ON r.result_message_id=m.id WHERE m.id=?1 AND m.conversation_id=?2 AND m.role='assistant'",
        params![target_answer_id, conversation_id], |r| r.get(0),
    ).optional().map_err(|_| "Feedback target answer is unavailable".to_string())?.flatten();
    let digest = format!(
        "{:x}",
        Sha256::digest(format!("{target_answer_id}:{source_message_id}:{kind}").as_bytes())
    );
    let inserted = connection.execute(
        "INSERT OR IGNORE INTO rr_feedback(id,target_answer_id,target_root_id,source_message_id,kind,evidence_json,label_source,confidence,extractor_version,status,created_at_ms) VALUES(?1,?2,?3,?4,?5,?6,'host',1.0,'host-v1','recorded',?7)",
        params![format!("rr-feedback-{}", &digest[..24]),target_answer_id,root_id,source_message_id,kind,json!({"start":evidence_start,"end":evidence_end}).to_string(),now_ms],
    ).map_err(|e| e.to_string())? == 1;
    if inserted {
        if let Some(root_id) = root_id {
            crate::role_routing::learning::repository::mark_root_dirty(connection, &root_id)?;
        }
    }
    Ok(inserted)
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
        assert_eq!(
            connection
                .query_row("SELECT root_id FROM rr_learning_dirty", [], |r| r
                    .get::<_, String>(0))
                .expect("dirty"),
            "r"
        );
        assert!(record_feedback(&connection, "c", "u", "a", "answer_challenge", 8, 8, 2).is_err());
    }
}
