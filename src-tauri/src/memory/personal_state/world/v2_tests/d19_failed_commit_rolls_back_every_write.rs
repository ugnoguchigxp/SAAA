use super::*;
#[test]
pub(super) fn d19_failed_commit_rolls_back_every_write() {
    use saaa_personal_state_core::world::EntityKind;
    use std::collections::BTreeMap;
    let writer = writer_db();
    let source = writer
        .write(|c| Ok(insert_source(c, PROJECT, "rb1", "rollback source")))
        .unwrap();
    // Commit clock must not precede the source's recorded_at.
    let now_ms = now();
    let revision_before: u64 = writer
        .read_serialized(|c| {
            c.query_row(
                "SELECT revision FROM personal_scope WHERE id='primary'",
                [],
                |r| r.get(0),
            )
            .map_err(crate::database_error)
        })
        .unwrap();
    let built = vec![
        entity_assertion(
            "rb_e1",
            "rb_e1p",
            "rbe1",
            EntityKind::Concept,
            "RB1",
            &[],
            &source,
            PROJECT,
            now_ms,
        ),
        entity_assertion(
            "rb_e2",
            "rb_e2p",
            "rbe2",
            EntityKind::Concept,
            "RB2",
            &[],
            &source,
            PROJECT,
            now_ms,
        ),
    ];
    // A payload byte count that does not match is a late failure after the
    // first assertion has already been written; the transaction must undo it.
    let error = writer
        .write(|c| {
            let ledger = store::load(c)?;
            let patch_id = crate::new_id("patch");
            let mut assertions = Vec::new();
            let mut payload_map: BTreeMap<String, serde_json::Value> = BTreeMap::new();
            for item in built {
                payload_map.insert(item.assertion.payload_ref.clone(), item.payload);
                assertions.push(item.assertion);
            }
            for assertion in assertions.iter_mut() {
                assertion.recorded_at = now_ms;
                assertion.effective_at = now_ms;
            }
            let inputs: BTreeSet<SourceKey> = assertions
                .iter()
                .flat_map(|a| a.input_dependencies.iter().cloned())
                .collect();
            let mut transitions = Vec::new();
            let mut sequence = ledger.transitions.last().map_or(1, |t| t.sequence + 1);
            for assertion in &assertions {
                transitions.push(Transition {
                    id: crate::new_id("transition"),
                    sequence,
                    assertion_id: assertion.id.clone(),
                    action: Action::Assert,
                    reason_code: "world-fixture".into(),
                    evidence: assertion.evidence.clone(),
                    input_dependencies: assertion.input_dependencies.clone(),
                    recorded_at: now_ms,
                });
                sequence += 1;
            }
            let patch = StatePatch {
                id: patch_id.clone(),
                base_revision: ledger.revision,
                input_epoch: ledger.input_epoch,
                policy_revision: ledger.policy_revision,
                fence: "fence-rollback".into(),
                assertions,
                transitions,
                coverage: Vec::new(),
            };
            let mut payload_bytes: BTreeMap<String, usize> = payload_map
                .iter()
                .map(|(k, v)| Ok((k.clone(), crate::memory::personal_state::encode(v)?.len())))
                .collect::<Result<_, String>>()?;
            payload_bytes.insert("rb_e2p".into(), 1);
            let context = CommitContext {
                access: AccessRequest {
                    principal: &ledger.principal,
                    scope: "primary",
                    task_request: Some(PROJECT),
                    purpose: Purpose::StateExtract,
                    max_classification: Classification::Confidential,
                    policy_revision: ledger.policy_revision,
                    authorized: true,
                },
                enabled: true,
                now: now_ms,
                live_fence: "fence-rollback",
                issued_patch_id: &patch_id,
                issued_assertion_ids: patch.assertions.iter().map(|a| a.id.clone()).collect(),
                issued_payload_bytes: payload_bytes,
                evidence_allowlist: inputs.clone(),
                input_dependencies: inputs,
            };
            store::commit(c, &patch, &context, &payload_map)
        })
        .unwrap_err();
    assert_eq!(
        error, "personal-payload-budget",
        "unexpected error: {error}"
    );
    writer
        .read_serialized(|c| {
            let assertions: u64 = c
                .query_row(
                    "SELECT count(*) FROM personal_assertions WHERE id IN ('rb_e1','rb_e2')",
                    [],
                    |r| r.get(0),
                )
                .map_err(crate::database_error)?;
            let payloads: u64 = c
                .query_row(
                    "SELECT count(*) FROM personal_payloads WHERE id IN ('rb_e1p','rb_e2p')",
                    [],
                    |r| r.get(0),
                )
                .map_err(crate::database_error)?;
            let revision: u64 = c
                .query_row(
                    "SELECT revision FROM personal_scope WHERE id='primary'",
                    [],
                    |r| r.get(0),
                )
                .map_err(crate::database_error)?;
            assert_eq!(assertions, 0, "assertions must roll back");
            assert_eq!(payloads, 0, "payloads must roll back");
            assert_eq!(revision, revision_before, "revision must roll back");
            Ok(())
        })
        .unwrap();
}
#[test]
pub(super) fn d13_v1_relation_can_be_superseded_by_an_equivalent_v2_relation() {
    use saaa_personal_state_core::world::{
        Basis as V1Basis, EffectInput as V1Effect, EntityKind as V1Kind, RelationType as V1Type,
    };
    let writer = writer_db();
    let source = writer
        .write(|c| Ok(insert_source(c, PROJECT, "xv", "cross-version source")))
        .unwrap();
    let now_ms = now();
    let mut committer = Committer {
        writer: &writer,
        project: PROJECT,
    };
    committer
        .commit(
            "xv-base",
            vec![
                entity_assertion(
                    "xv_c1",
                    "xv_c1p",
                    "xvc1",
                    V1Kind::Concept,
                    "XV Concept",
                    &[],
                    &source,
                    PROJECT,
                    now_ms,
                ),
                entity_assertion(
                    "xv_m1",
                    "xv_m1p",
                    "xvm1",
                    V1Kind::Metric,
                    "XV Metric",
                    &[],
                    &source,
                    PROJECT,
                    now_ms,
                ),
            ],
        )
        .unwrap();
    committer
        .commit(
            "xv-v1",
            vec![relation_assertion(
                "xv_v1",
                "xv_v1p",
                "xvc1",
                "xvm1",
                V1Type::Increases,
                Some(V1Effect::Intervention),
                &[],
                V1Basis::ModelHypothesis,
                vec![(
                    source.key.clone(),
                    saaa_personal_state_core::world::Stance::Context,
                )],
                &source,
                PROJECT,
                BTreeSet::from(["xv_c1".to_string(), "xv_m1".to_string()]),
                now_ms,
            )],
        )
        .unwrap();
    // Same logical identity (same valid_from), v2 payload, explicit Supersede.
    let v2 = v2_relation_assertion(
        "xv_v2",
        v2_relation_value(
            "xvc1",
            "xvm1",
            "increases",
            Some("intervention"),
            &[],
            None,
            None,
            None,
            None,
            std::slice::from_ref(&source.key),
            None,
            None,
        ),
        &source,
        PROJECT,
        BTreeSet::from(["xv_c1".to_string(), "xv_m1".to_string()]),
        now_ms,
    );
    committer
        .commit_superseding("xv-supersede", vec![v2], "xv_v1")
        .unwrap();
    // The v1 assertion is superseded and the v2 one is live; a v2 query reads it.
    let slice = run_v2(
        &writer,
        &[WorldSeed::EntityId("xvc1".into())],
        IncludeFlags::default(),
        8_192,
        now(),
    )
    .unwrap();
    assert!(slice
        .relations
        .iter()
        .any(|relation| relation.assertion_id == "xv_v2"));
    assert!(!slice
        .relations
        .iter()
        .any(|relation| relation.assertion_id == "xv_v1"));
}
