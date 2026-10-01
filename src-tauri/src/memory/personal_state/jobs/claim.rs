use super::*;
pub fn claim(c: &Connection, now: i64, enabled: bool) -> Result<Option<Job>, String> {
    if !enabled {
        return Ok(None);
    }
    let busy:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM runtime_runs WHERE status='running') OR EXISTS(SELECT 1 FROM personal_jobs WHERE status='running') OR EXISTS(SELECT 1 FROM personal_review_work WHERE status='running') OR EXISTS(SELECT 1 FROM personal_generations WHERE cancellation='sent-unconfirmed') OR ?1-(SELECT last_foreground_at FROM personal_scope)<30000",[now],|r|r.get(0)).map_err(database_error)?;
    if busy {
        return Ok(None);
    }
    c.execute("UPDATE personal_jobs SET status='completed',result_code='empty-no-change' WHERE status='queued' AND source_sequence IN (SELECT sequence FROM personal_sources WHERE available=1 AND bytes=0)", []).map_err(database_error)?;
    let job=c.query_row("SELECT j.id,j.source_sequence,j.lease_generation+1,s.input_epoch,j.offset_bytes,j.finalizing,j.scope_key,e.epoch,j.stage FROM personal_jobs j JOIN personal_sources p ON p.sequence=j.source_sequence CROSS JOIN personal_scope s LEFT JOIN context_scope_epochs e ON e.scope_key=j.scope_key WHERE j.status='queued' AND j.next_attempt_at<=?1 AND p.available=1 AND s.recovery_ready=1 AND (j.scope_key IS NULL OR e.scope_key IS NOT NULL) ORDER BY CASE WHEN s.world_turn%4<3 THEN -j.id ELSE j.id END LIMIT 1",[now],|r|Ok(Job{id:r.get(0)?,sequence:r.get(1)?,lease:r.get(2)?,epoch:r.get(3)?,offset:r.get(4)?,finalizing:r.get(5)?,scope_key:r.get(6)?,scope_epoch:r.get(7)?,stage:r.get(8)?})).optional().map_err(database_error)?;
    if let Some(j) = &job {
        let n=c.execute("UPDATE personal_jobs SET status='running',lease_generation=?2,lease_until=?3,epoch=?4,claim_scope_epoch=?5 WHERE id=?1 AND status='queued'",params![j.id,j.lease,now+60000,j.epoch,j.scope_epoch]).map_err(database_error)?;
        if n != 1 {
            return Err("personal-job-claim-conflict".into());
        }
        c.execute("UPDATE personal_scope SET world_turn=(world_turn+1)%4", [])
            .map_err(database_error)?;
    }
    Ok(job)
}
pub fn valid(c: &Connection, j: &Job, now: i64) -> Result<bool, String> {
    c.query_row("SELECT EXISTS(SELECT 1 FROM personal_jobs stored JOIN personal_scope s LEFT JOIN context_scope_epochs e ON e.scope_key=stored.scope_key WHERE stored.id=?1 AND stored.status='running' AND stored.lease_generation=?2 AND stored.lease_until>?3 AND ((stored.scope_key IS NULL AND stored.epoch=s.input_epoch AND s.input_epoch=?4) OR (stored.scope_key=?5 AND stored.claim_scope_epoch=?6 AND e.epoch=?6)))",params![j.id,j.lease,now,j.epoch,j.scope_key,j.scope_epoch],|r|r.get(0)).map_err(database_error)
}
