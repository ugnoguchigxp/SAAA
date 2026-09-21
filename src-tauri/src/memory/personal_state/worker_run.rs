//! One leased extraction job, with atomic continuity and World commit.
use super::*;
pub(super) async fn run(
    writer: &SqliteWriter,
    extractor: &dyn Extractor,
    job: &jobs::Job,
    cancel: Arc<RunCancellation>,
) -> Result<(), String> {
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
    let current=writer.read_serialized(|c|{
        let mut values=Vec::new();
        for a in ledger.assertions.values(){
            if a.kind.is_world(){continue;}
            if (a.access.task_request.is_none() || a.access.task_request.as_deref() == request_scope.as_deref()) && a.access.classification <= Classification::Confidential && a.access.purposes.contains(&Purpose::StateExtract) && matches!(ledger.status(&a.id,crate::memory::personal_state::now()),Status::Active|Status::Candidate|Status::Disputed){
                let payload:String=c.query_row("SELECT value_json FROM personal_payloads WHERE id=?1",[&a.payload_ref],|r|r.get(0)).map_err(database_error)?;
                values.push(json!({"id":a.id,"kind":a.kind,"key":a.semantic_key,"value":crate::memory::personal_state::decode::<Value>(payload)?,"task_request":a.access.task_request}));
            }
        }
        Ok(values)
    })?;
    let input = json!({"purpose":"personal_state_extract","instruction":EXTRACTION_INSTRUCTION,"request_scope":request_scope,"current":current,"source":{"ref":chunk.source,"text":chunk.text}});
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
    if extraction.candidates.iter().any(|c| c.kind.is_world()) {
        // World payloads are never produced by the continuity extractor.
        return Err("personal-extraction-invalid".into());
    }
    if extraction.candidates.len() > 10 || extraction.no_change != extraction.candidates.is_empty()
    {
        return Err("personal-extraction-invalid".into());
    }
    writer.write(|c| {
        let now = crate::memory::personal_state::now();
        if !jobs::valid(c, job, now)? {
            return Err("personal-job-fence".into());
        }
        c.execute(
            "UPDATE personal_jobs SET lease_until=?2 WHERE id=?1 AND lease_generation=?3",
            rusqlite::params![job.id, now + 30000, job.lease],
        )
        .map_err(database_error)?;
        Ok(())
    })?;
    let world_extraction = crate::memory::personal_state::world::extraction::extract(
        writer,
        extractor,
        &chunk.source,
        &chunk.text,
        request_scope.as_deref(),
        cancel.clone(),
    )
    .await?;
    let mut dependencies = BTreeSet::from([chunk.source.key.clone()]);
    for a in ledger.assertions.values() {
        if a.kind.is_world() {
            continue;
        }
        if (a.access.task_request.is_none()
            || a.access.task_request.as_deref() == request_scope.as_deref())
            && a.access.classification <= Classification::Confidential
            && a.access.purposes.contains(&Purpose::StateExtract)
            && matches!(
                ledger.status(&a.id, crate::memory::personal_state::now()),
                Status::Active | Status::Candidate | Status::Disputed
            )
        {
            dependencies.extend(a.input_dependencies.clone());
        }
    }
    let patch_id = crate::new_id("patch");
    let fence = format!("job-{}-{}", job.id, job.lease);
    let now = crate::memory::personal_state::now();
    let mut patch = StatePatch {
        id: patch_id.clone(),
        base_revision: ledger.revision,
        input_epoch: job.epoch,
        policy_revision: ledger.policy_revision,
        fence: fence.clone(),
        assertions: Vec::new(),
        transitions: Vec::new(),
        coverage: Vec::new(),
    };
    let mut payloads = BTreeMap::new();
    let mut next_sequence = ledger.transitions.last().map_or(1, |t| t.sequence + 1);
    for mut candidate in extraction.candidates {
        if !chunk.source.finalized {
            candidate.status = Status::Candidate;
            candidate.replaces = None;
        }
        if !matches!(candidate.status, Status::Active | Status::Candidate)
            || candidate.task_request.as_deref() != request_scope.as_deref()
        {
            return Err("personal-extraction-scope".into());
        }
        let id = crate::new_id("assertion");
        let payload = crate::new_id("payload");
        let mut access = chunk.source.access.clone();
        access.task_request = candidate.task_request;
        let assertion = Assertion {
            id: id.clone(),
            kind: candidate.kind,
            semantic_key: candidate.semantic_key,
            payload_ref: payload.clone(),
            access,
            evidence: BTreeSet::from([chunk.source.key.clone()]),
            depends_on: BTreeSet::new(),
            input_dependencies: dependencies.clone(),
            provenance: extractor.provenance(),
            observed_at: chunk.source.recorded_at,
            effective_at: now,
            recorded_at: now,
            valid_from: now,
            valid_until: None,
        };
        let mut actions = vec![(id.clone(), Action::Assert)];
        if let Some(old) = candidate.replaces {
            actions.push((old, Action::Supersede { by: id.clone() }));
        }
        if candidate.status == Status::Active {
            actions.push((id.clone(), Action::Activate));
        }
        for (target, action) in actions {
            patch.transitions.push(Transition {
                id: crate::new_id("transition"),
                sequence: next_sequence,
                assertion_id: target,
                action,
                reason_code: "extractor-supported".into(),
                evidence: assertion.evidence.clone(),
                input_dependencies: dependencies.clone(),
                recorded_at: now,
            });
            next_sequence += 1;
        }
        payloads.insert(payload, candidate.value);
        patch.assertions.push(assertion);
    }
    if job.finalizing {
        for old in ledger.assertions.values() {
            if ledger.status(&old.id, now) == Status::Candidate
                && old.evidence.iter().any(|k| {
                    k.id == chunk.source.key.id
                        && k.version == chunk.source.key.version
                        && *k != chunk.source.key
                })
            {
                patch.transitions.push(Transition {
                    id: crate::new_id("transition"),
                    sequence: next_sequence,
                    assertion_id: old.id.clone(),
                    action: Action::Invalidate,
                    reason_code: "full-message-finalized".into(),
                    evidence: BTreeSet::from([chunk.source.key.clone()]),
                    input_dependencies: dependencies.clone(),
                    recorded_at: now,
                });
                next_sequence += 1;
            }
        }
    }
    patch.coverage.push((
        chunk.source.key.clone(),
        if !chunk.source.finalized {
            if patch.assertions.is_empty() {
                Coverage::Pending
            } else {
                Coverage::Candidate
            }
        } else if extraction.no_change {
            Coverage::NoChange
        } else if patch
            .transitions
            .iter()
            .any(|t| t.action == Action::Activate)
        {
            Coverage::Applied
        } else {
            Coverage::Candidate
        },
    ));
    let payload_bytes = payloads
        .iter()
        .map(|(k, v)| Ok((k.clone(), crate::memory::personal_state::encode(v)?.len())))
        .collect::<Result<BTreeMap<_, _>, String>>()?;
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
            store::commit(&tx, &patch, &context, &payloads)?;
            if let Some(world) = &world_extraction {
                let mut provenance = extractor.provenance();
                provenance.extractor_version = "world-extraction-v1".into();
                provenance.schema_version = "world-v2".into();
                provenance.prompt_digest =
                    saaa_personal_state_core::world::runtime_frame::hex_sha256(
                        crate::memory::personal_state::world::extraction::INSTRUCTION.as_bytes(),
                    );
                crate::memory::personal_state::world::extraction::commit(
                    &tx,
                    world,
                    &chunk.source,
                    request_scope.as_deref().ok_or("world-extraction-scope")?,
                    &fence,
                    provenance,
                    now,
                )?;
            }
            jobs::finish(
                &tx,
                job,
                chunk.source.key.end,
                chunk.total_bytes,
                chunk.source.finalized,
            )?;
            tx.commit().map_err(database_error)?;
            Ok(())
        })
    })
}
