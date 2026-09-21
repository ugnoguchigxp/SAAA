#![cfg(test)]
use super::{extraction, test_support::*};
use crate::memory::personal_state::{store, worker};
use saaa_personal_state_core::*;
use serde_json::{json, Value};
use std::sync::Arc;
struct Extract;
#[async_trait::async_trait]
impl worker::Extractor for Extract {
    async fn extract(&self, _: Value, _: Arc<crate::RunCancellation>) -> Result<String, String> {
        Ok(json!({"candidates":[],"no_change":true}).to_string())
    }
    async fn extract_world(
        &self,
        input: Value,
        _: Arc<crate::RunCancellation>,
    ) -> Result<Option<String>, String> {
        let text = input["source"]["text"].as_str().unwrap();
        Ok(Some(json!({"candidates":[{"kind":"world_entity","payload":{"type":"entity","schema_version":2,"entity_id":"subject","entity_kind":"concept","name":text,"aliases":[],"objective_assertion_id":null},"quote":text,"quote_start":0,"quote_end":text.len(),"epistemic":"user_reported","replaces":null}],"no_change":false}).to_string()))
    }
    fn provenance(&self) -> Provenance {
        Provenance {
            model: "fixture".into(),
            release: "fixture".into(),
            extractor_version: "fixture".into(),
            prompt_digest: "fixture".into(),
            schema_version: "fixture".into(),
            config_digest: "fixture".into(),
            runtime_event: None,
        }
    }
}
#[tokio::test]
async fn wr_t09_finalized_user_reaches_existing_world_ledger() {
    let writer = writer_db();
    writer
        .write(|c| {
            insert_source(c, PROJECT, "extract-user", "設計対象");
            c.execute("UPDATE personal_jobs SET next_attempt_at=0", [])
                .unwrap();
            c.execute(
                "UPDATE personal_scope SET last_foreground_at=0,recovery_ready=1",
                [],
            )
            .unwrap();
            Ok(())
        })
        .unwrap();
    assert!(worker::tick_isolated(&writer, &Extract, true)
        .await
        .unwrap());
    let ledger = writer.read_serialized(store::load).unwrap();
    let a = ledger
        .assertions
        .values()
        .find(|a| a.kind == Kind::WorldEntity)
        .expect("world committed");
    assert_eq!(a.access.task_request.as_deref(), Some(PROJECT));
    assert_eq!(a.provenance.extractor_version, "world-extraction-v1");
    assert_eq!(
        ledger.status(&a.id, crate::memory::personal_state::now()),
        Status::Active
    );
    assert!(!worker::tick_isolated(&writer, &Extract, true)
        .await
        .unwrap());
}
#[test]
fn wr_t10_correction_and_revoked_source_cannot_revive_old_fact() {
    let mut c = memory_db();
    let source = insert_source(&c, PROJECT, "correction-source", "新しい設計");
    let candidate = json!({"kind":"world_entity","payload":{"type":"entity","schema_version":2,"entity_id":"subject","entity_kind":"concept","name":"設計","aliases":[],"objective_assertion_id":null},"quote":"新しい設計","quote_start":0,"quote_end":15,"epistemic":"user_reported","replaces":null});
    let mut value = json!({"candidates":[candidate],"no_change":false});
    let parsed = saaa_personal_state_core::world::extraction::Extraction::parse(
        &value.to_string(),
        "新しい設計",
    )
    .unwrap();
    extraction::commit(
        &c,
        &parsed,
        &source,
        PROJECT,
        "fixture",
        worker::Extractor::provenance(&Extract),
        crate::memory::personal_state::now(),
    )
    .unwrap();
    let ledger = store::load(&c).unwrap();
    let old = ledger
        .assertions
        .values()
        .find(|a| a.kind == Kind::WorldEntity)
        .unwrap()
        .id
        .clone();
    value["candidates"][0]["replaces"] = json!(old);
    value["candidates"][0]["payload"]["name"] = json!("新しい設計");
    let parsed = saaa_personal_state_core::world::extraction::Extraction::parse(
        &value.to_string(),
        "新しい設計",
    )
    .unwrap();
    let tx = c.transaction().unwrap();
    extraction::commit(
        &tx,
        &parsed,
        &source,
        PROJECT,
        "fixture",
        worker::Extractor::provenance(&Extract),
        crate::memory::personal_state::now(),
    )
    .unwrap();
    tx.commit().unwrap();
    assert_eq!(
        store::load(&c)
            .unwrap()
            .status(&old, crate::memory::personal_state::now()),
        Status::Superseded
    );
    // Removing the source-to-project authorization must reject a delayed candidate.
    c.execute(
        "DELETE FROM personal_source_scope_refs WHERE source_id=?1",
        [&source.key.id],
    )
    .unwrap();
    assert!(extraction::commit(
        &c,
        &parsed,
        &source,
        PROJECT,
        "fixture",
        worker::Extractor::provenance(&Extract),
        crate::memory::personal_state::now()
    )
    .is_err());
}

