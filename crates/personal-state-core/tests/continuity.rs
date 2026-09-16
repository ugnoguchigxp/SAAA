use saaa_personal_state_core::*;
use std::collections::{BTreeMap, BTreeSet};
fn set<T: Ord>(values: impl IntoIterator<Item = T>) -> BTreeSet<T> {
    values.into_iter().collect()
}
fn key(id: &str) -> SourceKey {
    SourceKey {
        id: id.into(),
        version: 1,
        start: 0,
        end: 42,
    }
}
fn access() -> AccessScope {
    AccessScope {
        principal: "owner".into(),
        scope: "primary".into(),
        task_request: Some("task/request-1".into()),
        purposes: set([
            Purpose::Reasoning,
            Purpose::StateExtract,
            Purpose::Diagnostics,
        ]),
        classification: Classification::Confidential,
        policy_revision: 1,
    }
}
fn request() -> AccessRequest<'static> {
    AccessRequest {
        principal: "owner",
        scope: "primary",
        task_request: Some("task/request-1"),
        purpose: Purpose::StateExtract,
        max_classification: Classification::Confidential,
        policy_revision: 1,
        authorized: true,
    }
}
fn source(id: &str, sequence: u64) -> SourceRef {
    SourceRef {
        key: key(id),
        digest: "a".repeat(64),
        sequence,
        recorded_at: 10,
        role: SourceRole::User,
        access: access(),
        available: true,
        valid_until: None,
        finalized: true,
    }
}
fn ledger() -> Ledger {
    let mut ledger = Ledger::new("owner".into(), "primary".into(), 1);
    ledger.record_source(source("s1", 1)).unwrap();
    ledger
}
fn assertion(id: &str) -> Assertion {
    Assertion {
        id: id.into(),
        kind: Kind::Constraint,
        semantic_key: "送付先".into(),
        payload_ref: format!("payload-{id}"),
        access: access(),
        evidence: set([key("s1")]),
        depends_on: BTreeSet::new(),
        input_dependencies: set([key("s1")]),
        provenance: Provenance {
            model: "fixture-27b".into(),
            release: "fixture".into(),
            extractor_version: "1".into(),
            prompt_digest: "fixture".into(),
            schema_version: "1".into(),
            config_digest: "fixture".into(),
            runtime_event: None,
        },
        observed_at: 10,
        effective_at: 10,
        recorded_at: 10,
        valid_from: 10,
        valid_until: None,
    }
}
fn event(id: &str, sequence: u64, action: Action) -> Transition {
    Transition {
        id: format!("e-{sequence}"),
        sequence,
        assertion_id: id.into(),
        action,
        reason_code: "explicit-user".into(),
        evidence: set([key("s1")]),
        input_dependencies: set([key("s1")]),
        recorded_at: 10,
    }
}
fn patch() -> StatePatch {
    StatePatch {
        id: "p1".into(),
        base_revision: 0,
        input_epoch: 1,
        policy_revision: 1,
        fence: "g1/a1".into(),
        assertions: vec![assertion("a1")],
        transitions: vec![
            event("a1", 1, Action::Assert),
            event("a1", 2, Action::Activate),
        ],
        coverage: vec![(key("s1"), Coverage::Applied)],
    }
}
fn context() -> CommitContext<'static> {
    CommitContext {
        access: request(),
        enabled: true,
        now: 10,
        live_fence: "g1/a1",
        issued_patch_id: "p1",
        issued_assertion_ids: set(["a1".into(), "a2".into(), "a3".into()]),
        issued_payload_bytes: BTreeMap::from([
            ("payload-a1".into(), 42),
            ("payload-a2".into(), 42),
            ("payload-a3".into(), 42),
        ]),
        evidence_allowlist: set([key("s1")]),
        input_dependencies: set([key("s1")]),
    }
}
#[test]
fn c1_active_required_and_no_silent_budget_omission() {
    let mut l = ledger();
    l.apply(&patch(), &context()).unwrap();
    let projection = l.project(&request(), 10).unwrap();
    assert_eq!(projection.items["a1"], Status::Active);
    assert_eq!(
        select_projection(&projection, &set(["a1".into()]), &[], 101, 100),
        Err(Error::Limit)
    );
    assert_eq!(
        select_projection(&projection, &set(["missing".into()]), &[], 0, 100),
        Err(Error::RequiredMissing)
    );
}
#[test]
fn c2_replay_conflict_epoch_fence_and_atomic_rejection() {
    let mut l = ledger();
    let p = patch();
    let c = context();
    assert_eq!(l.apply(&p, &c), Ok(true));
    assert_eq!(l.apply(&p, &c), Ok(false));
    let mut changed = p.clone();
    changed.assertions[0].semantic_key = "改ざん".into();
    assert_eq!(l.apply(&changed, &c), Err(Error::Conflict));
    let mut fresh = ledger();
    fresh.record_source(source("s2", 2)).unwrap();
    assert_eq!(fresh.apply(&p, &c), Err(Error::StalePatch));
    let mut c = c;
    c.live_fence = "cancelled";
    assert_eq!(ledger().apply(&p, &c), Err(Error::InvalidFence));
    let mut l = ledger();
    let before = l.clone();
    let mut p = patch();
    p.transitions[1].sequence = 3;
    assert_eq!(l.apply(&p, &context()), Err(Error::InvalidTransition));
    assert_eq!(l, before);
}
#[test]
fn c2_rejects_cycles_unknown_future_and_unmaterialized_optional_evidence() {
    for dependency in ["unknown", "a1"] {
        let mut p = patch();
        p.assertions[0].depends_on.insert(dependency.into());
        assert!(matches!(
            ledger().apply(&p, &context()),
            Err(Error::Cycle | Error::UnknownDependency)
        ));
    }
    let mut p = patch();
    p.assertions[0].evidence.insert(key("future"));
    assert_eq!(ledger().apply(&p, &context()), Err(Error::InvalidPatch));
    let mut c = context();
    c.evidence_allowlist.clear();
    assert_eq!(ledger().apply(&patch(), &c), Err(Error::InvalidPatch));
}
#[test]
fn c3_principal_task_scope_purpose_and_classification_do_not_leak() {
    let mut l = ledger();
    l.apply(&patch(), &context()).unwrap();
    let mut r = request();
    r.principal = "other";
    assert_eq!(l.project(&r, 10), Err(Error::Unauthorized));
    r = request();
    r.scope = "other";
    assert_eq!(l.project(&r, 10), Err(Error::Unauthorized));
    r = request();
    r.task_request = Some("other");
    assert!(l.project(&r, 10).unwrap().items.is_empty());
    r = request();
    r.purpose = Purpose::Tactical;
    assert!(l.project(&r, 10).unwrap().items.is_empty());
    r = request();
    r.max_classification = Classification::Public;
    assert!(l.project(&r, 10).unwrap().items.is_empty());
    for widen in [0, 1, 2] {
        let mut p = patch();
        let a = &mut p.assertions[0];
        match widen {
            0 => a.access.task_request = None,
            1 => a.access.classification = Classification::Public,
            _ => {
                a.access.purposes.insert(Purpose::Tactical);
            }
        }
        assert_eq!(ledger().apply(&p, &context()), Err(Error::Unauthorized));
    }
}
#[test]
fn c4_expiry_is_enforced_without_worker_and_old_versions_stay_stale() {
    let mut l = ledger();
    let mut p = patch();
    p.assertions[0].valid_until = Some(20);
    l.apply(&p, &context()).unwrap();
    assert_eq!(l.status("a1", 19), Status::Active);
    assert_eq!(l.status("a1", 20), Status::Stale);
    assert_eq!(l.status("a1", 9), Status::Stale);
    let mut newer = source("s1", 2);
    newer.key.version = 2;
    l.record_source(newer).unwrap();
    assert_eq!(l.status("a1", 10), Status::Stale);
    let mut old = source("s1", 1);
    old.key.start = 1;
    assert_eq!(l.record_source(old), Err(Error::InvalidSource));
}
#[test]
fn c4_c5_forget_closure_includes_unquoted_inputs_and_derived_derivatives() {
    let mut l = ledger();
    l.record_source(source("base-only", 2)).unwrap();
    let mut p = patch();
    p.input_epoch = 2;
    let mut c = context();
    c.input_dependencies.insert(key("base-only"));
    p.assertions[0].input_dependencies = c.input_dependencies.clone();
    for t in &mut p.transitions {
        t.input_dependencies = c.input_dependencies.clone();
    }
    let mut second = assertion("a2");
    second.semantic_key = "関連条件".into();
    second.depends_on.insert("a1".into());
    second.input_dependencies = c.input_dependencies.clone();
    p.assertions.push(second);
    let mut third = assertion("a3");
    third.semantic_key = "派生条件".into();
    third.depends_on.insert("a2".into());
    third.input_dependencies = c.input_dependencies.clone();
    p.assertions.push(third);
    for (id, seq) in [("a2", 3), ("a3", 5)] {
        for (n, action) in [(seq, Action::Assert), (seq + 1, Action::Activate)] {
            let mut t = event(id, n, action);
            t.input_dependencies = c.input_dependencies.clone();
            p.transitions.push(t);
        }
    }
    l.apply(&p, &c).unwrap();
    let effect = l.forget("base-only").unwrap();
    assert_eq!(effect.assertions.len(), 3);
    assert_eq!(effect.payload_refs.len(), 3);
    for id in ["a1", "a2", "a3"] {
        assert_eq!(l.status(id, 10), Status::Invalidated);
    }
    assert_eq!(
        l.record_source(source("base-only", 3)),
        Err(Error::InvalidSource)
    );
    assert!(l.sources[&key("base-only")].digest.is_empty());
    assert_eq!(l.apply(&p, &c), Err(Error::Conflict));
    assert_eq!(l.forget("base-only").unwrap().assertions, effect.assertions);
}
#[test]
fn c1_retraction_does_not_reactivate_and_replacement_is_explicit() {
    let mut l = ledger();
    let mut p = patch();
    p.transitions.push(event("a1", 3, Action::Retract));
    l.apply(&p, &context()).unwrap();
    assert_eq!(l.status("a1", 10), Status::Retracted);
    let mut p = patch();
    p.transitions.push(event("a1", 3, Action::Retract));
    p.transitions.push(event("a1", 4, Action::Activate));
    assert_eq!(
        ledger().apply(&p, &context()),
        Err(Error::InvalidTransition)
    );
    let mut p = patch();
    let mut a2 = assertion("a2");
    a2.effective_at = 10;
    p.assertions.push(a2);
    p.transitions.push(event("a2", 3, Action::Assert));
    p.transitions
        .push(event("a1", 4, Action::Supersede { by: "a2".into() }));
    p.transitions.push(event("a2", 5, Action::Activate));
    let mut l = ledger();
    l.apply(&p, &context()).unwrap();
    assert_eq!(l.status("a1", 10), Status::Superseded);
    assert_eq!(l.status("a2", 10), Status::Active);
}
#[test]
fn c15_pending_correction_and_partial_message_never_establish_old_certainty() {
    let mut l = ledger();
    l.apply(&patch(), &context()).unwrap();
    l.record_source(source("s2", 2)).unwrap();
    assert_eq!(
        l.project(&request(), 10).unwrap().items["a1"],
        Status::Stale
    );
    let mut l = ledger();
    l.sources.get_mut(&key("s1")).unwrap().finalized = false;
    assert_eq!(l.apply(&patch(), &context()), Err(Error::InvalidSource));
    let mut p = patch();
    p.transitions.pop();
    p.coverage[0].1 = Coverage::Candidate;
    l.apply(&p, &context()).unwrap();
    assert_eq!(l.status("a1", 10), Status::Candidate);
}
#[test]
fn c12_disabled_does_not_disable_forget() {
    let mut c = context();
    c.enabled = false;
    assert_eq!(ledger().apply(&patch(), &c), Err(Error::Disabled));
    let mut l = ledger();
    l.forget("s1").unwrap();
    assert!(!l.source_valid(&key("s1"), 10));
}
#[test]
fn strict_patch_schema_and_size_limit() {
    let mut v = serde_json::to_value(patch()).unwrap();
    v["self_reported_epoch"] = 99.into();
    assert!(serde_json::from_value::<StatePatch>(v).is_err());
    let mut p = patch();
    p.assertions[0].payload_ref = "x".repeat(16 * 1024);
    assert_eq!(ledger().apply(&p, &context()), Err(Error::Limit));
}
fn binding() -> ViewBinding {
    ViewBinding {
        allocation: "allocation-1".into(),
        runtime: "27b".into(),
        release: "certified-fixture".into(),
        lease_epoch: 4,
        request_digest: "fixed-request".into(),
    }
}
#[test]
fn c7_one_shot_expiry_and_every_binding_dimension() {
    let make = || {
        OneShotView::new(
            "view-1".into(),
            "digest".into(),
            binding(),
            20,
            vec![key("s1")],
            vec![],
        )
    };
    let selected = set([key("s1")]);
    let mut v = make();
    v.consume(&binding(), &selected, &selected, 10).unwrap();
    assert_eq!(
        v.consume(&binding(), &selected, &selected, 10),
        Err(Error::InvalidFence)
    );
    assert_eq!(
        make().consume(&binding(), &selected, &selected, 20),
        Err(Error::InvalidFence)
    );
    for field in 0..5 {
        let mut changed = binding();
        match field {
            0 => changed.allocation.push('x'),
            1 => changed.runtime.push('x'),
            2 => changed.release.push('x'),
            3 => changed.lease_epoch += 1,
            _ => changed.request_digest.push('x'),
        }
        assert_eq!(
            make().consume(&changed, &selected, &selected, 10),
            Err(Error::InvalidFence)
        );
    }
}
#[test]
fn c8_required_source_and_certified_materialization_boundaries() {
    let budget = Budget {
        certified: true,
        native_tokens: 131_072,
        output_reserve: 4_096,
        safety_margin: 1_976,
        requested_input: 125_000,
        max_bytes: 1_000_000,
    };
    assert_eq!(
        budget.validate_materialized(125_000, 1_000_000, 4_096),
        Ok(())
    );
    for (tokens, bytes, output) in [(125_001, 1, 1), (1, 1_000_001, 1), (1, 1, 4_097)] {
        assert_eq!(
            budget.validate_materialized(tokens, bytes, output),
            Err(Error::Limit)
        );
    }
    let mut unknown = budget;
    unknown.certified = false;
    assert_eq!(unknown.input_limit(), Err(Error::RequiredMissing));
    let mut view = OneShotView::new(
        "v".into(),
        "d".into(),
        binding(),
        20,
        vec![key("s1")],
        vec![(key("s2"), "budget".into())],
    );
    let selected = set([key("s1"), key("s2")]);
    assert_eq!(
        view.consume(&binding(), &selected, &selected, 10),
        Err(Error::RequiredMissing)
    );
    assert_eq!(
        view.consume(&binding(), &set([key("s1")]), &selected, 10),
        Ok(())
    );
}
#[test]
fn c1_optional_omission_is_explicit_and_order_is_stable() {
    let projection = Projection {
        revision: 1,
        input_epoch: 1,
        items: BTreeMap::from([("a".into(), Status::Active), ("b".into(), Status::Active)]),
    };
    let choices = [("a".into(), 30), ("b".into(), 20)];
    let (selected, omitted) =
        select_projection(&projection, &BTreeSet::new(), &choices, 0, 30).unwrap();
    assert_eq!(selected, vec!["a"]);
    assert_eq!(omitted, vec!["b"]);
}

