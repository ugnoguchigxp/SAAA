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
    let direct_provider_exists = providers["providers"]
        .as_array()
        .and_then(|items| {
            items.iter().find(|provider| {
                provider["id"] == crate::QWEN_DIRECT_PROVIDER_ID
                    && provider["kind"] == "openai-compatible"
                    && provider["enabled"] == true
                    && provider["model"]
                        .as_str()
                        .is_some_and(|model| !model.is_empty())
            })
        })
        .is_some();
    if !direct_provider_exists {
        return Ok(());
    }
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
    // Provider actors resolve their model from the selected provider document. A model value on
    // the actor is reserved for codex_sdk actors and would invalidate the Role Routing policy.
    reasoner["model"] = Value::Null;
    reasoner["label"] = Value::String("Qwen3.8 27B (direct)".into());
    connection.execute(
        "UPDATE settings_documents
         SET value_json=?1, updated_at=?2
         WHERE namespace='routing.roles' AND key='default'",
        params![roles.to_string(), crate::now_iso()],
    )?;
    Ok(())
}

/// The shipped 60 second step timeout was sized for one provider generation. Tool-capable
/// responses perform a generation, a tool call, and a follow-up generation inside the same step,
/// so installations that still carry that shipped value are upgraded without touching custom
/// timeout choices.
pub(crate) fn migrate_v34_to_v35_tool_step_timeout(
    connection: &Connection,
    previous_version: i64,
) -> rusqlite::Result<()> {
    if previous_version >= 35 {
        return Ok(());
    }
    connection.execute(
        "UPDATE settings_documents
         SET value_json=json_set(value_json, '$.limits.stepTimeoutMs', 120000), updated_at=?1
         WHERE namespace='routing.roles' AND key='default'
           AND json_valid(value_json)
           AND json_extract(value_json, '$.limits.stepTimeoutMs')=60000",
        [crate::now_iso()],
    )?;
    Ok(())
}

