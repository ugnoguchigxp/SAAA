//! World stage runs outside DB locks and atomically commits with job acknowledgment.
use super::*;
pub(super) async fn run(
    writer: &SqliteWriter,
    extractor: &dyn Extractor,
    job: &jobs::Job,
    cancel: Arc<RunCancellation>,
) -> Result<(), String> {
    let (chunk, scope) = writer.read_serialized(|c| {
        if !jobs::valid(c, job, crate::memory::personal_state::now())? {
            return Err("personal-job-fence".into());
        }
        let chunk = sources::load(
            c,
            job.sequence,
            job.offset,
            if job.finalizing { 262144 } else { 32000 },
        )?;
        let scope = crate::memory::personal_state::worker_scope::knowledge_request(
            c,
            job,
            &chunk.source.key,
        )?;
        Ok((chunk, scope))
    })?;
    if chunk.source.finalized && chunk.source.role == SourceRole::User && scope.is_none() {
        return Err("world-scope-unresolved".into());
    }
    let extraction = crate::memory::personal_state::world::extraction::extract(
        writer,
        extractor,
        &chunk.source,
        &chunk.text,
        scope.as_deref(),
        cancel.clone(),
    )
    .await?;
    cancel.with_active(|| {
        writer.write(|c| {
            let tx = c.transaction().map_err(database_error)?;
            let now = crate::memory::personal_state::now();
            if !jobs::valid(&tx, job, now)? {
                return Err("personal-job-fence".into());
            }
            if let Some(world) = &extraction {
                let mut provenance = extractor.provenance();
                provenance.extractor_version = "world-extraction-v2".into();
                provenance.schema_version = "world-v2".into();
                provenance.prompt_digest =
                    saaa_personal_state_core::world::runtime_frame::hex_sha256(
                        crate::memory::personal_state::world::extraction::INSTRUCTION.as_bytes(),
                    );
                crate::memory::personal_state::world::extraction::commit_with_sources(
                    &tx,
                    &world.extraction,
                    &chunk.source,
                    &world.context_sources,
                    scope.as_deref().ok_or("world-extraction-scope")?,
                    &format!("job-{}-{}", job.id, job.lease),
                    provenance,
                    now,
                )?;
            }
            jobs::finish(&tx, job, chunk.source.key.end, chunk.total_bytes, true)?;
            tx.commit().map_err(database_error)
        })
    })
}
