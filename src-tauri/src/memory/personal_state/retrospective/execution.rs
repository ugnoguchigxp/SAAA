use super::*;
use std::collections::BTreeMap;
pub(super) async fn run(
    writer: &SqliteWriter,
    extractor: &dyn Extractor,
    j: &jobs::ReviewJob,
    cancel: Arc<RunCancellation>,
) -> Result<(), String> {
    let (window, saved, current) = writer.transact(|c| {
        if !jobs::valid_review(c, j, super::super::now())? {
            return Err("personal-job-fence".into());
        }
        let window = window::load(c, j)?;
        for chunk in std::iter::once(&window.primary).chain(window.context.iter()) {
            let stale:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM personal_source_refs WHERE key_json=?1 AND json_extract(metadata,'$.access.policy_revision')!=?2)",params![super::super::encode(&chunk.source.key)?,j.policy],|r|r.get(0)).map_err(database_error)?;
            if stale { return Ok((window,None,json!({"authorizationPending":true}))); }
        }
        let saved: Option<String> = c
            .query_row(
                "SELECT proposal FROM personal_review_work WHERE id=?1",
                [j.id],
                |r| r.get(0),
            )
            .map_err(database_error)?;
        for chunk in std::iter::once(&window.primary).chain(window.context.iter()) {
            store::remember_source(c, &chunk.source)?;
            c.execute(
                "INSERT OR IGNORE INTO personal_review_inputs VALUES(?1,?2,?3)",
                params![j.id, chunk.source.key.id, chunk.source.key.version],
            )
            .map_err(database_error)?;
        }
        let current = window::current(c, &j.scope, &window.primary.text)?;
        for entry in current.as_array().into_iter().flatten() {
            if let Some(id) = entry["id"].as_str() {
                c.execute(
                    "INSERT OR IGNORE INTO personal_review_premises VALUES(?1,?2)",
                    params![j.id, id],
                )
                .map_err(database_error)?;
            }
        }
        Ok((window, saved, current))
    })?;
    if current["authorizationPending"] == true {
        return finish(
            writer,
            j,
            "held",
            "policy-reauthorization-required",
            None,
            window.end,
            &cancel,
        );
    }
    if window.incomplete {
        return finish(
            writer,
            j,
            "held",
            "evidence-window-incomplete",
            None,
            window.end,
            &cancel,
        );
    }
    let already = writer.read_serialized(|c| {
        let mut all=true;
        for chunk in std::iter::once(&window.primary).chain(window.context.iter()) {
            let done:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM personal_world_source_receipts WHERE scope_key=?1 AND source_id=?2 AND version=?3)",params![j.scope,chunk.source.key.id,chunk.source.key.version],|r|r.get(0)).map_err(database_error)?;
            all &= done;
        }
        Ok(all)
    })?;
    if already {
        return finish(
            writer,
            j,
            "completed",
            "already-registered",
            None,
            window.end,
            &cancel,
        );
    }
    let refs: Vec<_> = std::iter::once(&window.primary)
        .chain(window.context.iter())
        .map(|s| s.source.clone())
        .collect();
    let manifest = serde_json::to_string(&(
        &refs,
        extractor.provenance(),
        saaa_personal_state_core::world::runtime_frame::hex_sha256(
            serde_json::to_string(&current)
                .map_err(|_| "world-review-json")?
                .as_bytes(),
        ),
    ))
    .map_err(|_| "world-review-json")?;
    let additional = window
        .context
        .iter()
        .map(|s| {
            (
                (s.source.key.id.clone(), s.source.key.version),
                s.text.clone(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let raw = if let Some(saved) = saved {
        let old: Option<String> = writer.read_serialized(|c| {
            c.query_row(
                "SELECT manifest FROM personal_review_work WHERE id=?1",
                [j.id],
                |r| r.get(0),
            )
            .map_err(database_error)
        })?;
        if old.as_deref() != Some(&manifest) {
            return finish(
                writer,
                j,
                "held",
                "proposal-input-changed",
                None,
                window.end,
                &cancel,
            );
        }
        saved
    } else {
        let input = json!({"purpose":"world-extraction","retrospective":true,"request_scope":j.scope,"current":current.clone(),"source":{"ref":window.primary.source,"text":window.primary.text},"context_sources":window.context.iter().map(|s|json!({"ref":s.source,"text":s.text})).collect::<Vec<_>>(),"context_coverage":"bounded_as_of","instruction":format!("{}\n{}",INSTRUCTION,super::super::world::extraction::INSTRUCTION)});
        if serde_json::to_vec(&input)
            .map_err(|_| "world-review-json")?
            .len()
            > 48000
        {
            return finish(writer, j, "held", "input-budget", None, window.end, &cancel);
        }
        let raw = if extractor.owns_cancellation() {
            extractor.extract_world(input, cancel.clone()).await?
        } else {
            tokio::select! {biased; _=cancel.cancelled()=>return Err("personal-foreground-abort".into()), result=tokio::time::timeout(std::time::Duration::from_secs(30),extractor.extract_world(input,cancel.clone()))=>result.map_err(|_|"world-extraction-timeout")??}
        };
        let Some(raw) = raw else {
            return Err("personal-capability-unavailable".into());
        };
        // Parse before persisting untrusted model output. The proposal is not a second fact ledger.
        Extraction::parse_with_sources(&raw, &window.primary.text, &additional)?;
        writer.transact(|c| {
            if !jobs::valid_review(c, j, super::super::now())? {
                return Err("personal-job-fence".into());
            }
            for s in &refs {
                sources::revalidate(c, s)?;
            }
            c.execute(
                "UPDATE personal_review_work SET proposal=?2,manifest=?3 WHERE id=?1",
                params![j.id, raw, manifest],
            )
            .map_err(database_error)?;
            Ok(())
        })?;
        raw
    };
    let extraction = Extraction::parse_with_sources(&raw, &window.primary.text, &additional)?;
    cancel.with_active(|| writer.transact(|c| {
        let now=super::super::now();
        if !jobs::valid_review(c,j,now)? { return Err("personal-job-fence".into()); }
        for s in &refs { sources::revalidate(c,s)?; }
        window::revalidate_current(c,&current)?;
        // New same-Scope source beyond the snapshot invalidates currentness.
        let newer: bool=c.query_row("SELECT EXISTS(SELECT 1 FROM personal_sources p JOIN personal_source_scope_refs r ON r.source_id=p.message_id AND r.version=p.version WHERE r.scope_key=?1 AND p.available=1 AND p.role IN ('user','transcript') AND p.sequence>?2)",params![j.scope,j.boundary],|r|r.get(0)).map_err(database_error)?;
        let mode: String=c.query_row("SELECT mode FROM personal_review_settings",[],|r|r.get(0)).map_err(database_error)?;
        let (status,code)=if extraction.deferred_reason.is_some() { ("held","evidence-budget") }
        else if extraction.no_change { ("completed","no-change") }
        else if window.historical || newer { ("held","historical-currentness-unverified") }
        else if mode=="preview" { ("preview","selected") }
        else {
            let context=window.context.iter().map(|s|s.source.clone()).collect::<Vec<_>>();
            let mut provenance=extractor.provenance();
            provenance.extractor_version="world.retrospective_review.v1".into();
            provenance.schema_version="world-v2".into();
            provenance.prompt_digest=saaa_personal_state_core::world::runtime_frame::hex_sha256(instruction().as_bytes());
            super::super::world::extraction::commit_with_sources(c,&extraction,&window.primary.source,&context,&j.scope,&format!("review-{}-{}",j.id,j.generation),provenance,now)?;
            ("completed","applied")
        };
        complete(c,j,status,code,window.end,now)?;
        Ok(())
    }))
}
fn finish(
    writer: &SqliteWriter,
    j: &jobs::ReviewJob,
    status: &str,
    code: &str,
    _proposal: Option<&str>,
    end: u64,
    cancel: &Arc<RunCancellation>,
) -> Result<(), String> {
    cancel.with_active(|| {
        writer.transact(|c| {
            let now = super::super::now();
            if !jobs::valid_review(c, j, now)? {
                return Err("personal-job-fence".into());
            }
            complete(c, j, status, code, end, now)
        })
    })
}
fn complete(
    c: &Connection,
    j: &jobs::ReviewJob,
    status: &str,
    code: &str,
    end: u64,
    now: i64,
) -> Result<(), String> {
    c.execute("UPDATE personal_review_work SET status=?2,result=?3,updated_at=?4,lease_until=NULL WHERE id=?1",params![j.id,status,code,now]).map_err(database_error)?;
    if !j.current {
        c.execute("INSERT INTO personal_review_cursor VALUES(?1,?2,?3) ON CONFLICT(scope_key) DO UPDATE SET sequence=CASE WHEN policy_revision=excluded.policy_revision THEN max(sequence,excluded.sequence) ELSE excluded.sequence END,policy_revision=excluded.policy_revision",params![j.scope,end,j.policy]).map_err(database_error)?;
    }
    // Held work retains its sparse reason; large quotation/proposal detail expires separately.
    c.execute("UPDATE personal_review_work SET proposal=NULL,manifest=NULL WHERE status='held' AND updated_at<?1-604800000",[now]).map_err(database_error)?;
    // A minimal current-lane receipt survives detail expiry, including no-change results.
    c.execute("UPDATE personal_review_work SET proposal=NULL,manifest=NULL WHERE status='completed' AND lane='current'",[]).map_err(database_error)?;
    // Detail expiry never deletes cursor coverage or World source receipts.
    c.execute("DELETE FROM personal_review_work WHERE status='completed' AND lane='history' AND (updated_at<?1-604800000 OR id IN (SELECT id FROM personal_review_work WHERE status='completed' AND lane='history' ORDER BY updated_at DESC,id DESC LIMIT -1 OFFSET 1000))",[now]).map_err(database_error)?;
    Ok(())
}