#[test]
fn c2_future_source_and_policy_revision_are_rejected_at_commit() {
    let mut l = ledger();
    l.sources.get_mut(&key("s1")).unwrap().recorded_at = 11;
    assert_eq!(l.apply(&patch(), &context()), Err(Error::InvalidSource));
    let mut c = context();
    c.access.policy_revision = 2;
    assert_eq!(ledger().apply(&patch(), &c), Err(Error::StalePatch));
    let mut c = context();
    c.issued_payload_bytes.insert("payload-a1".into(), 2001);
    assert_eq!(ledger().apply(&patch(), &c), Err(Error::Limit));
}

#[test]
fn c8_item_count_duplicate_omission_and_canonical_order() {
    let keys: Vec<_> = (0..512).map(|i| key(&format!("s{i}"))).collect();
    let selected = set(keys.clone());
    let mut view = OneShotView::new("v".into(), "d".into(), binding(), 20, keys.clone(), vec![]);
    assert_eq!(view.consume(&binding(), &selected, &selected, 10), Ok(()));
    assert_eq!(view.ordered_sources, keys);
    let mut too_many = keys.clone();
    too_many.push(key("extra"));
    let selected = set(too_many.clone());
    let mut view = OneShotView::new("v".into(), "d".into(), binding(), 20, too_many, vec![]);
    assert_eq!(
        view.consume(&binding(), &selected, &selected, 10),
        Err(Error::RequiredMissing)
    );
    let mut view = OneShotView::new(
        "v".into(),
        "d".into(),
        binding(),
        20,
        vec![key("s1"), key("s1")],
        vec![],
    );
    let selected = set([key("s1")]);
    assert_eq!(
        view.consume(&binding(), &selected, &selected, 10),
        Err(Error::RequiredMissing)
    );
}

