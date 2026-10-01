use crate::database_error;
use rusqlite::{params, Connection, OptionalExtension};
#[derive(Debug, Clone)]
pub struct Job {
    pub id: u64,
    pub sequence: u64,
    pub lease: u64,
    pub epoch: u64,
    pub offset: u64,
    pub finalizing: bool,
    pub stage: String,
    pub scope_key: Option<String>,
    pub scope_epoch: Option<u64>,
}

/// Canonical source rows remain the backlog when the bounded pending queue is full.
/// No high-water cursor advances past deferred input; terminal jobs prevent re-enqueue.
mod review;
pub(crate) use review::{
    claim_review, fail_review, recover_reviews, refill_reviews, valid_review, ReviewJob,
};

pub fn refill(c: &Connection) -> Result<(), String> {
    c.execute("INSERT OR IGNORE INTO personal_jobs(source_sequence,epoch,status,scope_key)
      SELECT p.sequence,(SELECT input_epoch FROM personal_scope),'queued',
        (SELECT m.scope_key FROM conversation_message_scopes m JOIN context_scopes s ON s.scope_key=m.scope_key
         WHERE m.message_id=p.message_id AND s.state='active' AND (m.relation='focus' OR s.kind='user')
         ORDER BY CASE WHEN m.relation='focus' THEN 0 ELSE 1 END,m.scope_key LIMIT 1)
      FROM personal_sources p LEFT JOIN personal_jobs j ON j.source_sequence=p.sequence
      WHERE p.available=1 AND j.id IS NULL ORDER BY p.sequence
      LIMIT max(0,1024-(SELECT count(*) FROM personal_jobs WHERE status IN ('queued','running'))-(SELECT count(*) FROM personal_review_work WHERE status IN ('queued','running','preview')))", []).map_err(database_error)?;
    Ok(())
}
pub fn recover_expired(c: &Connection, now: i64) -> Result<(), String> {
    let expired: bool = c
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM personal_jobs WHERE status='running' AND lease_until<=?1)",
            [now],
            |r| r.get(0),
        )
        .map_err(database_error)?;
    recover_reviews(c, now)?;
    if expired {
        c.execute("UPDATE personal_generations SET output_allowed=0,status='interrupted',cancellation='requested' WHERE purpose IN ('personal_state_extract','world-extraction') AND status IN ('prepared','running')", []).map_err(database_error)?;
        c.execute("UPDATE personal_jobs SET status='queued',lease_until=NULL,lease_generation=lease_generation+1,result_code='expired-lease',next_attempt_at=?1 WHERE status='running' AND lease_until<=?1", [now]).map_err(database_error)?;
    }
    Ok(())
}
mod claim;
pub use claim::{claim, valid};
mod lifecycle;
pub use lifecycle::{advance_world, failed, finish};

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(count: usize) -> Connection {
        let c = Connection::open_in_memory().unwrap();
        crate::persistence::schema::initialize_database(&c).unwrap();
        for i in 0..count {
            c.execute("INSERT INTO conversation_messages(id,conversation_id,role,content,created_at) VALUES(?1,?2,'user','evidence','1')", params![format!("source-{i}"), crate::PRIMARY_CONVERSATION_ID]).unwrap();
        }
        c
    }

    #[test]
    fn world_maintenance_schema_reopen_preserves_preference_and_checkpoint() {
        let c = fixture(1);
        assert_eq!(
            c.query_row("SELECT enabled FROM personal_maintenance", [], |r| r
                .get::<_, u32>(0))
                .unwrap(),
            0
        );
        c.execute(
            "UPDATE personal_maintenance SET enabled=1,next_attempt_at=123456",
            [],
        )
        .unwrap();
        c.execute("UPDATE personal_jobs SET stage='world',retry_count=2", [])
            .unwrap();
        crate::persistence::schema::initialize_database(&c).unwrap();
        assert_eq!(
            c.query_row(
                "SELECT enabled,next_attempt_at FROM personal_maintenance",
                [],
                |r| Ok((r.get::<_, u32>(0)?, r.get::<_, i64>(1)?))
            )
            .unwrap(),
            (1, 123456)
        );
        assert_eq!(
            c.query_row("SELECT stage,retry_count FROM personal_jobs", [], |r| Ok((
                r.get::<_, String>(0)?,
                r.get::<_, u32>(1)?
            )))
            .unwrap(),
            ("world".into(), 2)
        );
    }

    #[test]
    fn bounded_queue_refills_deferred_canonical_sources_without_loss() {
        let c = fixture(1027);
        let count = |table: &str| {
            c.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| {
                r.get::<_, usize>(0)
            })
            .unwrap()
        };
        assert_eq!(count("personal_sources"), 1027);
        assert_eq!(count("personal_jobs"), 1024);
        c.execute("UPDATE personal_jobs SET status='completed' WHERE id IN (SELECT id FROM personal_jobs ORDER BY id LIMIT 3)", []).unwrap();
        refill(&c).unwrap();
        refill(&c).unwrap();
        assert_eq!(count("personal_jobs"), 1027);
        assert_eq!(
            c.query_row(
                "SELECT count(*) FROM personal_jobs WHERE status='queued'",
                [],
                |r| r.get::<_, usize>(0)
            )
            .unwrap(),
            1024
        );
    }

    #[test]
    fn newest_priority_preserves_one_oldest_turn_in_four() {
        let c = fixture(5);
        let mut selected = Vec::new();
        for _ in 0..4 {
            let j = claim(&c, 100_000, true).unwrap().unwrap();
            selected.push(j.sequence);
            c.execute(
                "UPDATE personal_jobs SET status='completed',lease_until=NULL WHERE id=?1",
                [j.id],
            )
            .unwrap();
        }
        assert_eq!(selected, vec![5, 4, 3, 1]);
    }

    #[test]
    fn expired_world_stage_is_resumed_and_old_owner_cannot_ack() {
        let c = fixture(1);
        let old = claim(&c, 100_000, true).unwrap().unwrap();
        advance_world(&c, &old, 100_001).unwrap();
        recover_expired(&c, 160_002).unwrap();
        assert!(!valid(&c, &old, 160_002).unwrap());
        let resumed = claim(&c, 160_002, true).unwrap().unwrap();
        assert_eq!(resumed.stage, "world");
        assert!(resumed.lease > old.lease);
        assert_eq!(
            failed(&c, &old, 160_003, false, "transient-unavailable").unwrap_err(),
            "personal-job-fence"
        );
        assert!(valid(&c, &resumed, 160_003).unwrap());
    }

    #[test]
    fn transient_outage_never_exhausts_malformed_attempt_budget() {
        let c = fixture(1);
        let mut now = 100_000;
        for _ in 0..8 {
            let j = claim(&c, now, true).unwrap().unwrap();
            failed(&c, &j, now, false, "transient-unavailable").unwrap();
            now += 600_000;
        }
        let row: (String, u32, u32) = c
            .query_row(
                "SELECT status,attempts,retry_count FROM personal_jobs",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(row, ("queued".into(), 0, 3));
    }

    #[test]
    fn scoped_job_ignores_unrelated_scope_epoch_and_rejects_its_own_change() {
        let connection = Connection::open_in_memory().unwrap();
        crate::persistence::schema::initialize_database(&connection).unwrap();
        connection
            .execute(
                "INSERT INTO conversation_messages(id,conversation_id,role,content,created_at)
                 VALUES('source',?1,'user','content','1')",
                [crate::PRIMARY_CONVERSATION_ID],
            )
            .unwrap();
        for key in ["task:a", "task:b"] {
            connection
                .execute(
                    "INSERT INTO context_scopes(scope_key,kind,opaque_id,state,created_at)
                     VALUES(?1,'task',substr(?1,6),'active','1')",
                    [key],
                )
                .unwrap();
            connection
                .execute(
                    "INSERT INTO context_scope_epochs(scope_key,epoch) VALUES(?1,1)",
                    [key],
                )
                .unwrap();
        }
        connection
            .execute(
                "UPDATE personal_jobs SET scope_key='task:a' WHERE source_sequence=(
                   SELECT sequence FROM personal_sources WHERE message_id='source'
                 )",
                [],
            )
            .unwrap();
        let job = claim(&connection, 100_000, true).unwrap().unwrap();
        assert_eq!(job.scope_key.as_deref(), Some("task:a"));
        connection
            .execute(
                "UPDATE context_scope_epochs SET epoch=epoch+1 WHERE scope_key='task:b'",
                [],
            )
            .unwrap();
        assert!(valid(&connection, &job, 100_001).unwrap());
        connection
            .execute(
                "UPDATE context_scope_epochs SET epoch=epoch+1 WHERE scope_key='task:a'",
                [],
            )
            .unwrap();
        assert!(!valid(&connection, &job, 100_002).unwrap());
    }
}