/// Move only the previously shipped direct-Qwen Role Routing bindings to the explicit Gemma 4
/// LARM profile. The old frontend actor represented the now-removed LFM backchannel and is removed
/// only when it still has the exact shipped binding. Operator-created actors remain untouched.
pub(crate) fn migrate_v35_to_v36_larm_conversation_profile(
    connection: &Connection,
    previous_version: i64,
) -> rusqlite::Result<()> {
    if previous_version >= 36 {
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
    let mut providers: Value = match serde_json::from_str(&providers_json) {
        Ok(value) => value,
        Err(_) => return Ok(()),
    };
    let dynamic_provider_exists = providers["providers"].as_array().is_some_and(|items| {
        items.iter().any(|provider| {
            provider["id"] == crate::DYNAMIC_LAN_PROVIDER_ID
                && provider["kind"] == "dynamic-lan"
                && provider["enabled"] == true
        })
    });
    if !dynamic_provider_exists {
        return Ok(());
    }
    let mut roles: Value = match serde_json::from_str(&roles_json) {
        Ok(value) => value,
        Err(_) => return Ok(()),
    };
    let shipped_reasoner = roles["roles"]["reasoner"] == "local-reasoner"
        && roles["actors"].as_array().is_some_and(|actors| {
            actors.iter().any(|actor| {
                actor["id"] == "local-reasoner"
                    && actor["transport"] == "provider"
                    && actor["providerId"] == crate::QWEN_DIRECT_PROVIDER_ID
                    && actor["model"].is_null()
            })
        });
    if !shipped_reasoner {
        return Ok(());
    }
    let shipped_frontend_role = roles["roles"]["frontend"] == "local-conversation-frontend";
    if let Some(actors) = roles["actors"].as_array_mut() {
        if let Some(reasoner) = actors
            .iter_mut()
            .find(|actor| actor["id"] == "local-reasoner")
        {
            reasoner["providerId"] = Value::String(crate::DYNAMIC_LAN_PROVIDER_ID.into());
            reasoner["label"] = Value::String("Gemma 4 E4B (LARM)".into());
            reasoner["resourceGroup"] = Value::String("larm-conversation".into());
        }
        let shipped_frontend = shipped_frontend_role
            && actors.iter().any(|actor| {
                actor["id"] == "local-conversation-frontend"
                    && actor["transport"] == "provider"
                    && actor["providerId"] == crate::DYNAMIC_LAN_PROVIDER_ID
                    && actor["resourceGroup"] == "harness-backchannel"
            });
        if shipped_frontend {
            actors.retain(|actor| actor["id"] != "local-conversation-frontend");
            roles["roles"]["frontend"] = Value::Null;
        }
    }
    providers["harness"]["larmProfile"] = Value::String("saaa-conversation-ornith15".into());
    let now = crate::now_iso();
    connection.execute(
        "UPDATE settings_documents SET value_json=?1, updated_at=?2
         WHERE namespace='providers.model' AND key='default'",
        params![providers.to_string(), &now],
    )?;
    connection.execute(
        "UPDATE settings_documents SET value_json=?1, updated_at=?2
         WHERE namespace='routing.roles' AND key='default'",
        params![roles.to_string(), &now],
    )?;
    Ok(())
}

/// Older provider actors sometimes stored a model alongside providerId. The provider document
/// owns the model; retaining the old field makes the entire settings snapshot fail validation.
pub(crate) fn clear_legacy_provider_actor_models(connection: &Connection) -> rusqlite::Result<()> {
    let stored: Option<String> = connection
        .query_row(
            "SELECT value_json FROM settings_documents
             WHERE namespace='routing.roles' AND key='default'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    let Some(stored) = stored else { return Ok(()) };
    let Ok(mut policy) = serde_json::from_str::<Value>(&stored) else {
        return Ok(());
    };
    let Some(actors) = policy.get_mut("actors").and_then(Value::as_array_mut) else {
        return Ok(());
    };
    let mut changed = false;
    for actor in actors {
        if actor["transport"] == "provider" && !actor["model"].is_null() {
            actor["model"] = Value::Null;
            changed = true;
        }
    }
    if changed {
        connection.execute(
            "UPDATE settings_documents SET value_json=?1, updated_at=?2
             WHERE namespace='routing.roles' AND key='default'",
            params![policy.to_string(), crate::now_iso()],
        )?;
    }
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
    fn clears_legacy_model_from_provider_actor_without_changing_codex_actor() {
        let connection = Connection::open_in_memory().expect("connection");
        connection
            .execute_batch(
                "CREATE TABLE settings_documents (
                    namespace TEXT NOT NULL, key TEXT NOT NULL, schema_version INTEGER NOT NULL,
                    value_json TEXT NOT NULL, updated_at TEXT NOT NULL,
                    PRIMARY KEY(namespace,key)
                )",
            )
            .expect("settings table");
        let policy = json!({"actors": [
            {"id": "provider", "transport": "provider", "providerId": "local", "model": "old-model"},
            {"id": "codex", "transport": "codex_sdk", "providerId": null, "model": "gpt-6-sol"}
        ]});
        connection
            .execute(
                "INSERT INTO settings_documents VALUES('routing.roles','default',15,?1,'1')",
                [policy.to_string()],
            )
            .expect("legacy policy");
        clear_legacy_provider_actor_models(&connection).expect("migration");
        clear_legacy_provider_actor_models(&connection).expect("idempotent migration");
        let stored: String = connection
            .query_row(
                "SELECT value_json FROM settings_documents WHERE namespace='routing.roles'",
                [],
                |row| row.get(0),
            )
            .expect("stored policy");
        let stored: Value = serde_json::from_str(&stored).expect("policy json");
        assert!(stored["actors"][0]["model"].is_null());
        assert_eq!(stored["actors"][1]["model"], "gpt-6-sol");
    }

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
        let mut policy = crate::role_routing::RoleRoutingSettings::default();
        policy.enabled = true;
        policy.roles.reasoner = Some("local-reasoner".into());
        policy.roles.frontend = Some("local-conversation-frontend".into());
        policy.actors = vec![
            crate::role_routing::contracts::RoutingActor {
                id: "local-reasoner".into(),
                label: "LAN reasoning provider".into(),
                aliases: vec![],
                transport: "provider".into(),
                provider_id: Some(crate::DYNAMIC_LAN_PROVIDER_ID.into()),
                model: None,
                location: "local".into(),
                resource_group: "local-inference".into(),
                max_input_bytes: 65_536,
                larm_provider: None,
                capabilities: vec!["reason".into(), "tools".into()],
            },
            crate::role_routing::contracts::RoutingActor {
                id: "local-conversation-frontend".into(),
                label: "LFM".into(),
                aliases: vec![],
                transport: "provider".into(),
                provider_id: Some(crate::DYNAMIC_LAN_PROVIDER_ID.into()),
                model: None,
                location: "local".into(),
                resource_group: "harness-backchannel".into(),
                max_input_bytes: 16_000,
                larm_provider: None,
                capabilities: vec!["social_reply".into(), "classify".into()],
            },
        ];
        policy.recipes = vec![crate::role_routing::contracts::RoutingRecipe {
            id: "reasoner-response".into(),
            action: crate::role_routing::contracts::RoutingAction::Respond,
            roles: vec!["reasoner".into()],
            enabled: true,
        }];
        let roles = serde_json::to_value(policy).expect("role policy json");
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
        assert!(stored["actors"][0]["model"].is_null());
        assert_eq!(
            stored["actors"][1]["providerId"],
            crate::DYNAMIC_LAN_PROVIDER_ID
        );
        let policy: crate::role_routing::RoleRoutingSettings =
            serde_json::from_value(stored).expect("role policy");
        crate::role_routing::contracts::validate_settings(&policy).expect("valid migrated policy");
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
    fn schema_35_extends_only_the_shipped_tool_step_timeout() {
        let c = Connection::open_in_memory().expect("connection");
        c.execute_batch(
            "CREATE TABLE settings_documents (
               namespace TEXT NOT NULL, key TEXT NOT NULL, schema_version INTEGER NOT NULL,
               value_json TEXT NOT NULL, updated_at TEXT NOT NULL,
               PRIMARY KEY(namespace,key)
             );
             INSERT INTO settings_documents VALUES(
               'routing.roles','default',15,
               '{\"limits\":{\"rootTimeoutMs\":180000,\"stepTimeoutMs\":60000}}','before');",
        )
        .expect("settings");

        migrate_v34_to_v35_tool_step_timeout(&c, 34).expect("migration");

        let step_timeout: i64 = c
            .query_row(
                "SELECT json_extract(value_json, '$.limits.stepTimeoutMs')
                 FROM settings_documents WHERE namespace='routing.roles' AND key='default'",
                [],
                |row| row.get(0),
            )
            .expect("step timeout");
        assert_eq!(step_timeout, 120_000);

        c.execute(
            "UPDATE settings_documents
             SET value_json=json_set(value_json, '$.limits.stepTimeoutMs', 90000)",
            [],
        )
        .expect("custom timeout");
        migrate_v34_to_v35_tool_step_timeout(&c, 34).expect("repeat migration");
        let custom_timeout: i64 = c
            .query_row(
                "SELECT json_extract(value_json, '$.limits.stepTimeoutMs')
                 FROM settings_documents WHERE namespace='routing.roles' AND key='default'",
                [],
                |row| row.get(0),
            )
            .expect("custom step timeout");
        assert_eq!(custom_timeout, 90_000);
    }

    #[test]
    fn schema_36_moves_only_shipped_roles_to_gemma_larm() {
        let c = Connection::open_in_memory().expect("connection");
        c.execute_batch(
            "CREATE TABLE settings_documents (
               namespace TEXT NOT NULL, key TEXT NOT NULL, schema_version INTEGER NOT NULL,
               value_json TEXT NOT NULL, updated_at TEXT NOT NULL,
               PRIMARY KEY(namespace,key)
             );",
        )
        .expect("settings");
        let providers = json!({
            "harness":{"address":"http://192.168.0.130:9810"},
            "providers":[{"id":crate::DYNAMIC_LAN_PROVIDER_ID,"kind":"dynamic-lan","enabled":true}]
        });
        let mut policy = crate::role_routing::RoleRoutingSettings::default();
        policy.enabled = true;
        policy.roles.reasoner = Some("local-reasoner".into());
        policy.roles.frontend = Some("local-conversation-frontend".into());
        policy.actors = vec![
            crate::role_routing::contracts::RoutingActor {
                id: "local-reasoner".into(),
                label: "Qwen3.8 27B (direct)".into(),
                aliases: vec![],
                transport: "provider".into(),
                provider_id: Some(crate::QWEN_DIRECT_PROVIDER_ID.into()),
                model: None,
                location: "local".into(),
                resource_group: "local-inference".into(),
                max_input_bytes: 65_536,
                larm_provider: None,
                capabilities: vec!["reason".into(), "tools".into()],
            },
            crate::role_routing::contracts::RoutingActor {
                id: "local-conversation-frontend".into(),
                label: "Harness conversation frontend".into(),
                aliases: vec!["LFM".into()],
                transport: "provider".into(),
                provider_id: Some(crate::DYNAMIC_LAN_PROVIDER_ID.into()),
                model: None,
                location: "local".into(),
                resource_group: "harness-backchannel".into(),
                max_input_bytes: 16_000,
                larm_provider: None,
                capabilities: vec!["social_reply".into(), "classify".into()],
            },
        ];
        c.execute(
            "INSERT INTO settings_documents VALUES('providers.model','default',15,?1,'before')",
            [providers.to_string()],
        )
        .expect("providers");
        c.execute(
            "INSERT INTO settings_documents VALUES('routing.roles','default',15,?1,'before')",
            [serde_json::to_string(&policy).unwrap()],
        )
        .expect("roles");

        migrate_v35_to_v36_larm_conversation_profile(&c, 35).expect("migration");

        let providers: String = c
            .query_row(
                "SELECT value_json FROM settings_documents WHERE namespace='providers.model'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let providers: Value = serde_json::from_str(&providers).unwrap();
        assert_eq!(providers["harness"]["address"], "http://192.168.0.130:9810");
        assert_eq!(
            providers["harness"]["larmProfile"],
            "saaa-conversation-ornith15"
        );
        let roles: String = c
            .query_row(
                "SELECT value_json FROM settings_documents WHERE namespace='routing.roles'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let roles: Value = serde_json::from_str(&roles).unwrap();
        assert_eq!(roles["roles"]["frontend"], Value::Null);
        assert_eq!(roles["actors"].as_array().unwrap().len(), 1);
        assert_eq!(
            roles["actors"][0]["providerId"],
            crate::DYNAMIC_LAN_PROVIDER_ID
        );
    }

    #[test]
    fn schema_36_preserves_custom_reasoner_policy() {
        let c = Connection::open_in_memory().expect("connection");
        c.execute_batch(
            "CREATE TABLE settings_documents (
               namespace TEXT NOT NULL, key TEXT NOT NULL, schema_version INTEGER NOT NULL,
               value_json TEXT NOT NULL, updated_at TEXT NOT NULL,
               PRIMARY KEY(namespace,key)
             );",
        )
        .expect("settings");
        let providers = json!({"harness":{"address":"https://custom.example"},"providers":[
            {"id":crate::DYNAMIC_LAN_PROVIDER_ID,"kind":"dynamic-lan","enabled":true}
        ]});
        let roles = json!({"roles":{"reasoner":"custom"},"actors":[
            {"id":"custom","transport":"provider","providerId":"custom-provider"}
        ]});
        c.execute(
            "INSERT INTO settings_documents VALUES('providers.model','default',15,?1,'before')",
            [providers.to_string()],
        )
        .unwrap();
        c.execute(
            "INSERT INTO settings_documents VALUES('routing.roles','default',15,?1,'before')",
            [roles.to_string()],
        )
        .unwrap();

        migrate_v35_to_v36_larm_conversation_profile(&c, 35).expect("migration");

        let stored: String = c
            .query_row(
                "SELECT value_json FROM settings_documents WHERE namespace='providers.model'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(serde_json::from_str::<Value>(&stored).unwrap(), providers);
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