#[test]
fn c4_source_expiry_and_lost_support_invalidate_dependents() {
    let mut l = ledger();
    l.sources.get_mut(&key("s1")).unwrap().valid_until = Some(15);
    l.apply(&patch(), &context()).unwrap();
    assert_eq!(l.status("a1", 14), Status::Active);
    assert_eq!(l.status("a1", 15), Status::Stale);
}

#[test]
fn c15_historical_evidence_cannot_restore_a_later_retracted_value() {
    let mut l = ledger();
    l.record_source(source("s2", 2)).unwrap();
    let mut p = patch();
    p.input_epoch = 2;
    let mut c = context();
    c.input_dependencies.insert(key("s2"));
    c.evidence_allowlist.insert(key("s2"));
    p.assertions[0].evidence = set([key("s2")]);
    p.assertions[0].input_dependencies = c.input_dependencies.clone();
    p.transitions.push(event("a1", 3, Action::Retract));
    let mut old = assertion("a2");
    old.input_dependencies = c.input_dependencies.clone();
    p.assertions.push(old);
    p.transitions.push(event("a2", 4, Action::Assert));
    p.transitions.push(event("a2", 5, Action::Activate));
    for t in &mut p.transitions {
        t.input_dependencies = c.input_dependencies.clone();
    }
    p.coverage.clear();
    assert_eq!(l.apply(&p, &c), Err(Error::InvalidSource));
    assert!(l.assertions.is_empty());
}
