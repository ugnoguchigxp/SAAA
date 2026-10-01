//! Lease-fenced completion, stage checkpoints and retry decisions.
use super::*;
pub fn finish(
    c: &Connection,
    j: &Job,
    offset: u64,
    total: u64,
    complete: bool,
) -> Result<(), String> {
    if !valid(c, j, super::super::now())? {
        return Err("personal-job-fence".into());
    }
    let finalizing = !complete && offset == total;
    let n=c.execute("UPDATE personal_jobs SET status=?3,offset_bytes=?4,finalizing=?5,stage='continuity',retry_count=0,lease_until=NULL,result_code=?6 WHERE id=?1 AND lease_generation=?2 AND status='running'",params![j.id,j.lease,if complete{"completed"}else{"queued"},if finalizing{0}else{offset},finalizing,if complete{"applied"}else if finalizing{"finalization-required"}else{"partial-candidate"}]).map_err(database_error)?;
    if n != 1 {
        return Err("personal-job-fence".into());
    }
    Ok(())
}
pub fn advance_world(c: &Connection, j: &Job, now: i64) -> Result<(), String> {
    if !valid(c, j, now)? {
        return Err("personal-job-fence".into());
    }
    let changed = c.execute("UPDATE personal_jobs SET stage='world',retry_count=0,lease_until=?3,result_code='continuity-applied' WHERE id=?1 AND lease_generation=?2 AND status='running'", params![j.id,j.lease,now+60000]).map_err(database_error)?;
    if changed != 1 {
        return Err("personal-job-fence".into());
    }
    Ok(())
}
pub fn failed(c: &Connection, j: &Job, now: i64, aborted: bool, code: &str) -> Result<(), String> {
    let (attempts, retries): (u32, u32) = c
        .query_row(
            "SELECT attempts,retry_count FROM personal_jobs WHERE id=?1",
            [j.id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(database_error)?;
    let transient = code == "transient-unavailable";
    let held = matches!(code, "evidence-budget-held" | "scope-unresolved-held");
    let delays = [30000, 120000, 600000];
    let exhausted = !aborted && !transient && attempts >= 3;
    let changed = c.execute("UPDATE personal_jobs SET status=?3,attempts=attempts+?4,abort_count=abort_count+?5,retry_count=?8,next_attempt_at=?6,lease_until=NULL,result_code=?7 WHERE id=?1 AND lease_generation=?2 AND status='running'",params![j.id,j.lease,if held {"blocked"} else if exhausted {"failed"} else {"queued"},u32::from(!aborted && !transient && !held),u32::from(aborted),now+if aborted{30000}else{delays[if transient { retries.min(2) } else { attempts.min(2) } as usize]},code,if transient{retries.saturating_add(1).min(3)}else{0}]).map_err(database_error)?;
    if changed != 1 {
        return Err("personal-job-fence".into());
    }
    Ok(())
}
