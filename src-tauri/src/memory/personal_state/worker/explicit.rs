//! User-requested foreground registration uses the same Local extraction and writer.
use super::*;
use crate::StartTurnInput;
use rusqlite::{params, Connection, OptionalExtension};
static ACTIVE: Mutex<Option<Arc<RunCancellation>>> = Mutex::new(None);
pub(super) fn interrupt() {
    if let Ok(active) = ACTIVE.lock() {
        if let Some(cancel) = active.as_ref() {
            cancel.cancel();
        }
    }
}
struct Active;
impl Drop for Active {
    fn drop(&mut self) {
        if let Ok(mut active) = ACTIVE.lock() {
            *active = None;
        }
    }
}
pub const TOOL: &str = "register_world_knowledge";

/// Only an unquoted directive clause in the current user turn grants this operation.
/// Restrict matching to whole clauses, rather than instructions inside retrieved text.
pub fn requested(text: &str) -> bool {
    let part = text
        .trim()
        .split(['\n', '。', ':', '：'])
        .next()
        .unwrap_or("");
    matches!(
        part.trim()
            .trim_end_matches(['.', '!', '！'])
            .to_lowercase()
            .as_str(),
        "覚えて"
            | "覚えてください"
            | "これを覚えて"
            | "これを覚えてください"
            | "次を覚えて"
            | "次を覚えてください"
            | "worldに登録して"
            | "worldに登録してください"
            | "この情報をworldに登録して"
            | "remember this"
            | "register this in world"
    )
}

