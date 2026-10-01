use super::*;
/// Called only with the existing scheduler's exclusive background slot held.
pub(super) async fn tick(
    writer: &SqliteWriter,
    extractor: &dyn Extractor,
    enabled: bool,
    background: &Mutex<Option<Arc<RunCancellation>>>,
    honor_foreground: bool,
) -> Result<bool, String> {
    let job = writer.transact(|c| jobs::claim_review(c, super::super::now(), enabled))?;
    let Some(job) = job else {
        return Ok(false);
    };
    let cancel = Arc::new(RunCancellation::default());
    *background
        .lock()
        .map_err(|_| "personal-scheduler-unavailable")? = Some(cancel.clone());
    if honor_foreground && super::super::scheduler::foreground_requested() {
        cancel.cancel();
    }
    let result = super::super::retrospective::run(writer, extractor, &job, cancel.clone()).await;
    *background
        .lock()
        .map_err(|_| "personal-scheduler-unavailable")? = None;
    if let Err(error) = result {
        writer.transact(|c| {
            if error=="world-review-premise" { c.execute("UPDATE personal_review_work SET proposal=NULL,manifest=NULL WHERE id=?1 AND generation=?2 AND status='running'",rusqlite::params![job.id,job.generation]).map_err(database_error)?; }
            c.execute("UPDATE personal_generations SET output_allowed=0,status='interrupted',cancellation='sent-unconfirmed' WHERE purpose='world-extraction' AND status IN ('prepared','running')", []).map_err(database_error)?;
            jobs::fail_review(c,&job,super::super::now(),if error=="world-review-premise" { "premise-changed" } else { error_code(&error,cancel.is_cancelled()) })
        })?;
        return Err(error);
    }
    Ok(true)
}