#[test]
fn wr_t09_five_elements_commit_with_host_evidence_and_remain_hypotheses() {
    use saaa_personal_state_core::world::model_v2::EntityKindV2;
    let f = crate::runtime::context::world::g1_tests::g1_fixture();
    let text = "改善は速度に依存し、速度との相関も仮説です。応答改善が目標です。";
    let source = f
        .writer
        .write(|c| Ok(insert_source(c, PROJECT, "five-natural", text)))
        .unwrap();
    f.set_now(crate::memory::personal_state::now() + 1);
    let mut payloads = vec![
        v2_entity_payload("natural-tech", EntityKindV2::Concept, "改善", &[], None),
        v2_entity_payload("natural-metric", EntityKindV2::Metric, "速度", &[], None),
        v2_entity_payload(
            "natural-goal",
            EntityKindV2::Goal,
            "応答改善",
            &[],
            Some("g1-obj"),
        ),
        v2_focus_value("natural-tech", "current_work", Some("g1-obj")),
    ];
    for (kind, effect, sign, target) in [
        ("depends_on", None, None, "natural-metric"),
        ("correlates_with", None, Some("positive"), "natural-metric"),
        ("decreases", Some("intervention"), None, "natural-metric"),
        ("serves_goal", None, None, "natural-goal"),
    ] {
        payloads.push(v2_relation_value(
            if kind == "correlates_with" {
                "decode"
            } else {
                "natural-tech"
            },
            target,
            kind,
            effect,
            &[],
            None,
            None,
            sign,
            None,
            &[],
            None,
            None,
        ));
    }
    let candidates: Vec<_> = payloads.into_iter().map(|payload| {
        let kind = format!("world_{}", payload["type"].as_str().unwrap());
        json!({"kind":kind,"payload":payload,"quote":text,"quote_start":0,"quote_end":text.len(),"epistemic":"inferred","replaces":null})
    }).collect();
    let raw = json!({"candidates":candidates,"no_change":false}).to_string();
    let parsed =
        saaa_personal_state_core::world::extraction::Extraction::parse(&raw, text).unwrap();
    f.writer
        .write(|c| {
            extraction::commit(
                c,
                &parsed,
                &source,
                PROJECT,
                "five-natural",
                worker::Extractor::provenance(&Extract),
                f.now(),
            )
        })
        .unwrap();
    let ledger = f.writer.read_serialized(store::load).unwrap();
    let assertions: Vec<_> = ledger
        .assertions
        .values()
        .filter(|a| a.evidence.contains(&source.key))
        .collect();
    assert_eq!(assertions.len(), 8);
    assert!(assertions
        .iter()
        .all(|a| ledger.status(&a.id, f.now()) == Status::Active));
    for a in assertions.iter().filter(|a| a.kind == Kind::WorldRelation) {
        let p = f
            .writer
            .read_serialized(|c| super::validation::load_payload_json(c, &a.payload_ref))
            .unwrap();
        assert_eq!(p["epistemic"], "hypothesis");
        assert_eq!(p["basis"], "model_hypothesis");
        assert_eq!(p["evidence_stances"][0]["source"]["id"], source.key.id);
    }
    let frame = f
        .service()
        .prepare_frame(f.request(
            f.access(),
            vec![],
            Some(crate::runtime::context::world::g1_tests::graph_request(
                "改善",
            )),
        ))
        .unwrap();
    let encoded = serde_json::to_string(frame.frame()).unwrap();
    for element in ["natural-tech", "decreases"] {
        assert!(encoded.contains(element), "{element}: {encoded}");
    }
    // Graph projection is bounded; omitted relationships remain in the canonical ledger.
    let relations: Vec<_> = assertions
        .iter()
        .filter(|a| a.kind == Kind::WorldRelation)
        .map(|a| {
            f.writer
                .read_serialized(|c| super::validation::load_payload_json(c, &a.payload_ref))
                .unwrap()["relation_type"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect();
    for kind in ["depends_on", "correlates_with", "decreases", "serves_goal"] {
        assert!(relations.iter().any(|r| r == kind));
    }
    let before = ledger.assertions.len();
    f.writer
        .write(|c| {
            extraction::commit(
                c,
                &parsed,
                &source,
                PROJECT,
                "five-natural",
                worker::Extractor::provenance(&Extract),
                f.now(),
            )
        })
        .unwrap();
    assert_eq!(
        f.writer
            .read_serialized(store::load)
            .unwrap()
            .assertions
            .len(),
        before
    );
}
