//! Transactional boundary for restarting a root after a condition amendment.
use rusqlite::{Connection, Transaction};

/// Moves a draining root to its next revision only after every old-revision tool has a known
/// terminal receipt. The coordinator transition and this check share one transaction.
pub(crate) fn resume_after_tools_settle(
    connection: &mut Connection,
    root_id: &str,
    now_ms: i64,
) -> Result<super::reducer::Transition, String> {
    let transaction = connection
        .transaction()
        .map_err(|error| error.to_string())?;
    let transition = resume_in_transaction(&transaction, root_id, now_ms)?;
    transaction.commit().map_err(|error| error.to_string())?;
    Ok(transition)
}

fn resume_in_transaction(
    transaction: &Transaction<'_>,
    root_id: &str,
    now_ms: i64,
) -> Result<super::reducer::Transition, String> {
    let revision: u32 = transaction
        .query_row(
            "SELECT revision FROM rr_roots WHERE root_id=?1",
            [root_id],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if super::tool_ledger::has_unsettled_for_revision(transaction, root_id, revision)? {
        return Err("Role-routing revision waits for a tool receipt".into());
    }
    super::coordinator::apply_in_transaction(
        transaction,
        root_id,
        super::reducer::Event::Resume,
        now_ms,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Connection {
        let connection = Connection::open_in_memory().expect("database");
        connection.execute_batch("PRAGMA foreign_keys=ON; CREATE TABLE conversations(id TEXT PRIMARY KEY); CREATE TABLE runtime_runs(id TEXT PRIMARY KEY); CREATE TABLE conversation_messages(id TEXT PRIMARY KEY); INSERT INTO conversations VALUES('c');").expect("base");
        crate::role_routing::schema::migrate(&connection).expect("schema");
        connection
            .execute(
                "INSERT INTO rr_policy_versions VALUES('p',1,'{}','d',1)",
                [],
            )
            .expect("policy");
        connection.execute_batch("INSERT INTO rr_roots(root_id,conversation_id,policy_id,phase,origin,presentation_mode,started_at_ms,scope_digest) VALUES('r','c','p','draining','text','visual',1,''); INSERT INTO rr_steps(id,root_id,revision,ordinal,actor_id,purpose,status,config_fingerprint,adapter_state_json) VALUES('s','r',0,0,'actor','respond','draining','{}','{}'); INSERT INTO rr_events VALUES('r',1,'input_barrier','{}',1);").expect("root");
        connection
    }

    #[test]
    fn rr_17_tool_unknown_blocks_restart() {
        let mut connection = fixture();
        connection.execute("INSERT INTO rr_tool_links(id,root_id,step_id,revision,operation_key,dispatch_state,created_at_ms,updated_at_ms) VALUES('l','r','s',0,'op','unknown',1,1)", []).expect("link");
        assert_eq!(
            resume_after_tools_settle(&mut connection, "r", 2),
            Err("Role-routing revision waits for a tool receipt".into())
        );
        let phase: String = connection
            .query_row("SELECT phase FROM rr_roots WHERE root_id='r'", [], |row| {
                row.get(0)
            })
            .expect("phase");
        assert_eq!(phase, "draining");
    }

    #[test]
    fn rr_17_settled_tool_allows_next_revision() {
        let mut connection = fixture();
        connection.execute("INSERT INTO rr_tool_links(id,root_id,step_id,revision,operation_key,dispatch_state,created_at_ms,updated_at_ms) VALUES('l','r','s',0,'op','settled',1,1)", []).expect("link");
        let transition = resume_after_tools_settle(&mut connection, "r", 2).expect("resume");
        assert_eq!(transition.state.revision, 1);
        assert_eq!(
            transition.state.phase,
            super::super::reducer::Phase::Responding
        );
    }
}
