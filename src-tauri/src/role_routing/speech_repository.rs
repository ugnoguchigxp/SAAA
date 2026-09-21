//! Durable ownership for role-routed speech. Audio bytes remain owned by the existing TTS
//! runtime; this ledger only decides which committed output and epoch may play.
use rusqlite::{params, Connection, OptionalExtension};

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

pub(crate) fn enqueue_final(
    connection: &Connection,
    root_id: &str,
    message_id: &str,
) -> Result<bool, String> {
    let row: Option<(String, i64, String, String)> = connection
        .query_row(
            "SELECT r.conversation_id,r.revision,o.id,s.actor_id
             FROM rr_roots r JOIN rr_steps s ON s.root_id=r.root_id AND s.revision=r.revision
             JOIN rr_outputs o ON o.step_id=s.id AND o.revision=r.revision AND o.accepted=1
             WHERE r.root_id=?1 AND r.phase='completed' AND r.result_message_id=?2
             ORDER BY s.ordinal DESC,o.created_at_ms DESC LIMIT 1",
            params![root_id, message_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    let Some((conversation_id, revision, output_id, actor_id)) = row else {
        return Ok(false);
    };
    let already_recorded: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM rr_speech WHERE root_id=?1 AND revision=?2 AND kind='final')",
            params![root_id, revision],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if already_recorded {
        return Ok(false);
    }
    let timestamp = now_ms();
    let epoch: i64 = connection.query_row(
        "SELECT COALESCE(MAX(s.epoch),0)+1 FROM rr_speech s JOIN rr_roots r ON r.root_id=s.root_id WHERE r.conversation_id=?1",
        [&conversation_id],
        |row| row.get(0),
    ).map_err(|error| error.to_string())?;
    connection.execute(
        "UPDATE rr_speech SET status='cancelled',updated_at_ms=?1 WHERE status IN ('queued','playing') AND root_id IN (SELECT root_id FROM rr_roots WHERE conversation_id=?2)",
        params![timestamp,conversation_id],
    ).map_err(|error| error.to_string())?;
    let id = format!("rr-speech-{root_id}-{revision}-final");
    let changed = connection.execute(
        "INSERT OR IGNORE INTO rr_speech(id,root_id,revision,epoch,kind,status,output_id,speaker_actor_id,created_at_ms,updated_at_ms) VALUES(?1,?2,?3,?4,'final','queued',?5,?6,?7,?7)",
        params![id,root_id,revision,epoch,output_id,actor_id,timestamp],
    ).map_err(|error| error.to_string())?;
    Ok(changed == 1)
}

pub(crate) fn mark_started(connection: &Connection, root_id: &str) -> Result<bool, String> {
    let timestamp = now_ms();
    let changed = connection.execute(
        "UPDATE rr_speech SET status='playing',updated_at_ms=?1
         WHERE root_id=?2 AND status='queued'
           AND epoch=(SELECT MAX(s.epoch) FROM rr_speech s JOIN rr_roots r ON r.root_id=s.root_id WHERE r.conversation_id=(SELECT conversation_id FROM rr_roots WHERE root_id=?2))
           AND NOT EXISTS(SELECT 1 FROM rr_speech WHERE status='playing')",
        params![timestamp,root_id],
    ).map_err(|error| error.to_string())?;
    Ok(changed == 1)
}

pub(crate) fn has_intent(connection: &Connection, root_id: &str) -> Result<bool, String> {
    connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM rr_speech WHERE root_id=?1)",
            [root_id],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())
}

pub(crate) fn mark_terminal(
    connection: &Connection,
    root_id: &str,
    status: &str,
) -> Result<bool, String> {
    if !matches!(status, "completed" | "cancelled" | "failed") {
        return Err("Role-routing speech terminal status is invalid".into());
    }
    let changed = connection.execute(
        "UPDATE rr_speech SET status=?1,updated_at_ms=?2 WHERE root_id=?3 AND status IN ('queued','playing')",
        params![status,now_ms(),root_id],
    ).map_err(|error| error.to_string())?;
    Ok(changed == 1)
}

