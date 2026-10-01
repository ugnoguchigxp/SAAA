//! Admission and bounded diagnostics. Callers use the existing single SqliteWriter;
//! connection discovery and inference run only after this read has released its lock.
use crate::database_error;
use rusqlite::{params, Connection};
use serde_json::{json, Value};

pub fn admission(c: &Connection, now: i64, enabled: bool) -> Result<bool, String> {
    c.query_row(
        "SELECT ?1>=m.next_attempt_at AND (
           EXISTS(SELECT 1 FROM personal_remote_operations WHERE state!='cleaned') OR
           (?2 AND s.recovery_ready=1 AND ?1-s.last_foreground_at>=30000
            AND NOT EXISTS(SELECT 1 FROM runtime_runs WHERE status='running')
            AND NOT EXISTS(SELECT 1 FROM personal_jobs WHERE status='running')
            AND NOT EXISTS(SELECT 1 FROM personal_generations WHERE cancellation='sent-unconfirmed')
            AND EXISTS(SELECT 1 FROM personal_jobs j JOIN personal_sources p ON p.sequence=j.source_sequence
                       WHERE j.status='queued' AND j.next_attempt_at<=?1 AND p.available=1)))
         FROM personal_maintenance m CROSS JOIN personal_scope s WHERE m.id=1",
        params![now, enabled], |r| r.get(0),
    ).map_err(database_error)
}

pub fn record(c: &Connection, now: i64, code: &str) -> Result<(), String> {
    // Fixed codes only: never persist endpoint credentials, model responses or source text.
    let failed = code != "ready";
    let failures: u32 = c
        .query_row(
            "SELECT failures FROM personal_maintenance WHERE id=1",
            [],
            |r| r.get(0),
        )
        .map_err(database_error)?;
    let delay = if failed {
        [30_000, 120_000, 600_000][failures.min(2) as usize]
    } else {
        0
    };
    c.execute("UPDATE personal_maintenance SET reason=?1,failures=?2,next_attempt_at=?3,updated_at=?4 WHERE id=1",
        params![code, if failed { failures.saturating_add(1).min(3) } else { 0 }, now+delay, now]).map_err(database_error)?;
    Ok(())
}

pub fn status(c: &Connection) -> Result<Value, String> {
    let mut result = c.query_row("SELECT reason,failures,next_attempt_at,updated_at FROM personal_maintenance WHERE id=1", [], |r|
        Ok(json!({"reason":r.get::<_,String>(0)?,"failures":r.get::<_,u32>(1)?,"nextAttemptAt":r.get::<_,i64>(2)?,"updatedAt":r.get::<_,i64>(3)?}))).map_err(database_error)?;
    let (queued,running,failed,held,world): (u64,u64,u64,u64,u64) = c.query_row("SELECT COALESCE(sum(status='queued'),0),COALESCE(sum(status='running'),0),COALESCE(sum(status='failed'),0),COALESCE(sum(status='blocked'),0),COALESCE(sum(status IN ('queued','running') AND stage='world'),0) FROM personal_jobs", [], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).map_err(database_error)?;
    let deferred: u64 = c.query_row("SELECT count(*) FROM personal_sources p LEFT JOIN personal_jobs j ON j.source_sequence=p.sequence WHERE p.available=1 AND j.id IS NULL", [], |r| r.get(0)).map_err(database_error)?;
    result["work"] = json!({"queued":queued,"running":running,"failed":failed,"held":held,"worldPending":world,"deferred":deferred});
    result["externalEvidence"] = json!({"state":"blocked_dependency","contract":"world_evidence_v1","reason":"contextstill-evidence-contract-unverified"});
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch("CREATE TABLE conversation_messages(id TEXT, conversation_id TEXT, role TEXT, content TEXT, created_at TEXT);
             CREATE TABLE runtime_runs(status TEXT);
             CREATE TABLE personal_remote_operations(state TEXT);").unwrap();
        c.execute_batch(include_str!("schema.sql")).unwrap();
        c.execute(
            "INSERT INTO personal_scope(id,principal) VALUES('primary','test')",
            [],
        )
        .unwrap();
        c
    }
    #[test]
    fn world_maintenance_backoff_is_bounded_and_recovers() {
        let c = fixture();
        for (now, expected) in [
            (100, 30100),
            (30100, 150100),
            (150100, 750100),
            (750100, 1350100),
        ] {
            record(&c, now, "connection-unavailable").unwrap();
            assert_eq!(status(&c).unwrap()["nextAttemptAt"], expected);
        }
        record(&c, 1350100, "ready").unwrap();
        assert_eq!(status(&c).unwrap()["failures"], 0);
        assert!(!admission(&c, 1350101, true).unwrap()); // Empty queue never connects.
        assert!(!admission(&c, 1350101, false).unwrap());
    }
    #[test]
    fn world_maintenance_admission_respects_foreground_disable_and_cleanup() {
        let c = fixture();
        c.execute("INSERT INTO conversation_messages VALUES('m','conversation_primary','user','test','100000')", []).unwrap();
        assert!(!admission(&c, 129999, true).unwrap());
        assert!(admission(&c, 130000, true).unwrap());
        assert!(!admission(&c, 130000, false).unwrap());
        c.execute("INSERT INTO runtime_runs VALUES('running')", [])
            .unwrap();
        assert!(!admission(&c, 130000, true).unwrap());
        c.execute(
            "INSERT INTO personal_remote_operations VALUES('pending')",
            [],
        )
        .unwrap();
        assert!(admission(&c, 130000, false).unwrap()); // Forget cleanup works while disabled.
        record(&c, 130000, "connection-unavailable").unwrap();
        assert!(!admission(&c, 159999, false).unwrap());
        assert!(admission(&c, 160000, false).unwrap());
        c.execute_batch(include_str!("schema.sql")).unwrap(); // Reinitialization retains backoff.
        assert!(!admission(&c, 159999, false).unwrap());
    }
}
