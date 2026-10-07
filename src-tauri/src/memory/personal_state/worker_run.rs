//! Durable continuity-first extraction; each stage commits with its checkpoint.
use super::*;
pub(super) async fn run(
    writer: &SqliteWriter,
    extractor: &dyn Extractor,
    job: &jobs::Job,
    cancel: Arc<RunCancellation>,
) -> Result<(), String> {
    if job.stage == "world" {
        return world_stage(writer, extractor, job, cancel).await;
    }
    let consolidating = job.stage == "consolidation";
    let (chunk, ledger) = writer.write(|c| {
        let tx = c.transaction().map_err(database_error)?;
        let source = sources::load(
            &tx,
            job.sequence,
            job.offset,
            if job.finalizing { 262144 } else { 32000 },
        )?;
        store::remember_source(&tx, &source.source)?;
        let ledger = store::load(&tx)?;
        tx.commit().map_err(database_error)?;
        Ok((source, ledger))
    })?;
    if job.finalizing && !chunk.source.finalized {
        return Err("personal-finalization-budget".into());
    }
    let request_scope =
        crate::memory::personal_state::worker_scope::request(job, &chunk.source.key.id);
    let (current, mut dependencies) = writer.read_serialized(|c| {
        crate::memory::personal_state::worker::admission::current(
            c,
            &ledger,
            request_scope.as_deref(),
            &chunk.text,
        )
    })?;
    let related = writer.read_serialized(|c| {
        crate::memory::personal_state::worker::admission::dialogue(
            c,
            &chunk.source,
            job.scope_key.as_deref(),
        )
    })?;
    let mut context_sources: Vec<SourceRef> = dependencies
        .iter()
        .filter_map(|key| ledger.sources.get(key).cloned())
        .collect();
    for (source, _) in &related {
        dependencies.insert(source.key.clone());
        context_sources.push(source.clone());
    }
    let dialogue:Vec<_>=related.iter().map(|(source,text)|json!({"source":source.key,"role":source.role,"text":text,"recorded_at":source.recorded_at,"instructionAuthority":"none"})).collect();
    let exposed: BTreeSet<String> = current
        .iter()
        .filter_map(|a| a["id"].as_str().map(str::to_owned))
        .collect();
    if consolidating
        && crate::memory::personal_state::worker::admission::independent_origins(&current).len() < 2
    {
        writer.transact(|c| jobs::advance_world(c, job, crate::memory::personal_state::now()))?;
        return world_stage(writer, extractor, job, cancel).await;
    }
    dependencies.insert(chunk.source.key.clone());
    let input = json!({"purpose":"personal_state_extract","now":crate::memory::personal_state::now(),"instruction":if consolidating { crate::memory::personal_state::worker::admission::CONSOLIDATION_INSTRUCTION } else { EXTRACTION_INSTRUCTION },"request_scope":request_scope,"current":current,"dialogue":dialogue,"context_sources":context_sources,"source":{"ref":chunk.source,"text":chunk.text}});
    if crate::memory::personal_state::encode(&input)?.len() > 48000 {
        return Err("personal-extraction-budget".into());
    }
    let raw = if extractor.owns_cancellation() {
        extractor.extract(input, cancel.clone()).await?
    } else {
        tokio::select! {biased; _=cancel.cancelled()=>return Err("personal-foreground-abort".into()),result=tokio::time::timeout(std::time::Duration::from_secs(30),extractor.extract(input,cancel.clone()))=>result.map_err(|_|"personal-extraction-timeout")??}
    };
    if raw.len() > 16384 {
        return Err("personal-extraction-output-budget".into());
    }
    let extraction: Extraction = crate::memory::personal_state::decode(raw)?;
    if extraction
        .candidates
        .iter()
        .any(|c| c.kind.is_world() || consolidating && c.kind != Kind::Observation)
    {
        // World payloads are never produced by the continuity extractor.
        return Err("personal-extraction-invalid".into());
    }
    if extraction.candidates.len() > 10 || extraction.no_change != extraction.candidates.is_empty()
    {
        return Err("personal-extraction-invalid".into());
    }
    writer.write(|c| {
        for source in &context_sources {
            store::remember_source(c, source)?;
        }
        let now = crate::memory::personal_state::now();
        if !jobs::valid(c, job, now)? {
            return Err("personal-job-fence".into());
        }
        c.execute(
            "UPDATE personal_jobs SET lease_until=?2 WHERE id=?1 AND lease_generation=?3",
            rusqlite::params![job.id, now + 60000, job.lease],
        )
        .map_err(database_error)?;
        Ok(())
    })?;
    let patch_id = crate::new_id("patch");
    let fence = format!("job-{}-{}", job.id, job.lease);
    let now = crate::memory::personal_state::now();
    let patch::Built {
        mut patch,
        payloads,
        payload_bytes,
        dependencies,
        request_scope,
    } = writer.read_serialized(|c| {
        patch::build(patch::Input {
            c,
            chunk: &chunk,
            ledger: &ledger,
            job,
            extractor,
            current: &current,
            dependencies,
            extraction,
            request_scope,
            patch_id: patch_id.clone(),
            fence: fence.clone(),
            now,
            consolidating,
            exposed: &exposed,
        })
    })?;
    let mut context = CommitContext {
        access: AccessRequest {
            principal: &ledger.principal,
            scope: "primary",
            task_request: request_scope.as_deref(),
            purpose: Purpose::StateExtract,
            max_classification: Classification::Confidential,
            policy_revision: ledger.policy_revision,
            authorized: true,
        },
        enabled: true,
        now,
        live_fence: &fence,
        issued_patch_id: &patch_id,
        issued_assertion_ids: patch.assertions.iter().map(|a| a.id.clone()).collect(),
        issued_payload_bytes: payload_bytes,
        evidence_allowlist: dependencies.clone(),
        input_dependencies: dependencies,
    };
    cancel.with_active(|| {
        writer.write(|c| {
            let tx = c.transaction().map_err(database_error)?;
            if !jobs::valid(&tx, job, now)? {
                return Err("personal-job-fence".into());
            }
            if job.finalizing {
                sources::finalize(&tx, &chunk.source)?;
            }
            crate::memory::personal_state::worker_scope::rebase(
                &tx,
                job,
                &mut patch,
                &mut context,
            )?;
            let enabled:bool=tx.query_row("SELECT enabled FROM personal_consolidation_settings WHERE id=1",[],|r|r.get(0)).map_err(database_error)?;
            if !consolidating || enabled {
                store::commit(&tx, &patch, &context, &payloads)?;
                if let Some(scope)=job.scope_key.as_deref() {
                    let revision:u64=tx.query_row("SELECT COALESCE((SELECT revision FROM memory_episode_scope_policies WHERE scope_key=?1),1)",[scope],|r|r.get(0)).map_err(database_error)?;
                    for a in &patch.assertions { tx.execute("INSERT INTO personal_assertion_scope_policies VALUES(?1,?2,?3)",rusqlite::params![a.id,scope,revision]).map_err(database_error)?; }
                }
            }
            if chunk.source.finalized {
                let enabled:bool=tx.query_row("SELECT enabled FROM personal_consolidation_settings WHERE id=1",[],|r|r.get(0)).map_err(database_error)?;
                if enabled && !consolidating {
                    tx.execute("UPDATE personal_jobs SET stage='consolidation',status='queued',offset_bytes=0,lease_until=NULL,next_attempt_at=?3 WHERE id=?1 AND lease_generation=?2",rusqlite::params![job.id,job.lease,crate::memory::personal_state::now()]).map_err(database_error)?;
                } else { jobs::advance_world(&tx, job, crate::memory::personal_state::now())?; }
            } else {
                jobs::finish(&tx, job, chunk.source.key.end, chunk.total_bytes, false)?;
            }
            tx.commit().map_err(database_error)?;
            Ok(())
        })
    })?;
    if chunk.source.finalized {
        let stage = writer.read_serialized(|c| {
            c.query_row(
                "SELECT stage FROM personal_jobs WHERE id=?1",
                [job.id],
                |r| r.get::<_, String>(0),
            )
            .map_err(database_error)
        })?;
        if stage == "consolidation" {
            return Ok(());
        }
        world_stage(writer, extractor, job, cancel).await?;
    }
    Ok(())
}

#[path = "worker_run/world_stage.rs"]
mod world_stage;
use world_stage::run as world_stage;

#[path = "worker_run/patch.rs"]
mod patch;
