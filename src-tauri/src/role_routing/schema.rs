//! Additive SQLite ledger for role-routing. It contains no execution side effects.
use rusqlite::Connection;

pub(crate) fn migrate(connection: &Connection) -> rusqlite::Result<()> {
    connection.execute_batch("CREATE TABLE IF NOT EXISTS rr_policy_versions (id TEXT PRIMARY KEY, version INTEGER NOT NULL UNIQUE, config_json TEXT NOT NULL CHECK(json_valid(config_json)), digest TEXT NOT NULL, created_at_ms INTEGER NOT NULL);
    CREATE INDEX IF NOT EXISTS idx_rr_policy_versions_digest ON rr_policy_versions(digest);
    CREATE TABLE IF NOT EXISTS rr_roots (root_id TEXT PRIMARY KEY, conversation_id TEXT NOT NULL, runtime_run_id TEXT, policy_id TEXT NOT NULL, revision INTEGER NOT NULL DEFAULT 0 CHECK(revision >= 0), phase TEXT NOT NULL, active_slot TEXT, origin TEXT NOT NULL, presentation_mode TEXT NOT NULL, started_at_ms INTEGER NOT NULL, deadline_at_ms INTEGER, cancel_requested INTEGER NOT NULL DEFAULT 0 CHECK(cancel_requested IN (0,1)), drain_reason TEXT, result_message_id TEXT, previous_root_id TEXT, target_answer_id TEXT, scope_digest TEXT NOT NULL, FOREIGN KEY(conversation_id) REFERENCES conversations(id) ON DELETE CASCADE, FOREIGN KEY(runtime_run_id) REFERENCES runtime_runs(id) ON DELETE SET NULL, FOREIGN KEY(policy_id) REFERENCES rr_policy_versions(id), FOREIGN KEY(result_message_id) REFERENCES conversation_messages(id) ON DELETE SET NULL, FOREIGN KEY(previous_root_id) REFERENCES rr_roots(root_id) ON DELETE SET NULL, FOREIGN KEY(target_answer_id) REFERENCES conversation_messages(id) ON DELETE SET NULL);
    CREATE INDEX IF NOT EXISTS idx_rr_roots_conversation_started ON rr_roots(conversation_id, started_at_ms DESC);
    CREATE UNIQUE INDEX IF NOT EXISTS idx_rr_roots_one_active_per_conversation ON rr_roots(conversation_id) WHERE phase IN ('queued','responding','draining');
    CREATE TABLE IF NOT EXISTS rr_inputs (input_id TEXT PRIMARY KEY, root_id TEXT, conversation_id TEXT NOT NULL, message_id TEXT NOT NULL, payload_digest TEXT NOT NULL, origin TEXT NOT NULL, source_id TEXT, disposition TEXT NOT NULL, received_at_ms INTEGER NOT NULL, UNIQUE(conversation_id, input_id), FOREIGN KEY(root_id) REFERENCES rr_roots(root_id) ON DELETE CASCADE, FOREIGN KEY(conversation_id) REFERENCES conversations(id) ON DELETE CASCADE, FOREIGN KEY(message_id) REFERENCES conversation_messages(id) ON DELETE CASCADE);
    CREATE TABLE IF NOT EXISTS rr_decisions (id TEXT PRIMARY KEY, root_id TEXT NOT NULL, revision INTEGER NOT NULL, input_id TEXT, features_json TEXT NOT NULL CHECK(json_valid(features_json)), candidates_json TEXT NOT NULL CHECK(json_valid(candidates_json)), selected_id TEXT, action TEXT NOT NULL, reason_codes_json TEXT NOT NULL CHECK(json_valid(reason_codes_json)), ranker_version TEXT NOT NULL, policy_id TEXT NOT NULL, created_at_ms INTEGER NOT NULL, FOREIGN KEY(root_id) REFERENCES rr_roots(root_id) ON DELETE CASCADE, FOREIGN KEY(input_id) REFERENCES rr_inputs(input_id) ON DELETE SET NULL, FOREIGN KEY(policy_id) REFERENCES rr_policy_versions(id));
    CREATE TABLE IF NOT EXISTS rr_steps (id TEXT PRIMARY KEY, root_id TEXT NOT NULL, decision_id TEXT, revision INTEGER NOT NULL, ordinal INTEGER NOT NULL, actor_id TEXT NOT NULL, purpose TEXT NOT NULL, status TEXT NOT NULL CHECK(status IN ('planned','running','draining','succeeded','failed','cancelled','interrupted')), cancel_requested INTEGER NOT NULL DEFAULT 0 CHECK(cancel_requested IN (0,1)), config_fingerprint TEXT NOT NULL, adapter_state_json TEXT NOT NULL CHECK(json_valid(adapter_state_json)), started_at_ms INTEGER, completed_at_ms INTEGER, error_code TEXT, usage_json TEXT NOT NULL DEFAULT '{}' CHECK(json_valid(usage_json)), UNIQUE(root_id, ordinal), FOREIGN KEY(root_id) REFERENCES rr_roots(root_id) ON DELETE CASCADE, FOREIGN KEY(decision_id) REFERENCES rr_decisions(id) ON DELETE SET NULL);
    CREATE INDEX IF NOT EXISTS idx_rr_steps_running ON rr_steps(root_id, status, ordinal);
    CREATE TABLE IF NOT EXISTS rr_outputs (id TEXT PRIMARY KEY, step_id TEXT NOT NULL, revision INTEGER NOT NULL, kind TEXT NOT NULL, payload_json TEXT NOT NULL CHECK(json_valid(payload_json)), accepted INTEGER NOT NULL DEFAULT 0 CHECK(accepted IN (0,1)), created_at_ms INTEGER NOT NULL, FOREIGN KEY(step_id) REFERENCES rr_steps(id) ON DELETE CASCADE);
    CREATE TABLE IF NOT EXISTS rr_events (root_id TEXT NOT NULL, seq INTEGER NOT NULL CHECK(seq > 0), kind TEXT NOT NULL, data_json TEXT NOT NULL CHECK(json_valid(data_json)), created_at_ms INTEGER NOT NULL, PRIMARY KEY(root_id, seq), FOREIGN KEY(root_id) REFERENCES rr_roots(root_id) ON DELETE CASCADE);
    CREATE TABLE IF NOT EXISTS rr_feedback (id TEXT PRIMARY KEY, target_answer_id TEXT NOT NULL, target_root_id TEXT, source_message_id TEXT NOT NULL, kind TEXT NOT NULL, evidence_json TEXT NOT NULL CHECK(json_valid(evidence_json)), label_source TEXT NOT NULL, confidence REAL, extractor_version TEXT NOT NULL, status TEXT NOT NULL, created_at_ms INTEGER NOT NULL, UNIQUE(target_answer_id, source_message_id, kind, extractor_version), FOREIGN KEY(target_answer_id) REFERENCES conversation_messages(id) ON DELETE CASCADE, FOREIGN KEY(target_root_id) REFERENCES rr_roots(root_id) ON DELETE CASCADE, FOREIGN KEY(source_message_id) REFERENCES conversation_messages(id) ON DELETE CASCADE);")
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn migration_is_idempotent() {
        let c = Connection::open_in_memory().expect("connection");
        c.execute_batch("PRAGMA foreign_keys=ON; CREATE TABLE conversations(id TEXT PRIMARY KEY); CREATE TABLE runtime_runs(id TEXT PRIMARY KEY); CREATE TABLE conversation_messages(id TEXT PRIMARY KEY);").expect("base");
        migrate(&c).expect("first");
        migrate(&c).expect("second");
        let count: i64 = c
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='rr_roots'",
                [],
                |row| row.get(0),
            )
            .expect("query");
        assert_eq!(count, 1);
    }

    #[test]
    fn rr_02_one_active_root_per_conversation() {
        let c = Connection::open_in_memory().expect("connection");
        c.execute_batch("PRAGMA foreign_keys=ON; CREATE TABLE conversations(id TEXT PRIMARY KEY); CREATE TABLE runtime_runs(id TEXT PRIMARY KEY); CREATE TABLE conversation_messages(id TEXT PRIMARY KEY); INSERT INTO conversations VALUES('c');").expect("base");
        migrate(&c).expect("migration");
        c.execute(
            "INSERT INTO rr_policy_versions VALUES('p',1,'{}','d',1)",
            [],
        )
        .expect("policy");
        c.execute("INSERT INTO rr_roots(root_id,conversation_id,policy_id,phase,origin,presentation_mode,started_at_ms,scope_digest) VALUES('r1','c','p','responding','text','visual',1,'')", []).expect("first root");
        assert!(c.execute("INSERT INTO rr_roots(root_id,conversation_id,policy_id,phase,origin,presentation_mode,started_at_ms,scope_digest) VALUES('r2','c','p','queued','text','visual',2,'')", []).is_err());
    }
}
