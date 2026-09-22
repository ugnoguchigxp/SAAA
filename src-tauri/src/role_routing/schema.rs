//! Additive SQLite ledger for role-routing. It contains no execution side effects.
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;

pub(crate) fn migrate(connection: &Connection) -> rusqlite::Result<()> {
    connection.execute_batch("CREATE TABLE IF NOT EXISTS rr_policy_versions (id TEXT PRIMARY KEY, version INTEGER NOT NULL UNIQUE, config_json TEXT NOT NULL CHECK(json_valid(config_json)), digest TEXT NOT NULL, created_at_ms INTEGER NOT NULL);
    CREATE INDEX IF NOT EXISTS idx_rr_policy_versions_digest ON rr_policy_versions(digest);
    CREATE TABLE IF NOT EXISTS rr_roots (root_id TEXT PRIMARY KEY, conversation_id TEXT NOT NULL, runtime_run_id TEXT, policy_id TEXT NOT NULL, revision INTEGER NOT NULL DEFAULT 0 CHECK(revision >= 0), phase TEXT NOT NULL, active_slot TEXT, origin TEXT NOT NULL, presentation_mode TEXT NOT NULL, started_at_ms INTEGER NOT NULL, deadline_at_ms INTEGER, cancel_requested INTEGER NOT NULL DEFAULT 0 CHECK(cancel_requested IN (0,1)), drain_reason TEXT, result_message_id TEXT, previous_root_id TEXT, target_answer_id TEXT, scope_digest TEXT NOT NULL, FOREIGN KEY(conversation_id) REFERENCES conversations(id) ON DELETE CASCADE, FOREIGN KEY(runtime_run_id) REFERENCES runtime_runs(id) ON DELETE SET NULL, FOREIGN KEY(policy_id) REFERENCES rr_policy_versions(id), FOREIGN KEY(result_message_id) REFERENCES conversation_messages(id) ON DELETE SET NULL, FOREIGN KEY(previous_root_id) REFERENCES rr_roots(root_id) ON DELETE SET NULL, FOREIGN KEY(target_answer_id) REFERENCES conversation_messages(id) ON DELETE SET NULL);
    CREATE INDEX IF NOT EXISTS idx_rr_roots_conversation_started ON rr_roots(conversation_id, started_at_ms DESC);
    DROP INDEX IF EXISTS idx_rr_roots_one_active_per_conversation;
    CREATE UNIQUE INDEX idx_rr_roots_one_active_per_conversation ON rr_roots(conversation_id) WHERE phase IN ('responding','draining');
    CREATE TABLE IF NOT EXISTS rr_inputs (input_id TEXT PRIMARY KEY, root_id TEXT, conversation_id TEXT NOT NULL, message_id TEXT NOT NULL, payload_digest TEXT NOT NULL, origin TEXT NOT NULL, source_id TEXT, disposition TEXT NOT NULL, generation INTEGER NOT NULL DEFAULT 0, received_at_ms INTEGER NOT NULL, UNIQUE(conversation_id, input_id), FOREIGN KEY(root_id) REFERENCES rr_roots(root_id) ON DELETE CASCADE, FOREIGN KEY(conversation_id) REFERENCES conversations(id) ON DELETE CASCADE, FOREIGN KEY(message_id) REFERENCES conversation_messages(id) ON DELETE CASCADE);
    CREATE TABLE IF NOT EXISTS rr_decisions (id TEXT PRIMARY KEY, root_id TEXT NOT NULL, revision INTEGER NOT NULL, input_id TEXT, features_json TEXT NOT NULL CHECK(json_valid(features_json)), candidates_json TEXT NOT NULL CHECK(json_valid(candidates_json)), selected_id TEXT, action TEXT NOT NULL, reason_codes_json TEXT NOT NULL CHECK(json_valid(reason_codes_json)), ranker_version TEXT NOT NULL, policy_id TEXT NOT NULL, created_at_ms INTEGER NOT NULL, FOREIGN KEY(root_id) REFERENCES rr_roots(root_id) ON DELETE CASCADE, FOREIGN KEY(input_id) REFERENCES rr_inputs(input_id) ON DELETE SET NULL, FOREIGN KEY(policy_id) REFERENCES rr_policy_versions(id));
    CREATE TABLE IF NOT EXISTS rr_steps (id TEXT PRIMARY KEY, root_id TEXT NOT NULL, decision_id TEXT, revision INTEGER NOT NULL, ordinal INTEGER NOT NULL, actor_id TEXT NOT NULL, purpose TEXT NOT NULL, status TEXT NOT NULL CHECK(status IN ('planned','running','draining','succeeded','failed','cancelled','interrupted')), cancel_requested INTEGER NOT NULL DEFAULT 0 CHECK(cancel_requested IN (0,1)), config_fingerprint TEXT NOT NULL, adapter_state_json TEXT NOT NULL CHECK(json_valid(adapter_state_json)), started_at_ms INTEGER, completed_at_ms INTEGER, error_code TEXT, usage_json TEXT NOT NULL DEFAULT '{}' CHECK(json_valid(usage_json)), UNIQUE(root_id, ordinal), FOREIGN KEY(root_id) REFERENCES rr_roots(root_id) ON DELETE CASCADE, FOREIGN KEY(decision_id) REFERENCES rr_decisions(id) ON DELETE SET NULL);
    CREATE INDEX IF NOT EXISTS idx_rr_steps_running ON rr_steps(root_id, status, ordinal);
    DROP INDEX IF EXISTS idx_rr_steps_one_active_reasoning;
    CREATE UNIQUE INDEX idx_rr_steps_one_active_reasoning ON rr_steps(root_id) WHERE status='running' AND purpose IN ('respond','reconsider','review','revise','tool_specialist');
    CREATE UNIQUE INDEX IF NOT EXISTS idx_rr_steps_id_root ON rr_steps(id, root_id);
    CREATE TABLE IF NOT EXISTS rr_outputs (id TEXT PRIMARY KEY, step_id TEXT NOT NULL, revision INTEGER NOT NULL, kind TEXT NOT NULL, payload_json TEXT NOT NULL CHECK(json_valid(payload_json)), accepted INTEGER NOT NULL DEFAULT 0 CHECK(accepted IN (0,1)), created_at_ms INTEGER NOT NULL, FOREIGN KEY(step_id) REFERENCES rr_steps(id) ON DELETE CASCADE);
    CREATE TABLE IF NOT EXISTS rr_premium_proposals (id TEXT PRIMARY KEY, root_id TEXT NOT NULL, candidate_id TEXT NOT NULL, policy_id TEXT NOT NULL, revision INTEGER NOT NULL, estimated_cost_micros INTEGER, expires_at_ms INTEGER NOT NULL, status TEXT NOT NULL CHECK(status IN ('proposed','approved','declined','expired')), created_at_ms INTEGER NOT NULL, approved_at_ms INTEGER, consumed_at_ms INTEGER, FOREIGN KEY(root_id) REFERENCES rr_roots(root_id) ON DELETE CASCADE, FOREIGN KEY(policy_id) REFERENCES rr_policy_versions(id));
    CREATE UNIQUE INDEX IF NOT EXISTS idx_rr_premium_proposals_active_root ON rr_premium_proposals(root_id) WHERE status='proposed';
    CREATE TABLE IF NOT EXISTS rr_events (root_id TEXT NOT NULL, seq INTEGER NOT NULL CHECK(seq > 0), kind TEXT NOT NULL, data_json TEXT NOT NULL CHECK(json_valid(data_json)), created_at_ms INTEGER NOT NULL, PRIMARY KEY(root_id, seq), FOREIGN KEY(root_id) REFERENCES rr_roots(root_id) ON DELETE CASCADE);
    CREATE TABLE IF NOT EXISTS rr_tool_links (id TEXT PRIMARY KEY, root_id TEXT NOT NULL, step_id TEXT NOT NULL, revision INTEGER NOT NULL, operation_key TEXT NOT NULL, invocation_id TEXT, dispatch_state TEXT NOT NULL CHECK(dispatch_state IN ('reserved','dispatched','settled','unknown')), result_ref TEXT, created_at_ms INTEGER NOT NULL, updated_at_ms INTEGER NOT NULL, UNIQUE(root_id, operation_key), FOREIGN KEY(root_id) REFERENCES rr_roots(root_id) ON DELETE CASCADE, FOREIGN KEY(step_id, root_id) REFERENCES rr_steps(id, root_id) ON DELETE CASCADE);
    CREATE INDEX IF NOT EXISTS idx_rr_tool_links_invocation ON rr_tool_links(invocation_id);
    CREATE TABLE IF NOT EXISTS rr_feedback (id TEXT PRIMARY KEY, target_answer_id TEXT NOT NULL, target_root_id TEXT, source_message_id TEXT NOT NULL, kind TEXT NOT NULL, evidence_json TEXT NOT NULL CHECK(json_valid(evidence_json)), label_source TEXT NOT NULL, confidence REAL, extractor_version TEXT NOT NULL, status TEXT NOT NULL, created_at_ms INTEGER NOT NULL, UNIQUE(target_answer_id, source_message_id, kind, extractor_version), FOREIGN KEY(target_answer_id) REFERENCES conversation_messages(id) ON DELETE CASCADE, FOREIGN KEY(target_root_id) REFERENCES rr_roots(root_id) ON DELETE CASCADE, FOREIGN KEY(source_message_id) REFERENCES conversation_messages(id) ON DELETE CASCADE);
    CREATE TABLE IF NOT EXISTS rr_speech (id TEXT PRIMARY KEY, root_id TEXT NOT NULL, revision INTEGER NOT NULL, epoch INTEGER NOT NULL CHECK(epoch > 0), kind TEXT NOT NULL CHECK(kind IN ('ack','progress','final')), status TEXT NOT NULL CHECK(status IN ('queued','playing','completed','cancelled','failed')), output_id TEXT, speaker_actor_id TEXT NOT NULL, created_at_ms INTEGER NOT NULL, updated_at_ms INTEGER NOT NULL, UNIQUE(root_id,revision,kind), FOREIGN KEY(root_id) REFERENCES rr_roots(root_id) ON DELETE CASCADE, FOREIGN KEY(output_id) REFERENCES rr_outputs(id) ON DELETE SET NULL);
    CREATE UNIQUE INDEX IF NOT EXISTS idx_rr_speech_one_playing ON rr_speech((1)) WHERE status='playing';")?;
    ensure_rr_input_generation(connection)?;
    ensure_rr_proposal_consumed(connection)?;
    Ok(())
}

/// Existing installations used the dynamic harness for both the conversational frontend and the
/// reasoner. LFM is now bypassed, so move only that shipped reasoner binding to an explicitly
/// configured direct Qwen provider. Custom actor assignments and databases without the direct
/// provider are left untouched.
pub(crate) fn migrate_v33_to_v34_direct_qwen_reasoner(
    connection: &Connection,
    previous_version: i64,
) -> rusqlite::Result<()> {
    if previous_version >= 34 {
        return Ok(());
    }
    let documents: Option<(String, String)> = connection
        .query_row(
            "SELECT providers.value_json, roles.value_json
             FROM settings_documents providers
             JOIN settings_documents roles
               ON roles.namespace='routing.roles' AND roles.key='default'
             WHERE providers.namespace='providers.model' AND providers.key='default'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let Some((providers_json, roles_json)) = documents else {
        return Ok(());
    };
    let providers: Value = match serde_json::from_str(&providers_json) {
        Ok(value) => value,
        Err(_) => return Ok(()),
    };
    let direct_model = providers["providers"]
        .as_array()
        .and_then(|items| {
            items.iter().find(|provider| {
                provider["id"] == crate::QWEN_DIRECT_PROVIDER_ID
                    && provider["kind"] == "openai-compatible"
                    && provider["enabled"] == true
            })
        })
        .and_then(|provider| provider["model"].as_str());
    let Some(direct_model) = direct_model else {
        return Ok(());
    };
    let mut roles: Value = match serde_json::from_str(&roles_json) {
        Ok(value) => value,
        Err(_) => return Ok(()),
    };
    let Some(reasoner_id) = roles["roles"]["reasoner"].as_str().map(str::to_string) else {
        return Ok(());
    };
    let Some(reasoner) = roles["actors"].as_array_mut().and_then(|actors| {
        actors.iter_mut().find(|actor| {
            actor["id"] == reasoner_id
                && actor["transport"] == "provider"
                && actor["providerId"] == crate::DYNAMIC_LAN_PROVIDER_ID
        })
    }) else {
        return Ok(());
    };
    reasoner["providerId"] = Value::String(crate::QWEN_DIRECT_PROVIDER_ID.into());
    reasoner["model"] = Value::String(direct_model.into());
    reasoner["label"] = Value::String("Qwen3.8 27B (direct)".into());
    connection.execute(
        "UPDATE settings_documents
         SET value_json=?1, updated_at=?2
         WHERE namespace='routing.roles' AND key='default'",
        params![roles.to_string(), crate::now_iso()],
    )?;
    Ok(())
}

/// Adds the approval-consumption column to databases created before it existed. Consumption is
/// tracked with a timestamp rather than a new status so the shipped status CHECK is not rewritten.
fn ensure_rr_proposal_consumed(connection: &Connection) -> rusqlite::Result<()> {
    let exists: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('rr_premium_proposals') WHERE name='consumed_at_ms')",
        [],
        |row| row.get(0),
    )?;
    if !exists {
        connection
            .execute_batch("ALTER TABLE rr_premium_proposals ADD COLUMN consumed_at_ms INTEGER")?;
    }
    Ok(())
}

