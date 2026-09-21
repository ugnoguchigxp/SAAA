//! Links role-routing steps to the existing tool-selection invocation owner.
//!
//! This table never invents tool success.  `settled` only means the owner returned a terminal
//! receipt; callers must use its invocation ledger for the actual outcome.
use rusqlite::{params, Connection, OptionalExtension};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ToolLink {
    pub(crate) id: String,
    pub(crate) root_id: String,
    pub(crate) step_id: String,
    pub(crate) revision: u32,
    pub(crate) operation_key: String,
    pub(crate) invocation_id: Option<String>,
    pub(crate) dispatch_state: String,
    pub(crate) result_ref: Option<String>,
}

/// Reserves one operation before it reaches the existing tool owner. Repeating the same root
/// operation is a no-op and returns the original link, so an unknown remote outcome is never
/// silently retried.
pub(crate) fn reserve(
    connection: &Connection,
    link: &ToolLink,
    now_ms: i64,
) -> Result<ToolLink, String> {
    connection
        .execute(
            "INSERT OR IGNORE INTO rr_tool_links(id,root_id,step_id,revision,operation_key,invocation_id,dispatch_state,result_ref,created_at_ms,updated_at_ms) VALUES(?1,?2,?3,?4,?5,NULL,'reserved',NULL,?6,?6)",
            params![link.id, link.root_id, link.step_id, link.revision, link.operation_key, now_ms],
        )
        .map_err(|error| error.to_string())?;
    let stored = find_by_operation(connection, &link.root_id, &link.operation_key)?
        .ok_or_else(|| "Role-routing tool reservation was not persisted".to_string())?;
    // The operation key is idempotent only inside the execution envelope that first reserved it.
    // Reusing it from another step or revision must not inherit the original authorization.
    if stored.step_id != link.step_id || stored.revision != link.revision {
        return Err("Role-routing tool operation belongs to a different execution envelope".into());
    }
    Ok(stored)
}

pub(crate) fn settle(
    connection: &Connection,
    root_id: &str,
    operation_key: &str,
    invocation_id: Option<&str>,
    result_ref: Option<&str>,
    state: &str,
    now_ms: i64,
) -> Result<ToolLink, String> {
    if !matches!(state, "dispatched" | "settled" | "unknown") {
        return Err("Invalid role-routing tool dispatch state".into());
    }
    let existing = find_by_operation(connection, root_id, operation_key)?
        .ok_or_else(|| "Role-routing tool reservation is unavailable".to_string())?;
    // A settled link is idempotent only for the same terminal request. Everything else is a
    // conflict that the caller must see: a silent no-op would hide a lost or duplicated mutation.
    if existing.dispatch_state == "settled" {
        let same_invocation =
            invocation_id.is_none_or(|value| existing.invocation_id.as_deref() == Some(value));
        let same_result =
            result_ref.is_none_or(|value| existing.result_ref.as_deref() == Some(value));
        return if state == "settled" && same_invocation && same_result {
            Ok(existing)
        } else {
            Err("Role-routing tool link conflicts with its settled receipt".into())
        };
    }
    let allowed = match existing.dispatch_state.as_str() {
        "reserved" => matches!(state, "dispatched" | "settled" | "unknown"),
        "dispatched" => matches!(state, "settled" | "unknown"),
        // An unknown outcome may be corrected only by a concrete terminal result. It must never be
        // turned back into `dispatched`, which would invite a retry.
        "unknown" => state == "settled" && result_ref.is_some(),
        _ => false,
    };
    if !allowed {
        return Err(format!(
            "Role-routing tool link cannot transition {} -> {state}",
            existing.dispatch_state
        ));
    }
    let changed = connection
        .execute(
            "UPDATE rr_tool_links SET invocation_id=COALESCE(?1,invocation_id),result_ref=COALESCE(?2,result_ref),dispatch_state=?3,updated_at_ms=?4 WHERE root_id=?5 AND operation_key=?6 AND dispatch_state=?7",
            params![invocation_id, result_ref, state, now_ms, root_id, operation_key, existing.dispatch_state],
        )
        .map_err(|error| error.to_string())?;
    if changed != 1 {
        return Err("Role-routing tool settlement lost a concurrent update".into());
    }
    find_by_operation(connection, root_id, operation_key)?
        .ok_or_else(|| "Role-routing tool settlement was not persisted".into())
}