pub fn definition() -> Value {
    json!({"type":"function","function":{"name":TOOL,"description":"When the current user explicitly says これを覚えて or Worldに登録して, immediately extract World knowledge from that saved user turn using LocalLLM. Takes no source IDs or generated facts. Only report registration after status=registered. not_confirmed, already_processed and no_world_change do not confirm a new registration. Existing Memory OFF is respected.","parameters":{"type":"object","properties":{},"additionalProperties":false}}})
}
fn source_id(c: &Connection, input: &StartTurnInput) -> Result<String, String> {
    if !requested(&input.content) {
        return Err("world-explicit-request-required".into());
    }
    let id: Option<String> = c.query_row("SELECT input_message_id FROM runtime_runs WHERE id=?1 AND conversation_id=?2 AND status='running'", params![input.run_id,input.conversation_id], |r|r.get(0)).optional().map_err(database_error)?.flatten();
    let id = match id {
        Some(id) => id,
        None => {
            let key = input
                .run_id
                .strip_prefix("run_")
                .ok_or("world-current-turn-unavailable")?;
            let exists:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM task_queue_jobs WHERE job_key=?1 AND scope=?2 AND lane='conversation' AND state='running')",params![key,input.conversation_id],|r|r.get(0)).map_err(database_error)?;
            if !exists {
                return Err("world-current-turn-unavailable".into());
            }
            format!("check_{key}")
        }
    };
    let matches:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM conversation_messages m JOIN personal_sources p ON p.message_id=m.id WHERE m.id=?1 AND m.conversation_id=?2 AND m.role IN ('user','transcript') AND m.content=?3 AND p.available=1 AND NOT EXISTS(SELECT 1 FROM personal_tombstones t WHERE t.source_id=m.id))",params![id,input.conversation_id,input.content],|r|r.get(0)).map_err(database_error)?;
    if !matches {
        return Err("world-current-turn-unavailable".into());
    }
    Ok(id)
}
fn claim(c: &Connection, input: &StartTurnInput) -> Result<Option<jobs::Job>, String> {
    let id = source_id(c, input)?;
    let busy:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM personal_jobs WHERE status='running') OR EXISTS(SELECT 1 FROM personal_generations WHERE cancellation='sent-unconfirmed')",[],|r|r.get(0)).map_err(database_error)?;
    if busy {
        return Err("world-registration-busy".into());
    }
    jobs::refill(c)?;
    let job:Option<jobs::Job>=c.query_row("SELECT j.id,j.source_sequence,j.lease_generation+1,s.input_epoch,j.offset_bytes,j.finalizing,j.scope_key,e.epoch,j.stage FROM personal_jobs j JOIN personal_sources p ON p.sequence=j.source_sequence CROSS JOIN personal_scope s LEFT JOIN context_scope_epochs e ON e.scope_key=j.scope_key WHERE p.message_id=?1 AND p.available=1 AND j.status='queued' AND s.recovery_ready=1",[&id],|r|Ok(jobs::Job{id:r.get(0)?,sequence:r.get(1)?,lease:r.get(2)?,epoch:r.get(3)?,offset:r.get(4)?,finalizing:r.get(5)?,scope_key:r.get(6)?,scope_epoch:r.get(7)?,stage:r.get(8)?})).optional().map_err(database_error)?;
    let Some(mut j) = job else {
        let done:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM personal_jobs j JOIN personal_sources p ON p.sequence=j.source_sequence WHERE p.message_id=?1 AND p.available=1 AND j.status='completed')",[id],|r|r.get(0)).map_err(database_error)?;
        return if done {
            Ok(None)
        } else {
            Err("world-registration-held".into())
        };
    };
    if j.scope_key.is_none() {
        let mut query=c.prepare("SELECT r.scope_key FROM personal_source_scope_refs r JOIN context_scopes s ON s.scope_key=r.scope_key JOIN personal_sources p ON p.message_id=r.source_id AND p.version=r.version WHERE r.source_id=?1 AND p.available=1 AND s.state='active' LIMIT 2").map_err(database_error)?;
        let scopes = query
            .query_map([&id], |r| r.get::<_, String>(0))
            .map_err(database_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)?;
        if scopes.len() != 1 {
            return Err("world-scope-unresolved".into());
        }
        j.scope_key = scopes.into_iter().next();
    }
    let source = sources::load(c, j.sequence, 0, 32000)?;
    if !source.source.finalized {
        return Err("world-registration-held".into());
    }
    let knowledge =
        crate::memory::personal_state::worker_scope::knowledge_request(c, &j, &source.source.key)?
            .ok_or("world-scope-unresolved")?;
    let eligible:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM personal_source_scope_refs r JOIN context_scopes s ON s.scope_key=r.scope_key WHERE r.source_id=?1 AND r.version=?2 AND r.scope_key=?3 AND s.state='active')",params![source.source.key.id,source.source.key.version,knowledge],|r|r.get(0)).map_err(database_error)?;
    if !eligible {
        return Err("world-scope-unresolved".into());
    }
    j.scope_epoch = match &j.scope_key {
        Some(scope) => Some(
            c.query_row(
                "SELECT epoch FROM context_scope_epochs WHERE scope_key=?1",
                [scope],
                |r| r.get(0),
            )
            .map_err(database_error)?,
        ),
        None => None,
    };
    let changed=c.execute("UPDATE personal_jobs SET status='running',lease_generation=?2,lease_until=?3,epoch=?4,scope_key=?5,claim_scope_epoch=?6 WHERE id=?1 AND status='queued'",params![j.id,j.lease,crate::memory::personal_state::now()+60000,j.epoch,j.scope_key,j.scope_epoch]).map_err(database_error)?;
    if changed != 1 {
        return Err("personal-job-fence".into());
    }
    Ok(Some(j))
}
async fn extract_now(
    writer: &SqliteWriter,
    extractor: &dyn Extractor,
    input: &StartTurnInput,
    cancel: Arc<RunCancellation>,
) -> Result<Value, String> {
    let job = writer.transact(|c| claim(c, input))?;
    let Some(job) = job else {
        return Ok(json!({"status":"already_processed"}));
    };
    let (before, observations_before, source_id)=writer.read_serialized(|c| {
        let ids=store::load(c)?.assertions.keys().cloned().collect::<std::collections::BTreeSet<_>>();
        let count:u64=c.query_row("SELECT count(*) FROM personal_world_observations o JOIN personal_sources p ON p.message_id=o.source_id AND p.version=o.source_version WHERE p.sequence=?1",[job.sequence],|r|r.get(0)).map_err(database_error)?;
        let source:String=c.query_row("SELECT message_id FROM personal_sources WHERE sequence=?1",[job.sequence],|r|r.get(0)).map_err(database_error)?;
        Ok((ids,count,source))
    })?;
    if let Err(error) = run(writer, extractor, &job, cancel.clone()).await {
        writer.transact(|c| {
            c.execute("UPDATE personal_generations SET output_allowed=0,status='interrupted',cancellation='sent-unconfirmed' WHERE purpose IN ('personal_state_extract','world-extraction') AND status IN ('prepared','running')",[]).map_err(database_error)?;
            c.execute("UPDATE personal_registrations SET pins=0,desired='deleted' WHERE incarnation IN (SELECT incarnation FROM personal_cleanup WHERE stage!='complete')",[]).map_err(database_error)?;
            jobs::failed(c,&job,crate::memory::personal_state::now(),cancel.is_cancelled(),error_code(&error,cancel.is_cancelled()))
        })?;
        return Err(error);
    }
    // A continuity change alone is not proof that World knowledge was registered.
    let count = writer.read_serialized(|c| {
        let ledger = crate::memory::personal_state::store::load(c)?;
        Ok(ledger
            .assertions
            .values()
            .filter(|a| {
                a.kind.is_world()
                    && a.input_dependencies.iter().any(|key| key.id == source_id)
                    && !before.contains(&a.id)
                    && matches!(
                        ledger.status(&a.id, crate::memory::personal_state::now()),
                        Status::Active | Status::Disputed
                    )
            })
            .count())
    })?;
    let observations=writer.read_serialized(|c|c.query_row("SELECT count(*) FROM personal_world_observations o JOIN personal_sources p ON p.message_id=o.source_id AND p.version=o.source_version WHERE p.sequence=?1",[job.sequence],|r|r.get::<_,u64>(0)).map_err(database_error))?.saturating_sub(observations_before);
    Ok(
        json!({"status":if count>0 || observations>0 {"registered"} else {"no_world_change"},"worldAssertions":count,"worldObservations":observations}),
    )
}
#[path = "explicit_execute.rs"]
mod execution;
pub use execution::execute;
#[cfg(test)]
use execution::within_budget;

#[cfg(test)]
#[path = "explicit_tests.rs"]
mod tests;