/// Adds the classifier-generation column to databases created before it existed. Shipped
/// migrations are never rewritten in place; the column is added idempotently.
fn ensure_rr_input_generation(connection: &Connection) -> rusqlite::Result<()> {
    let exists: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('rr_inputs') WHERE name='generation')",
        [],
        |row| row.get(0),
    )?;
    if !exists {
        connection.execute_batch(
            "ALTER TABLE rr_inputs ADD COLUMN generation INTEGER NOT NULL DEFAULT 0",
        )?;
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn schema_34_moves_only_the_shipped_reasoner_to_direct_qwen() {
        let c = Connection::open_in_memory().expect("connection");
        c.execute_batch(
            "CREATE TABLE settings_documents (
               namespace TEXT NOT NULL, key TEXT NOT NULL, schema_version INTEGER NOT NULL,
               value_json TEXT NOT NULL, updated_at TEXT NOT NULL,
               PRIMARY KEY(namespace,key)
             );",
        )
        .expect("settings");
        let providers = json!({"providers":[
            {"id":crate::DYNAMIC_LAN_PROVIDER_ID,"kind":"dynamic-lan","enabled":true},
            {"id":crate::QWEN_DIRECT_PROVIDER_ID,"kind":"openai-compatible","enabled":true,
             "model":"Qwen3.8-27B-ROCmFP4-FAST.gguf"}
        ]});
        let roles = json!({
            "roles":{"reasoner":"local-reasoner","frontend":"local-conversation-frontend"},
            "actors":[
                {"id":"local-reasoner","transport":"provider",
                 "providerId":crate::DYNAMIC_LAN_PROVIDER_ID,"model":null,"label":"LAN reasoning provider"},
                {"id":"local-conversation-frontend","transport":"provider",
                 "providerId":crate::DYNAMIC_LAN_PROVIDER_ID,"model":null,"label":"LFM"}
            ]
        });
        c.execute(
            "INSERT INTO settings_documents VALUES('providers.model','default',15,?1,'before')",
            [providers.to_string()],
        )
        .expect("providers");
        c.execute(
            "INSERT INTO settings_documents VALUES('routing.roles','default',15,?1,'before')",
            [roles.to_string()],
        )
        .expect("roles");

        migrate_v33_to_v34_direct_qwen_reasoner(&c, 33).expect("migration");

        let stored: String = c
            .query_row(
                "SELECT value_json FROM settings_documents WHERE namespace='routing.roles'",
                [],
                |row| row.get(0),
            )
            .expect("stored roles");
        let stored: Value = serde_json::from_str(&stored).expect("json");
        assert_eq!(
            stored["actors"][0]["providerId"],
            crate::QWEN_DIRECT_PROVIDER_ID
        );
        assert_eq!(
            stored["actors"][0]["model"],
            "Qwen3.8-27B-ROCmFP4-FAST.gguf"
        );
        assert_eq!(
            stored["actors"][1]["providerId"],
            crate::DYNAMIC_LAN_PROVIDER_ID
        );
    }

    #[test]
    fn schema_34_preserves_custom_reasoner_bindings() {
        let c = Connection::open_in_memory().expect("connection");
        c.execute_batch(
            "CREATE TABLE settings_documents (
               namespace TEXT NOT NULL, key TEXT NOT NULL, schema_version INTEGER NOT NULL,
               value_json TEXT NOT NULL, updated_at TEXT NOT NULL,
               PRIMARY KEY(namespace,key)
             );",
        )
        .expect("settings");
        let providers = json!({"providers":[
            {"id":crate::QWEN_DIRECT_PROVIDER_ID,"kind":"openai-compatible","enabled":true,
             "model":"Qwen3.8-27B-ROCmFP4-FAST.gguf"}
        ]});
        let roles = json!({
            "roles":{"reasoner":"custom"},
            "actors":[{"id":"custom","transport":"provider","providerId":"another-provider"}]
        });
        c.execute(
            "INSERT INTO settings_documents VALUES('providers.model','default',15,?1,'before')",
            [providers.to_string()],
        )
        .expect("providers");
        c.execute(
            "INSERT INTO settings_documents VALUES('routing.roles','default',15,?1,'before')",
            [roles.to_string()],
        )
        .expect("roles");

        migrate_v33_to_v34_direct_qwen_reasoner(&c, 33).expect("migration");

        let stored: String = c
            .query_row(
                "SELECT value_json FROM settings_documents WHERE namespace='routing.roles'",
                [],
                |row| row.get(0),
            )
            .expect("stored roles");
        assert_eq!(serde_json::from_str::<Value>(&stored).unwrap(), roles);
    }

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
        assert!(c.execute("INSERT INTO rr_roots(root_id,conversation_id,policy_id,phase,origin,presentation_mode,started_at_ms,scope_digest) VALUES('r2','c','p','responding','text','visual',2,'')", []).is_err());
        c.execute("INSERT INTO rr_roots(root_id,conversation_id,policy_id,phase,origin,presentation_mode,started_at_ms,scope_digest) VALUES('queued','c','p','queued','text','visual',3,'')", []).expect("one queued root");
    }

    #[test]
    fn rr_02_migrate_existing_partial_state() {
        let c = Connection::open_in_memory().expect("connection");
        c.execute_batch("PRAGMA foreign_keys=ON; CREATE TABLE conversations(id TEXT PRIMARY KEY); CREATE TABLE runtime_runs(id TEXT PRIMARY KEY); CREATE TABLE conversation_messages(id TEXT PRIMARY KEY); INSERT INTO conversations VALUES('c');").expect("base");
        migrate(&c).expect("first");
        c.execute(
            "INSERT INTO rr_policy_versions VALUES('p',1,'{}','d',1)",
            [],
        )
        .expect("policy");
        c.execute("INSERT INTO rr_roots(root_id,conversation_id,policy_id,revision,phase,origin,presentation_mode,started_at_ms,scope_digest) VALUES('r','c','p',1,'responding','text','visual',1,'')", []).expect("root");
        c.execute("INSERT INTO rr_steps(id,root_id,revision,ordinal,actor_id,purpose,status,config_fingerprint,adapter_state_json) VALUES('s','r',1,0,'actor','respond','running','{}','{}')", []).expect("step");
        c.execute(
            "INSERT INTO rr_outputs VALUES('o','s',1,'draft','{}',0,2)",
            [],
        )
        .expect("output");
        // Re-running the migration on a database that already has in-progress work is a no-op:
        // it must neither drop nor silently rewrite the partial root/step/output.
        migrate(&c).expect("second");
        let root: (i64, String) = c
            .query_row(
                "SELECT revision,phase FROM rr_roots WHERE root_id='r'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("root preserved");
        assert_eq!(root, (1, "responding".into()));
        assert_eq!(
            c.query_row(
                "SELECT count(*) FROM rr_steps WHERE status='running'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .expect("running steps"),
            1
        );
        assert_eq!(
            c.query_row(
                "SELECT count(*) FROM rr_outputs WHERE accepted=0",
                [],
                |r| r.get::<_, i64>(0)
            )
            .expect("unaccepted outputs"),
            1
        );
    }

    #[test]
    fn rr_02_tool_link_cannot_reference_a_step_from_another_root() {
        let c = Connection::open_in_memory().expect("connection");
        c.execute_batch("PRAGMA foreign_keys=ON; CREATE TABLE conversations(id TEXT PRIMARY KEY); CREATE TABLE runtime_runs(id TEXT PRIMARY KEY); CREATE TABLE conversation_messages(id TEXT PRIMARY KEY); INSERT INTO conversations VALUES('c');").expect("base");
        migrate(&c).expect("migration");
        c.execute(
            "INSERT INTO rr_policy_versions VALUES('p',1,'{}','d',1)",
            [],
        )
        .expect("policy");
        c.execute("INSERT INTO rr_roots(root_id,conversation_id,policy_id,phase,origin,presentation_mode,started_at_ms,scope_digest) VALUES('r1','c','p','responding','text','visual',1,'')", []).expect("active root");
        c.execute("INSERT INTO rr_roots(root_id,conversation_id,policy_id,phase,origin,presentation_mode,started_at_ms,scope_digest) VALUES('r2','c','p','queued','text','visual',2,'')", []).expect("queued root");
        c.execute("INSERT INTO rr_steps(id,root_id,revision,ordinal,actor_id,purpose,status,config_fingerprint,adapter_state_json) VALUES('step-r1','r1',0,0,'actor','respond','running','{}','{}')", []).expect("step");
        assert!(c.execute("INSERT INTO rr_tool_links(id,root_id,step_id,revision,operation_key,dispatch_state,created_at_ms,updated_at_ms) VALUES('bad','r2','step-r1',0,'operation','reserved',1,1)", []).is_err());
    }
}