/// Barge-in fence for a newly accepted user turn. The durable cancellation commits before the
/// caller asks the process-local TTS runtime to stop each previous run.
pub(crate) fn cancel_conversation(
    connection: &Connection,
    conversation_id: &str,
) -> Result<Vec<String>, String> {
    let run_ids = connection
        .prepare(
            "SELECT DISTINCT COALESCE(r.runtime_run_id,r.root_id)
             FROM rr_speech s JOIN rr_roots r ON r.root_id=s.root_id
             WHERE r.conversation_id=?1 AND s.status IN ('queued','playing')",
        )
        .map_err(|error| error.to_string())?
        .query_map([conversation_id], |row| row.get::<_, String>(0))
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    connection
        .execute(
            "UPDATE rr_speech SET status='cancelled',updated_at_ms=?1
             WHERE status IN ('queued','playing') AND root_id IN
               (SELECT root_id FROM rr_roots WHERE conversation_id=?2)",
            params![now_ms(), conversation_id],
        )
        .map_err(|error| error.to_string())?;
    Ok(run_ids)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rr_29_old_speech_end_new_owner() {
        let connection = Connection::open_in_memory().expect("db");
        connection.execute_batch("PRAGMA foreign_keys=ON;CREATE TABLE conversations(id TEXT PRIMARY KEY);CREATE TABLE runtime_runs(id TEXT PRIMARY KEY);CREATE TABLE conversation_messages(id TEXT PRIMARY KEY);INSERT INTO conversations VALUES('c');INSERT INTO conversation_messages VALUES('a1');INSERT INTO conversation_messages VALUES('a2');").expect("base");
        crate::role_routing::schema::migrate(&connection).expect("schema");
        connection
            .execute(
                "INSERT INTO rr_policy_versions VALUES('p',1,'{}','d',1)",
                [],
            )
            .expect("policy");
        for (root, message, started) in [("r1", "a1", 1), ("r2", "a2", 2)] {
            connection.execute("INSERT INTO rr_roots(root_id,conversation_id,policy_id,revision,phase,origin,presentation_mode,started_at_ms,scope_digest,result_message_id) VALUES(?1,'c','p',0,'completed','text','voice',?3,'',?2)", params![root,message,started]).expect("root");
            connection.execute("INSERT INTO rr_steps(id,root_id,revision,ordinal,actor_id,purpose,status,config_fingerprint,adapter_state_json) VALUES(?1,?2,0,0,'author','respond','succeeded','f','{}')", params![format!("step-{root}"),root]).expect("step");
            connection.execute("INSERT INTO rr_outputs(id,step_id,revision,kind,payload_json,accepted,created_at_ms) VALUES(?1,?2,0,'answer','{}',1,?3)", params![format!("output-{root}"),format!("step-{root}"),started]).expect("output");
        }
        assert!(enqueue_final(&connection, "r1", "a1").expect("first"));
        assert!(!enqueue_final(&connection, "r1", "a1").expect("duplicate"));
        assert!(mark_started(&connection, "r1").expect("first starts"));
        assert!(enqueue_final(&connection, "r2", "a2").expect("second"));
        assert!(mark_started(&connection, "r2").expect("second starts"));
        assert!(!mark_terminal(&connection, "r1", "completed").expect("late old end"));
        assert_eq!(
            connection
                .query_row(
                    "SELECT root_id||':'||status FROM rr_speech WHERE status='playing'",
                    [],
                    |row| row.get::<_, String>(0)
                )
                .expect("owner"),
            "r2:playing"
        );
        assert_eq!(
            cancel_conversation(&connection, "c").expect("barge-in"),
            vec!["r2"]
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT count(*) FROM rr_speech WHERE status='playing'",
                    [],
                    |row| { row.get::<_, i64>(0) }
                )
                .expect("playing count"),
            0
        );
    }
}
