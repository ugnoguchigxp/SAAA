use super::*;
/// Maps the active routing step purpose to its role and checks the trusted tool effect. Steps that
/// do not belong to a routing root (or a conversation without a running step) keep the legacy
/// unrestricted meaning because this gateway only routes role-root calls.
pub(super) fn authorize_routing_tool(
    writer: &SqliteWriter,
    root_id: &str,
    effect: crate::role_routing::tools::ToolEffect,
    binding: Option<&RoleStepBinding<'_>>,
) -> Result<(), String> {
    let root_id = root_id.to_string();
    let binding = binding.map(|binding| {
        (
            binding.step_id.to_string(),
            binding.revision,
            binding.attempt_started_at_ms,
            binding.config_fingerprint.to_string(),
        )
    });
    writer.read_serialized(move |connection| {
        let purpose: Option<String> = if let Some((step_id, revision, started_at, fingerprint)) = &binding {
            connection
            .query_row(
                "SELECT s.purpose FROM rr_steps s JOIN rr_roots r ON r.root_id=s.root_id
                 WHERE s.root_id=?1 AND s.id=?2 AND s.revision=?3 AND s.started_at_ms=?4
                   AND s.config_fingerprint=?5 AND s.status='running' AND r.phase='responding'
                   AND r.revision=s.revision AND r.cancel_requested=0",
                rusqlite::params![&root_id, step_id, revision, started_at, fingerprint],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| error.to_string())?
        } else {
            connection
                .query_row(
                    "SELECT purpose FROM rr_steps WHERE root_id=?1 AND status IN ('running','draining') ORDER BY ordinal LIMIT 1",
                    [&root_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|error| error.to_string())?
        };
        let Some(purpose) = purpose else {
            return if binding.is_some() {
                Err("routing MCP session binding is stale".into())
            } else {
                Ok(())
            };
        };
        let role = match purpose.as_str() {
            "review" => "reviewer",
            "respond" | "reconsider" | "revise" => "reasoner",
            "tool_specialist" => "tool_specialist",
            "frontend" => "frontend",
            other => return Err(format!("Role-routing step purpose {other} has no tool role")),
        };
        crate::role_routing::tools::permits_effect(role, effect).map_err(str::to_string)
    })
}
pub(super) fn resolve_role_tool_effect(
    service: &ToolSelectionService,
    context: &RequestContext,
    name: &str,
    arguments: &str,
) -> crate::role_routing::tools::ToolEffect {
    match name {
        TOOL_SEARCH | "tools.search" | TOOL_DESCRIBE | "tools.describe" => {
            crate::role_routing::tools::ToolEffect::ReadOnly
        }
        TOOL_INVOKE | "tools.invoke" => serde_json::from_str::<Value>(arguments)
            .ok()
            .and_then(|arguments| {
                arguments
                    .get("executionRef")
                    .and_then(Value::as_str)
                    .and_then(|reference| service.execution_effect(context, reference).ok())
            })
            .map(|effect| crate::role_routing::tools::classify_effect(Some(&effect)))
            .unwrap_or(crate::role_routing::tools::ToolEffect::Mutating),
        _ => crate::role_routing::tools::ToolEffect::Mutating,
    }
}
/// The existing tool-selection service remains the invocation owner.  Role routing only reserves
/// its operation key before dispatch and records the owner's receipt afterwards.  A process that
/// dies after `dispatched` therefore cannot silently replay a potentially mutating operation.
pub(super) fn reserve_routing_operation(
    writer: &SqliteWriter,
    root_id: &str,
    operation_key: &str,
    binding: Option<&RoleStepBinding<'_>>,
) -> Result<bool, String> {
    let root_id = root_id.to_string();
    let operation_key = operation_key.to_string();
    let binding = binding.map(|binding| {
        (
            binding.step_id.to_string(),
            binding.revision,
            binding.attempt_started_at_ms,
            binding.config_fingerprint.to_string(),
        )
    });
    writer.write(move |connection| {
        let transaction = connection.transaction().map_err(|error| error.to_string())?;
        let step: Option<(String, u32)> = if let Some((step_id, revision, started_at, fingerprint)) = &binding {
            transaction.query_row(
                "SELECT s.id,s.revision FROM rr_steps s JOIN rr_roots r ON r.root_id=s.root_id
                 WHERE s.root_id=?1 AND s.id=?2 AND s.revision=?3 AND s.started_at_ms=?4
                   AND s.config_fingerprint=?5 AND s.status='running' AND r.phase='responding'
                   AND r.revision=s.revision AND r.cancel_requested=0",
                rusqlite::params![&root_id, step_id, revision, started_at, fingerprint],
                |row| Ok((row.get(0)?, row.get(1)?)),
            ).optional().map_err(|error| error.to_string())?
        } else {
            transaction
                .query_row(
                    "SELECT id,revision FROM rr_steps WHERE root_id=?1 AND status='running' ORDER BY ordinal LIMIT 1",
                    [&root_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(|error| error.to_string())?
        };
        let step = match step {
            Some(step) => Some(step),
            // Before the coordinator claims a step, fall back to the lowest planned step so the
            // reservation still binds to the step the result will belong to.
            None if binding.is_none() => transaction
                .query_row(
                    "SELECT id,revision FROM rr_steps WHERE root_id=?1 AND status IN ('planned','draining') ORDER BY ordinal LIMIT 1",
                    [&root_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(|error| error.to_string())?,
            None => return Err("Role-routing MCP session binding is stale".into()),
        };
        let Some((step_id, revision)) = step else {
            transaction.commit().map_err(|error| error.to_string())?;
            return Ok(true);
        };
        if crate::role_routing::tool_ledger::find_by_operation(&transaction, &root_id, &operation_key)?.is_some() {
            transaction.commit().map_err(|error| error.to_string())?;
            return Ok(false);
        }
        let policy_json: String = transaction
            .query_row(
                "SELECT p.config_json FROM rr_roots r JOIN rr_policy_versions p ON p.id=r.policy_id WHERE r.root_id=?1",
                [&root_id],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        let policy: crate::role_routing::RoleRoutingSettings = serde_json::from_str(&policy_json)
            .map_err(|error| format!("Role-routing tool budget policy is invalid: {error}"))?;
        let used: i64 = transaction
            .query_row(
                "SELECT count(*) FROM rr_tool_links WHERE root_id=?1",
                [&root_id],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        if used >= i64::from(policy.limits.max_tool_calls) {
            return Err("Role-routing tool budget exceeded".into());
        }
        // The link id must include the root so the same name+arguments used by two different roots
        // cannot collide on the primary key.
        let link_id = format!(
            "rr-tool-{:x}",
            Sha256::digest(format!("{root_id}:{operation_key}").as_bytes())
        );
        let link = crate::role_routing::tool_ledger::ToolLink {
            id: link_id.chars().take(32).collect(),
            root_id: root_id.clone(),
            step_id,
            revision,
            operation_key: operation_key.clone(),
            invocation_id: None,
            dispatch_state: "reserved".into(),
            result_ref: None,
        };
        let now_ms = now_ms();
        crate::role_routing::tool_ledger::reserve(&transaction, &link, now_ms)?;
        crate::role_routing::tool_ledger::settle(
            &transaction,
            &root_id,
            &operation_key,
            None,
            None,
            "dispatched",
            now_ms,
        )?;
        transaction.commit().map_err(|error| error.to_string())?;
        Ok(true)
    })
}
pub(super) fn settle_routing_operation(
    writer: &SqliteWriter,
    root_id: &str,
    operation_key: &str,
    output: &Value,
) -> Result<(), String> {
    let invocation_id = output
        .pointer("/data/invocationId")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let result_ref = output
        .pointer("/data/resultRef")
        .and_then(Value::as_str)
        .map(str::to_owned);
    // A transport/unavailable failure leaves the remote outcome unknown; it must not be recorded
    // as a settled success, because that would license a later retry of a possibly applied
    // mutation. Local validation failures are terminal and safe to settle.
    let error_code = output.pointer("/error/code").and_then(Value::as_str);
    let unknown = matches!(error_code, Some("unavailable" | "timeout" | "transport"));
    let state = if unknown { "unknown" } else { "settled" };
    let root_id = root_id.to_string();
    let operation_key = operation_key.to_string();
    writer.write(move |connection| {
        crate::role_routing::tool_ledger::settle(
            connection,
            &root_id,
            &operation_key,
            invocation_id.as_deref(),
            result_ref.as_deref(),
            state,
            now_ms(),
        )
        .map(|_| ())
    })
}
pub(super) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}
#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn role_writer() -> SqliteWriter {
        let connection = Connection::open_in_memory().expect("database");
        crate::initialize_database(&connection).expect("schema");
        connection.execute("INSERT INTO conversations(id,title,task_mode,created_at,updated_at) VALUES('c',NULL,'conversation','1','1')", []).expect("conversation");
        connection.execute("INSERT INTO runtime_runs(id,conversation_id,route_kind,status,started_at) VALUES('r','c','conversation.respond','running','1')", []).expect("run");
        let policy_id: String = connection
            .query_row(
                "SELECT id FROM rr_policy_versions ORDER BY version DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .expect("policy");
        connection.execute("INSERT INTO rr_roots(root_id,conversation_id,runtime_run_id,policy_id,revision,phase,origin,presentation_mode,started_at_ms,scope_digest) VALUES('r','c','r',?1,0,'responding','text','visual',1,'')", [&policy_id]).expect("root");
        connection.execute("INSERT INTO rr_steps(id,root_id,revision,ordinal,actor_id,purpose,status,config_fingerprint,adapter_state_json,started_at_ms) VALUES('s0','r',0,0,'actor','respond','running','f0','{}',10)", []).expect("step");
        SqliteWriter::from_connection(connection)
    }

    #[test]
    fn the_three_entry_points_are_fixed() {
        let definitions = definitions();
        let names: Vec<&str> = definitions
            .iter()
            .filter_map(|definition| definition.pointer("/function/name").and_then(Value::as_str))
            .collect();
        assert_eq!(names, vec![TOOL_SEARCH, TOOL_DESCRIBE, TOOL_INVOKE]);
        assert_eq!(internal_name(TOOL_SEARCH), Some("tools.search"));
        assert!(is_selection_tool(TOOL_INVOKE));
        assert!(!is_selection_tool("recall"));
    }

    #[test]
    fn search_schema_is_the_fixed_contract() {
        let schema = search_schema();
        assert_eq!(schema.pointer("/properties/limit/maximum"), Some(&json!(8)));
        assert_eq!(schema.pointer("/properties/limit/minimum"), Some(&json!(1)));
        assert_eq!(schema.pointer("/additionalProperties"), Some(&json!(false)));
        assert_eq!(schema.pointer("/required/0"), Some(&json!("intent")));
    }

    #[test]
    fn error_envelope_is_stable() {
        let envelope = error_envelope(&ToolSelectionError::invalid());
        assert_eq!(envelope.pointer("/ok"), Some(&json!(false)));
        assert_eq!(
            envelope.pointer("/error/code"),
            Some(&json!("invalid-input"))
        );
        assert_eq!(envelope.pointer("/error/retryable"), Some(&json!(false)));
    }

    #[test]
    fn rr_21_old_session_cannot_use_new_step() {
        let writer = role_writer();
        let old = RoleStepBinding {
            root_id: "r",
            step_id: "s0",
            revision: 0,
            attempt_started_at_ms: 10,
            config_fingerprint: "f0",
        };
        writer
            .write(|connection| {
                connection
                    .execute("UPDATE rr_steps SET status='succeeded',completed_at_ms=20 WHERE id='s0'", [])
                    .map_err(|error| error.to_string())?;
                connection
                    .execute("UPDATE rr_roots SET revision=1 WHERE root_id='r'", [])
                    .map_err(|error| error.to_string())?;
                connection.execute("INSERT INTO rr_steps(id,root_id,revision,ordinal,actor_id,purpose,status,config_fingerprint,adapter_state_json,started_at_ms) VALUES('s1','r',1,1,'actor','respond','running','f1','{}',20)", []).map_err(|error| error.to_string())?;
                Ok(())
            })
            .expect("advance root");
        assert!(reserve_routing_operation(&writer, "r", "old-operation", Some(&old)).is_err());
        let links = writer
            .read_serialized(|connection| {
                connection
                    .query_row("SELECT count(*) FROM rr_tool_links", [], |row| {
                        row.get::<_, i64>(0)
                    })
                    .map_err(|error| error.to_string())
            })
            .expect("links");
        assert_eq!(links, 0);
    }

    #[test]
    fn rr_21_missing_step_denied() {
        let writer = role_writer();
        let missing = RoleStepBinding {
            root_id: "r",
            step_id: "missing",
            revision: 0,
            attempt_started_at_ms: 10,
            config_fingerprint: "f0",
        };
        assert!(reserve_routing_operation(&writer, "r", "missing-step", Some(&missing)).is_err());
        assert!(authorize_routing_tool(
            &writer,
            "r",
            crate::role_routing::tools::ToolEffect::ReadOnly,
            Some(&missing)
        )
        .is_err());
    }

    #[test]
    fn rr_21_tool_budget() {
        let writer = role_writer();
        writer
            .write(|connection| {
                let policy_id: String = connection
                    .query_row(
                        "SELECT policy_id FROM rr_roots WHERE root_id='r'",
                        [],
                        |row| row.get(0),
                    )
                    .map_err(|error| error.to_string())?;
                let mut policy = crate::role_routing::RoleRoutingSettings::default();
                policy.limits.max_tool_calls = 0;
                connection
                    .execute(
                        "UPDATE rr_policy_versions SET config_json=?1 WHERE id=?2",
                        rusqlite::params![
                            serde_json::to_string(&policy).map_err(|error| error.to_string())?,
                            policy_id
                        ],
                    )
                    .map_err(|error| error.to_string())?;
                Ok(())
            })
            .expect("zero tool budget");
        let binding = RoleStepBinding {
            root_id: "r",
            step_id: "s0",
            revision: 0,
            attempt_started_at_ms: 10,
            config_fingerprint: "f0",
        };
        assert!(
            reserve_routing_operation(&writer, "r", "over-budget", Some(&binding))
                .expect_err("budget must reject")
                .contains("budget exceeded")
        );
        assert_eq!(
            writer
                .read_serialized(|connection| connection
                    .query_row("SELECT count(*) FROM rr_tool_links", [], |row| row
                        .get::<_, i64>(0))
                    .map_err(|error| error.to_string()))
                .expect("links"),
            0
        );
    }

    #[test]
    fn rr_10_update_between_reserve_and_invoke() {
        let writer = role_writer();
        writer
            .write(|connection| {
                connection
                    .execute(
                        "UPDATE rr_roots SET cancel_requested=1,phase='draining' WHERE root_id='r'",
                        [],
                    )
                    .map(|_| ())
                    .map_err(|error| error.to_string())
            })
            .expect("cancel root");
        let binding = RoleStepBinding {
            root_id: "r",
            step_id: "s0",
            revision: 0,
            attempt_started_at_ms: 10,
            config_fingerprint: "f0",
        };
        assert!(reserve_routing_operation(&writer, "r", "cancelled", Some(&binding)).is_err());
    }

    #[test]
    fn rr_29_tool_permit_update_before_invoke() {
        let writer = role_writer();
        let binding = RoleStepBinding {
            root_id: "r",
            step_id: "s0",
            revision: 0,
            attempt_started_at_ms: 10,
            config_fingerprint: "f0",
        };
        authorize_routing_tool(
            &writer,
            "r",
            crate::role_routing::tools::ToolEffect::ReadOnly,
            Some(&binding),
        )
        .expect("initial permit");
        writer
            .write(|connection| {
                connection
                    .execute(
                        "UPDATE rr_roots SET cancel_requested=1,phase='cancelled' WHERE root_id='r'",
                        [],
                    )
                    .map(|_| ())
                    .map_err(|error| error.to_string())
            })
            .expect("condition update commits");
        assert!(
            reserve_routing_operation(&writer, "r", "must-not-reach-owner", Some(&binding))
                .is_err()
        );
        assert_eq!(
            writer
                .read_serialized(|connection| connection
                    .query_row("SELECT count(*) FROM rr_tool_links", [], |row| row
                        .get::<_, i64>(0))
                    .map_err(|error| error.to_string()))
                .expect("links"),
            0,
            "the second gateway check must stop invocation before owner reservation"
        );
    }

    #[test]
    fn rr_10_reviewer_resolved_mutation_denied_at_gateway() {
        let writer = role_writer();
        writer
            .write(|connection| {
                connection
                    .execute("UPDATE rr_steps SET purpose='review' WHERE id='s0'", [])
                    .map(|_| ())
                    .map_err(|error| error.to_string())
            })
            .expect("review step");
        let binding = RoleStepBinding {
            root_id: "r",
            step_id: "s0",
            revision: 0,
            attempt_started_at_ms: 10,
            config_fingerprint: "f0",
        };
        assert!(authorize_routing_tool(
            &writer,
            "r",
            crate::role_routing::tools::ToolEffect::Mutating,
            Some(&binding),
        )
        .is_err());
        assert!(authorize_routing_tool(
            &writer,
            "r",
            crate::role_routing::tools::ToolEffect::ReadOnly,
            Some(&binding),
        )
        .is_ok());
    }

    #[test]
    fn rr_11_duplicate_operation_once_uses_canonical_payload() {
        assert_eq!(
            routing_operation_key("tools_invoke", r#"{"a":1,"b":{"x":2,"y":3}}"#),
            routing_operation_key("tools_invoke", r#"{"b":{"y":3,"x":2},"a":1}"#),
        );
        assert_ne!(
            routing_operation_key("tools_invoke", r#"{"a":1}"#),
            routing_operation_key("tools_invoke", r#"{"a":2}"#),
        );
    }

    #[test]
    fn rr_11_settle_failure_blocks_continuation() {
        let writer = role_writer();
        let error = settle_routing_operation(
            &writer,
            "r",
            "operation-that-was-never-reserved",
            &json!({"ok":true,"data":{"status":"succeeded"}}),
        )
        .expect_err("missing reservation must be visible to caller");
        assert!(error.contains("reservation is unavailable"));
    }
}
