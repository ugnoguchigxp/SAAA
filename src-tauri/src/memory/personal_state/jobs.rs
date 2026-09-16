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
}

pub fn claim(c: &Connection, now: i64, enabled: bool) -> Result<Option<Job>, String> {
    if !enabled {
        return Ok(None);
    }
    let busy:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM runtime_runs WHERE status='running') OR EXISTS(SELECT 1 FROM personal_jobs WHERE status='running') OR EXISTS(SELECT 1 FROM personal_generations WHERE cancellation='sent-unconfirmed') OR ?1-(SELECT last_foreground_at FROM personal_scope)<30000",[now],|r|r.get(0)).map_err(database_error)?;
    if busy {
        return Ok(None);
    }
    c.execute("UPDATE personal_jobs SET status='completed',result_code='empty-no-change' WHERE status='queued' AND source_sequence IN (SELECT sequence FROM personal_sources WHERE available=1 AND bytes=0)", []).map_err(database_error)?;
    let job=c.query_row("SELECT j.id,j.source_sequence,j.lease_generation+1,s.input_epoch,j.offset_bytes,j.finalizing FROM personal_jobs j JOIN personal_sources p ON p.sequence=j.source_sequence CROSS JOIN personal_scope s WHERE j.status='queued' AND j.next_attempt_at<=?1 AND p.available=1 AND s.recovery_ready=1 ORDER BY j.id LIMIT 1",[now],|r|Ok(Job{id:r.get(0)?,sequence:r.get(1)?,lease:r.get(2)?,epoch:r.get(3)?,offset:r.get(4)?,finalizing:r.get(5)?})).optional().map_err(database_error)?;
    if let Some(j) = &job {
        let n=c.execute("UPDATE personal_jobs SET status='running',lease_generation=?2,lease_until=?3,epoch=?4 WHERE id=?1 AND status='queued'",params![j.id,j.lease,now+30000,j.epoch]).map_err(database_error)?;
        if n != 1 {
            return Err("personal-job-claim-conflict".into());
        }
    }
    Ok(job)
}
pub fn valid(c: &Connection, j: &Job, now: i64) -> Result<bool, String> {
    c.query_row("SELECT EXISTS(SELECT 1 FROM personal_jobs j JOIN personal_scope s WHERE j.id=?1 AND j.status='running' AND j.lease_generation=?2 AND j.lease_until>?3 AND j.epoch=s.input_epoch AND s.input_epoch=?4)",params![j.id,j.lease,now,j.epoch],|r|r.get(0)).map_err(database_error)
}
pub fn finish(
    c: &Connection,
    j: &Job,
    offset: u64,
    total: u64,
    complete: bool,
) -> Result<(), String> {
    if !valid(c, j, super::now())? {
        return Err("personal-job-fence".into());
    }
    let finalizing = !complete && offset == total;
    let n=c.execute("UPDATE personal_jobs SET status=?3,offset_bytes=?4,finalizing=?5,lease_until=NULL,result_code=?6 WHERE id=?1 AND lease_generation=?2 AND status='running'",params![j.id,j.lease,if complete{"completed"}else{"queued"},if finalizing{0}else{offset},finalizing,if complete{"applied"}else if finalizing{"finalization-required"}else{"partial-candidate"}]).map_err(database_error)?;
    if n != 1 {
        return Err("personal-job-fence".into());
    }
    Ok(())
}
pub fn failed(c: &Connection, j: &Job, now: i64, aborted: bool, code: &str) -> Result<(), String> {
    let attempts: u32 = c
        .query_row(
            "SELECT attempts FROM personal_jobs WHERE id=?1",
            [j.id],
            |r| r.get(0),
        )
        .map_err(database_error)?;
    let delays = [30000, 120000, 600000];
    let exhausted = !aborted && attempts >= 3;
    c.execute("UPDATE personal_jobs SET status=?3,attempts=attempts+?4,abort_count=abort_count+?5,next_attempt_at=?6,lease_until=NULL,result_code=?7 WHERE id=?1 AND lease_generation=?2 AND status='running'",params![j.id,j.lease,if exhausted{"failed"}else{"queued"},u32::from(!aborted),u32::from(aborted),now+if aborted{30000}else{delays[attempts.min(2) as usize]},code]).map_err(database_error)?;
    Ok(())
}