pub(crate) fn find_by_operation(
    connection: &Connection,
    root_id: &str,
    operation_key: &str,
) -> Result<Option<ToolLink>, String> {
    connection
        .query_row(
            "SELECT id,root_id,step_id,revision,operation_key,invocation_id,dispatch_state,result_ref FROM rr_tool_links WHERE root_id=?1 AND operation_key=?2",
            params![root_id, operation_key],
            |row| Ok(ToolLink { id: row.get(0)?, root_id: row.get(1)?, step_id: row.get(2)?, revision: row.get(3)?, operation_key: row.get(4)?, invocation_id: row.get(5)?, dispatch_state: row.get(6)?, result_ref: row.get(7)? }),
        )
        .optional()
        .map_err(|error| error.to_string())
}

/// A condition update must not restart the root while a mutation's outcome is unknown.  `unknown`
/// is deliberately included: retrying it could duplicate a side effect.
pub(crate) fn has_unsettled_for_revision(
    connection: &Connection,
    root_id: &str,
    revision: u32,
) -> Result<bool, String> {
    connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM rr_tool_links WHERE root_id=?1 AND revision=?2 AND dispatch_state IN ('reserved','dispatched','unknown'))",
            params![root_id, revision],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Connection {
        let connection = Connection::open_in_memory().expect("connection");
        connection.execute_batch("PRAGMA foreign_keys=ON; CREATE TABLE conversations(id TEXT PRIMARY KEY); CREATE TABLE runtime_runs(id TEXT PRIMARY KEY); CREATE TABLE conversation_messages(id TEXT PRIMARY KEY); INSERT INTO conversations VALUES('c');").expect("base");
        crate::role_routing::schema::migrate(&connection).expect("schema");
        connection
            .execute(
                "INSERT INTO rr_policy_versions VALUES('p',1,'{}','d',1)",
                [],
            )
            .expect("policy");
        connection.execute("INSERT INTO rr_roots(root_id,conversation_id,policy_id,phase,origin,presentation_mode,started_at_ms,scope_digest) VALUES('root','c','p','responding','text','visual',1,'')", []).expect("root");
        connection.execute("INSERT INTO rr_steps(id,root_id,revision,ordinal,actor_id,purpose,status,config_fingerprint,adapter_state_json) VALUES('step','root',0,0,'actor','respond','running','{}','{}')", []).expect("step");
        connection
    }

    fn proposed() -> ToolLink {
        ToolLink {
            id: "link".into(),
            root_id: "root".into(),
            step_id: "step".into(),
            revision: 0,
            operation_key: "operation".into(),
            invocation_id: None,
            dispatch_state: "reserved".into(),
            result_ref: None,
        }
    }

    #[test]
    fn rr_11_duplicate_operation_keeps_the_original_reservation() {
        let connection = fixture();
        assert_eq!(
            reserve(&connection, &proposed(), 1).expect("reserve").id,
            "link"
        );
        let mut duplicate = proposed();
        duplicate.id = "other-link".into();
        assert_eq!(
            reserve(&connection, &duplicate, 2)
                .expect("same operation")
                .id,
            "link"
        );
        assert_eq!(
            settle(
                &connection,
                "root",
                "operation",
                Some("invocation"),
                Some("result"),
                "settled",
                3
            )
            .expect("settle")
            .dispatch_state,
            "settled"
        );
    }

    #[test]
    fn rr_11_operation_key_cannot_cross_step_or_revision() {
        let connection = fixture();
        reserve(&connection, &proposed(), 1).expect("reserve");
        connection
            .execute(
                "INSERT INTO rr_steps(id,root_id,revision,ordinal,actor_id,purpose,status,config_fingerprint,adapter_state_json) VALUES('other-step','root',1,1,'actor','respond','planned','{}','{}')",
                [],
            )
            .expect("other step");
        let mut conflicting = proposed();
        conflicting.id = "other-link".into();
        conflicting.step_id = "other-step".into();
        conflicting.revision = 1;
        assert!(reserve(&connection, &conflicting, 2).is_err());
        assert_eq!(
            find_by_operation(&connection, "root", "operation")
                .expect("lookup")
                .expect("original")
                .step_id,
            "step"
        );
    }

    #[test]
    fn rr_11_same_payload_different_roots() {
        let connection = fixture();
        connection
            .execute("INSERT INTO conversations VALUES('c2')", [])
            .expect("conversation");
        connection.execute("INSERT INTO rr_roots(root_id,conversation_id,policy_id,phase,origin,presentation_mode,started_at_ms,scope_digest) VALUES('root2','c2','p','responding','text','visual',1,'')", []).expect("root2");
        connection.execute("INSERT INTO rr_steps(id,root_id,revision,ordinal,actor_id,purpose,status,config_fingerprint,adapter_state_json) VALUES('step2','root2',0,0,'actor','respond','running','{}','{}')", []).expect("step2");
        reserve(&connection, &proposed(), 1).expect("root1 reserve");
        let mut second = proposed();
        second.id = "link2".into();
        second.root_id = "root2".into();
        second.step_id = "step2".into();
        reserve(&connection, &second, 1).expect("root2 reserve");
        assert_eq!(
            connection
                .query_row(
                    "SELECT count(*) FROM rr_tool_links WHERE operation_key='operation'",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .expect("links"),
            2
        );
    }

    #[test]
    fn rr_11_unknown_no_retry() {
        let connection = fixture();
        reserve(&connection, &proposed(), 1).expect("reserve");
        settle(
            &connection,
            "root",
            "operation",
            Some("invocation"),
            None,
            "unknown",
            2,
        )
        .expect("unknown");
        assert_eq!(
            reserve(&connection, &proposed(), 3)
                .expect("same reservation")
                .dispatch_state,
            "unknown"
        );
    }

    #[test]
    fn rr_11_late_owner_settles_unknown() {
        let connection = fixture();
        reserve(&connection, &proposed(), 1).expect("reserve");
        settle(
            &connection,
            "root",
            "operation",
            Some("invocation"),
            None,
            "unknown",
            2,
        )
        .expect("unknown");
        // A detached owner's later concrete result may settle the unknown link.
        let settled = settle(
            &connection,
            "root",
            "operation",
            Some("invocation"),
            Some("result"),
            "settled",
            3,
        )
        .expect("late settle");
        assert_eq!(settled.dispatch_state, "settled");
        // Re-settling to the same terminal state is idempotent; conflicting transitions surface.
        assert!(settle(
            &connection,
            "root",
            "operation",
            Some("invocation"),
            Some("result"),
            "settled",
            4,
        )
        .is_ok());
        assert!(settle(
            &connection,
            "root",
            "operation",
            Some("invocation"),
            Some("result"),
            "dispatched",
            5,
        )
        .is_err());
    }

    #[test]
    fn rr_11_settled_receipt_rejects_conflicting_replay() {
        let connection = fixture();
        let link = proposed();
        reserve(&connection, &link, 1).expect("reserve");
        settle(
            &connection,
            "root",
            "operation",
            Some("invocation-1"),
            Some("result-1"),
            "settled",
            2,
        )
        .expect("settle");
        assert!(settle(
            &connection,
            "root",
            "operation",
            Some("invocation-1"),
            Some("result-1"),
            "settled",
            3,
        )
        .is_ok());
        assert!(settle(
            &connection,
            "root",
            "operation",
            Some("invocation-1"),
            Some("different-result"),
            "settled",
            4,
        )
        .is_err());
    }

    #[test]
    fn rr_11_settle_failure_blocks_continuation() {
        let connection = fixture();
        reserve(&connection, &proposed(), 1).expect("reserve");
        // An unknown outcome cannot be downgraded back to dispatched, so a continuation that tries
        // to re-run the operation gets a hard error instead of a silent success.
        settle(&connection, "root", "operation", None, None, "unknown", 2).expect("unknown");
        assert!(settle(
            &connection,
            "root",
            "operation",
            Some("invocation"),
            None,
            "dispatched",
            3,
        )
        .is_err());
        assert!(has_unsettled_for_revision(&connection, "root", 0).expect("unsettled"));
    }
}
