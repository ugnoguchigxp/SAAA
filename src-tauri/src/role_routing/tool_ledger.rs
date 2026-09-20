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
    find_by_operation(connection, &link.root_id, &link.operation_key)?
        .ok_or_else(|| "Role-routing tool reservation was not persisted".into())
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
    let changed = connection
        .execute(
            "UPDATE rr_tool_links SET invocation_id=COALESCE(?1,invocation_id),result_ref=COALESCE(?2,result_ref),dispatch_state=?3,updated_at_ms=?4 WHERE root_id=?5 AND operation_key=?6 AND dispatch_state IN ('reserved','dispatched')",
            params![invocation_id, result_ref, state, now_ms, root_id, operation_key],
        )
        .map_err(|error| error.to_string())?;
    if changed == 0 {
        return find_by_operation(connection, root_id, operation_key)?
            .ok_or_else(|| "Role-routing tool reservation is unavailable".into());
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
}
