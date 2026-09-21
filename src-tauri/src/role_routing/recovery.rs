//! Startup recovery for durable routing roots.
//!
//! A restarted process never replays an actor or a tool implicitly. Running roots are retained as
//! interrupted evidence; queued roots remain ordered receipts for an explicit scheduler decision.
use super::{coordinator, reducer};
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};

/// A queued root whose durable transition to `responding` has committed. The caller may dispatch
/// its actor only after receiving this value; startup recovery deliberately never calls this.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ClaimedRoot {
    pub(crate) root_id: String,
    pub(crate) revision: u32,
}

pub(crate) fn queued_root_ids(connection: &Connection, limit: u8) -> Result<Vec<String>, String> {
    if limit == 0 {
        return Ok(Vec::new());
    }
    let mut statement = connection
        .prepare(
            "SELECT root_id FROM rr_roots WHERE phase='queued' ORDER BY started_at_ms,root_id LIMIT ?1",
        )
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map([i64::from(limit)], |row| row.get(0))
        .map_err(|error| error.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())
}

/// Claims the oldest queued root whose conversation has no active routing root.
///
/// `IMMEDIATE` makes the selection and `queued -> responding` transition one SQLite writer
/// critical section. A process crash after this returns is reconciled as interrupted at startup;
/// a process crash before commit leaves the receipt queued. No actor or tool I/O happens here.
pub(crate) fn claim_next_queued(
    connection: &mut Connection,
    now_ms: i64,
) -> Result<Option<ClaimedRoot>, String> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| error.to_string())?;
    let root_id: Option<String> = transaction
        .query_row(
            "SELECT queued.root_id
             FROM rr_roots queued
             WHERE queued.phase='queued'
               AND NOT EXISTS (
                   SELECT 1 FROM rr_roots active
                   WHERE active.conversation_id=queued.conversation_id
                     AND active.phase IN ('responding','draining')
               )
             ORDER BY queued.started_at_ms,queued.root_id
             LIMIT 1",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    let Some(root_id) = root_id else {
        transaction.commit().map_err(|error| error.to_string())?;
        return Ok(None);
    };
    let transition =
        coordinator::apply_in_transaction(&transaction, &root_id, reducer::Event::Start, now_ms)?;
    if !matches!(
        transition.effects.as_slice(),
        [reducer::Effect::DispatchActor { .. }]
    ) {
        return Err("Queued role-routing root could not be claimed".into());
    }
    let claimed = ClaimedRoot {
        root_id,
        revision: transition.state.revision,
    };
    transaction.commit().map_err(|error| error.to_string())?;
    Ok(Some(claimed))
}

/// Marks only in-flight work interrupted. The caller must explicitly decide whether a queued
/// receipt starts later; no provider, sidecar, tool, or speech effect is produced here.
pub(crate) fn reconcile_startup(connection: &mut Connection, now_ms: i64) -> Result<usize, String> {
    let transaction = connection
        .transaction()
        .map_err(|error| error.to_string())?;
    let count = reconcile_startup_in_transaction(&transaction, now_ms)?;
    transaction.commit().map_err(|error| error.to_string())?;
    Ok(count)
}

