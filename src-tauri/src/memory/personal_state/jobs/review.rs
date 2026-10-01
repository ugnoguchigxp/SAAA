//! Review work shares the existing job owner, admission, scheduler and Writer.
use super::*;
#[derive(Debug)]
pub(crate) struct ReviewJob {
    pub id: u64,
    pub scope: String,
    pub sequence: u64,
    pub boundary: u64,
    pub epoch: u64,
    pub policy: u64,
    pub generation: u64,
    pub current: bool,
}
pub(crate) fn refill_reviews(c: &Connection) -> Result<(), String> {
    let mode: String = c
        .query_row("SELECT mode FROM personal_review_settings", [], |r| {
            r.get(0)
        })
        .map_err(database_error)?;
    if mode == "off" {
        return Ok(());
    }
    c.execute("UPDATE personal_review_work SET proposal=NULL,manifest=NULL WHERE status='held' AND updated_at<?1-604800000",[super::super::now()]).map_err(database_error)?;
    c.execute("UPDATE personal_review_work SET status='held',proposal=NULL,manifest=NULL,generation=generation+1,result='policy-changed' WHERE status IN ('queued','running','preview') AND policy_revision!=(SELECT policy_revision FROM personal_scope)",[]).map_err(database_error)?;
    // Keep eight candidate windows available for newly arrived evidence; sparse held reasons remain.
    c.execute("UPDATE personal_review_work SET proposal=NULL,manifest=NULL WHERE id IN (SELECT id FROM personal_review_work WHERE status='held' AND proposal IS NOT NULL ORDER BY updated_at,id LIMIT 8) AND (SELECT count(*) FROM personal_review_work WHERE proposal IS NOT NULL AND status='held')>=120",[]).map_err(database_error)?;
    // Latest evidence has its own receipt; processing it never advances the history cursor.
    c.execute("INSERT OR IGNORE INTO personal_review_work(scope_key,sequence,boundary,scope_epoch,policy_revision,version,lane)
       SELECT r.scope_key,max(p.sequence),max(p.sequence),e.epoch,s.policy_revision,s.policy_revision,'current'
       FROM personal_sources p JOIN personal_source_scope_refs r ON r.source_id=p.message_id AND r.version=p.version
       JOIN context_scopes scope ON scope.scope_key=r.scope_key AND scope.state='active'
       JOIN context_scope_epochs e ON e.scope_key=r.scope_key CROSS JOIN personal_scope s
       WHERE p.available=1 AND p.role IN ('user','transcript') AND p.bytes>0 AND scope.kind IN ('project','user')
       GROUP BY r.scope_key ORDER BY max(p.sequence) DESC
       LIMIT max(0,min(128-(SELECT count(*) FROM personal_review_work WHERE status IN ('queued','running','preview') OR (status='held' AND proposal IS NOT NULL)),1024-(SELECT count(*) FROM personal_jobs WHERE status IN ('queued','running'))-(SELECT count(*) FROM personal_review_work WHERE status IN ('queued','running','preview'))))",[]).map_err(database_error)?;
    // One history window per Scope in flight; canonical rows, not terminal detail, are the cursor.
    c.execute("INSERT OR IGNORE INTO personal_review_work(scope_key,sequence,boundary,scope_epoch,policy_revision,version)
      SELECT r.scope_key,min(p.sequence),max(p.sequence),e.epoch,(SELECT policy_revision FROM personal_scope),(SELECT policy_revision FROM personal_scope)
      FROM personal_sources p JOIN personal_source_scope_refs r ON r.source_id=p.message_id AND r.version=p.version
      JOIN context_scopes s ON s.scope_key=r.scope_key AND s.state='active'
      JOIN context_scope_epochs e ON e.scope_key=s.scope_key
      LEFT JOIN personal_review_cursor cursor ON cursor.scope_key=s.scope_key AND cursor.policy_revision=(SELECT policy_revision FROM personal_scope)
      WHERE p.available=1 AND p.role IN ('user','transcript') AND p.bytes>0
      AND p.sequence>COALESCE(cursor.sequence,0)
      AND NOT EXISTS(SELECT 1 FROM personal_review_work w WHERE w.scope_key=s.scope_key AND w.status IN ('queued','running') AND w.lane='history')
      AND s.kind IN ('project','user') GROUP BY r.scope_key
      ORDER BY min(p.sequence) LIMIT max(0,min(120-(SELECT count(*) FROM personal_review_work WHERE status IN ('queued','running','preview') OR (status='held' AND proposal IS NOT NULL)),1024-(SELECT count(*) FROM personal_jobs WHERE status IN ('queued','running'))-(SELECT count(*) FROM personal_review_work WHERE status IN ('queued','running','preview') OR (status='held' AND proposal IS NOT NULL))))", []).map_err(database_error)?;
    Ok(())
}
pub(crate) fn claim_review(
    c: &Connection,
    now: i64,
    enabled: bool,
) -> Result<Option<ReviewJob>, String> {
    if !enabled {
        return Ok(None);
    }
    let busy: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM runtime_runs WHERE status='running') OR EXISTS(SELECT 1 FROM personal_jobs WHERE status='running') OR EXISTS(SELECT 1 FROM personal_review_work WHERE status='running') OR EXISTS(SELECT 1 FROM personal_generations WHERE cancellation='sent-unconfirmed') OR ?1-(SELECT last_foreground_at FROM personal_scope)<30000 OR (SELECT recovery_ready FROM personal_scope)!=1 OR (SELECT mode FROM personal_review_settings)='off'", [now], |r| r.get(0)).map_err(database_error)?;
    if busy {
        return Ok(None);
    }
    let job = c.query_row("SELECT w.id,w.scope_key,w.sequence,w.boundary,e.epoch,s.policy_revision,w.generation+1,w.lane='current' FROM personal_review_work w JOIN context_scopes scope ON scope.scope_key=w.scope_key AND scope.state='active' JOIN context_scope_epochs e ON e.scope_key=w.scope_key CROSS JOIN personal_scope s WHERE w.status='queued' AND w.next_attempt_at<=?1 ORDER BY CASE WHEN (SELECT turn FROM personal_review_settings)%4<3 THEN w.lane!='current' ELSE w.lane!='history' END,w.id LIMIT 1", [now], |r| Ok(ReviewJob {id:r.get(0)?,scope:r.get(1)?,sequence:r.get(2)?,boundary:r.get(3)?,epoch:r.get(4)?,policy:r.get(5)?,generation:r.get(6)?,current:r.get(7)?})).optional().map_err(database_error)?;
    if let Some(j) = &job {
        c.execute("UPDATE personal_review_work SET status='running',scope_epoch=?2,policy_revision=?3,generation=?4,lease_until=?5 WHERE id=?1", params![j.id,j.epoch,j.policy,j.generation,now+60000]).map_err(database_error)?;
        c.execute("UPDATE personal_review_settings SET turn=turn+1", [])
            .map_err(database_error)?;
        c.execute(
            "UPDATE personal_review_settings SET dispatch_turn=dispatch_turn+1",
            [],
        )
        .map_err(database_error)?;
    }
    Ok(job)
}
pub(crate) fn valid_review(c: &Connection, j: &ReviewJob, now: i64) -> Result<bool, String> {
    c.query_row("SELECT EXISTS(SELECT 1 FROM personal_review_work w JOIN context_scope_epochs e ON e.scope_key=w.scope_key JOIN context_scopes scope ON scope.scope_key=w.scope_key CROSS JOIN personal_scope s WHERE w.id=?1 AND w.status='running' AND w.generation=?2 AND w.lease_until>?3 AND e.epoch=?4 AND s.policy_revision=?5 AND scope.state='active' AND s.recovery_ready=1 AND (SELECT mode FROM personal_review_settings)!='off')", params![j.id,j.generation,now,j.epoch,j.policy], |r|r.get(0)).map_err(database_error)
}
pub(crate) fn recover_reviews(c: &Connection, now: i64) -> Result<(), String> {
    c.execute("UPDATE personal_review_work SET status='queued',generation=generation+1,lease_until=NULL,result='lease-expired' WHERE status='running' AND lease_until<=?1", [now]).map_err(database_error)?;
    Ok(())
}
pub(crate) fn fail_review(
    c: &Connection,
    j: &ReviewJob,
    now: i64,
    code: &str,
) -> Result<(), String> {
    c.execute("UPDATE personal_review_work SET status=CASE WHEN ?4 IN ('extraction-invalid','evidence-budget-held','scope-unresolved-held') THEN 'held' ELSE 'queued' END,lease_until=NULL,retries=min(3,retries+1),next_attempt_at=?3+CASE retries WHEN 0 THEN 30000 WHEN 1 THEN 120000 ELSE 600000 END,result=?4,updated_at=?3 WHERE id=?1 AND generation=?2 AND status='running'", params![j.id,j.generation,now,code]).map_err(database_error)?;
    Ok(())
}