pub(crate) fn reconcile_startup_in_transaction(
    transaction: &Transaction<'_>,
    now_ms: i64,
) -> Result<usize, String> {
    let mut statement = transaction
        .prepare("SELECT root_id FROM rr_roots WHERE phase IN ('responding','draining') ORDER BY started_at_ms,root_id")
        .map_err(|error| error.to_string())?;
    let root_ids = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    drop(statement);
    for root_id in &root_ids {
        transaction
            .execute(
                "UPDATE rr_roots SET phase='failed',active_slot=NULL,drain_reason='app-restarted' WHERE root_id=?1",
                [root_id],
            )
            .map_err(|error| error.to_string())?;
        transaction
            .execute(
                "UPDATE rr_steps SET status='interrupted',completed_at_ms=?1,error_code='app-restarted' WHERE root_id=?2 AND status IN ('planned','running','draining')",
                params![now_ms, root_id],
            )
            .map_err(|error| error.to_string())?;
        let sequence: i64 = transaction
            .query_row(
                "SELECT COALESCE(MAX(seq),0)+1 FROM rr_events WHERE root_id=?1",
                [root_id],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        transaction
            .execute(
                "INSERT INTO rr_events(root_id,seq,kind,data_json,created_at_ms) VALUES(?1,?2,'root_interrupted','{}',?3)",
                params![root_id, sequence, now_ms],
            )
            .map_err(|error| error.to_string())?;
    }
    Ok(root_ids.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Connection {
        let connection = Connection::open_in_memory().expect("database");
        connection.execute_batch("PRAGMA foreign_keys=ON;CREATE TABLE conversations(id TEXT PRIMARY KEY);CREATE TABLE runtime_runs(id TEXT PRIMARY KEY);CREATE TABLE conversation_messages(id TEXT PRIMARY KEY);INSERT INTO conversations VALUES('c');").expect("base");
        crate::role_routing::schema::migrate(&connection).expect("schema");
        connection
            .execute(
                "INSERT INTO rr_policy_versions VALUES('p',1,'{}','d',1)",
                [],
            )
            .expect("policy");
        for (id, phase, started) in [
            ("queued-b", "queued", 2),
            ("running", "responding", 3),
            ("queued-a", "queued", 1),
        ] {
            connection.execute("INSERT INTO rr_roots(root_id,conversation_id,policy_id,phase,origin,presentation_mode,started_at_ms,scope_digest) VALUES(?1,'c','p',?2,'text','visual',?3,'')",params![id,phase,started]).expect("root");
        }
        connection
            .execute(
                "INSERT INTO rr_events VALUES('running',1,'root_started','{}',1)",
                [],
            )
            .expect("event");
        connection
    }

    #[test]
    fn rr_18_queue_order_and_restart_are_safe() {
        let mut connection = fixture();
        assert_eq!(
            queued_root_ids(&connection, 8).expect("queue"),
            vec!["queued-a", "queued-b"]
        );
        assert_eq!(
            reconcile_startup(&mut connection, 10).expect("reconcile"),
            1
        );
        let state: (String, String) = connection
            .query_row(
                "SELECT phase,drain_reason FROM rr_roots WHERE root_id='running'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("root state");
        assert_eq!(state, ("failed".into(), "app-restarted".into()));
        assert_eq!(
            queued_root_ids(&connection, 8).expect("queue after restart"),
            vec!["queued-a", "queued-b"]
        );
    }

    #[test]
    fn rr_18_claims_fifo_only_after_the_prior_root_is_terminal() {
        let mut connection = fixture();
        connection
            .execute(
                "UPDATE rr_roots SET phase='failed',active_slot=NULL WHERE root_id='running'",
                [],
            )
            .expect("finish existing root");
        let first = claim_next_queued(&mut connection, 4)
            .expect("claim")
            .expect("first queued root");
        assert_eq!(first.root_id, "queued-a");
        assert_eq!(
            claim_next_queued(&mut connection, 5).expect("claim while active"),
            None
        );
        connection
            .execute(
                "UPDATE rr_roots SET phase='completed',active_slot=NULL WHERE root_id=?1",
                [&first.root_id],
            )
            .expect("finish first");
        let second = claim_next_queued(&mut connection, 6)
            .expect("claim second")
            .expect("second queued root");
        assert_eq!(second.root_id, "queued-b");
    }

    #[test]
    fn rr_18_claim_skips_a_conversation_with_active_work() {
        let mut connection = fixture();
        connection
            .execute("INSERT INTO conversations VALUES('other')", [])
            .expect("other conversation");
        connection
            .execute(
                "INSERT INTO rr_roots(root_id,conversation_id,policy_id,phase,origin,presentation_mode,started_at_ms,scope_digest) VALUES('other-queued','other','p','queued','text','visual',0,'')",
                [],
            )
            .expect("queued other root");
        let claimed = claim_next_queued(&mut connection, 4)
            .expect("claim")
            .expect("available conversation root");
        assert_eq!(claimed.root_id, "other-queued");
    }
}
