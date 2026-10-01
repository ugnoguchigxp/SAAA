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
    assert_eq!(a.provenance.extractor_version, "world-extraction-v2");
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

struct StagedExtract {
    continuity: std::sync::atomic::AtomicUsize,
    world: std::sync::atomic::AtomicUsize,
}
#[async_trait::async_trait]
impl worker::Extractor for StagedExtract {
    async fn extract(
        &self,
        input: Value,
        _: Arc<crate::RunCancellation>,
    ) -> Result<String, String> {
        self.continuity
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(json!({"candidates":[{"kind":"objective","semantic_key":"daily-objective","value":"設計対象を理解する","status":"active","task_request":input["request_scope"],"replaces":null}],"no_change":false}).to_string())
    }
    async fn extract_world(
        &self,
        input: Value,
        cancel: Arc<crate::RunCancellation>,
    ) -> Result<Option<String>, String> {
        assert!(
            input["current"]
                .as_array()
                .unwrap()
                .iter()
                .any(|a| a["kind"] == "objective"),
            "World must see the objective committed by the previous stage"
        );
        if self.world.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
            return Err("world-extraction-invalid".into());
        }
        let objective = input["current"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["kind"] == "objective")
            .unwrap()["id"]
            .clone();
        let text = input["source"]["text"].as_str().unwrap();
        let _ = cancel;
        Ok(Some(json!({"candidates":[{"kind":"world_entity","payload":{"type":"entity","schema_version":2,"entity_id":"daily-goal","entity_kind":"goal","name":text,"aliases":[],"objective_assertion_id":objective},"quote":text,"quote_start":0,"quote_end":text.len(),"epistemic":"user_reported","replaces":null}],"no_change":false}).to_string()))
    }
    fn provenance(&self) -> Provenance {
        Extract.provenance()
    }
}
#[tokio::test]
async fn world_maintenance_resumes_world_without_replaying_committed_continuity() {
    let writer = writer_db();
    writer
        .write(|c| {
            insert_source(c, PROJECT, "stage-source", "設計対象");
            c.execute(
                "UPDATE personal_scope SET last_foreground_at=0,recovery_ready=1",
                [],
            )
            .unwrap();
            Ok(())
        })
        .unwrap();
    let extractor = StagedExtract {
        continuity: 0.into(),
        world: 0.into(),
    };
    assert!(worker::tick_isolated(&writer, &extractor, true)
        .await
        .is_err());
    assert!(writer
        .read_serialized(store::load)
        .unwrap()
        .assertions
        .values()
        .any(|a| a.kind == Kind::Objective));
    let stage: String = writer
        .read_serialized(|c| {
            c.query_row(
                "SELECT stage FROM personal_jobs WHERE status='queued'",
                [],
                |r| r.get(0),
            )
            .map_err(crate::database_error)
        })
        .unwrap();
    assert_eq!(stage, "world");
    writer
        .write(|c| {
            c.execute("UPDATE personal_jobs SET next_attempt_at=0", [])
                .unwrap();
            Ok(())
        })
        .unwrap();
    assert!(worker::tick_isolated(&writer, &extractor, true)
        .await
        .unwrap());
    assert_eq!(
        extractor
            .continuity
            .load(std::sync::atomic::Ordering::SeqCst),
        1
    );
    assert_eq!(extractor.world.load(std::sync::atomic::Ordering::SeqCst), 2);
    let ledger = writer.read_serialized(store::load).unwrap();
    assert_eq!(
        ledger
            .assertions
            .values()
            .filter(|a| a.kind == Kind::WorldEntity)
            .count(),
        1
    );
    let goal = ledger
        .assertions
        .values()
        .find(|a| a.kind == Kind::WorldEntity)
        .unwrap();
    let objective = ledger
        .assertions
        .values()
        .find(|a| a.kind == Kind::Objective)
        .unwrap();
    assert!(goal.depends_on.contains(&objective.id));
}

#[tokio::test]
async fn world_maintenance_own_user_scope_commits_without_fake_project() {
    let writer = writer_db();
    let scope = writer
        .write(|c| {
            let principal: String = c
                .query_row("SELECT principal FROM personal_scope", [], |r| r.get(0))
                .unwrap();
            let scope = format!("user:{principal}");
            insert_source(c, &scope, "daily-source", "日常の設計対象");
            c.execute(
                "UPDATE personal_scope SET last_foreground_at=0,recovery_ready=1",
                [],
            )
            .unwrap();
            Ok(scope)
        })
        .unwrap();
    assert!(worker::tick_isolated(&writer, &Extract, true)
        .await
        .unwrap());
    let ledger = writer.read_serialized(store::load).unwrap();
    let entity = ledger
        .assertions
        .values()
        .find(|a| a.kind == Kind::WorldEntity)
        .unwrap();
    assert_eq!(entity.access.task_request.as_deref(), Some(scope.as_str()));
    assert_eq!(
        ledger.status(&entity.id, crate::memory::personal_state::now()),
        Status::Active
    );
}

#[test]
fn world_maintenance_multisource_quotes_bind_versions_and_scope() {
    let c = memory_db();
    let first = insert_source(&c, PROJECT, "window-first", "cache introduced");
    let second = insert_source(&c, PROJECT, "window-second", "speed improved");
    insert_source(
        &c,
        "project:other",
        "window-other",
        "unrelated private context",
    );
    let text = "exception on cold start";
    let primary = insert_source(&c, PROJECT, "window-primary", text);
    let window =
        crate::memory::personal_state::sources::world_context(&c, &primary, PROJECT, text.len())
            .unwrap();
    assert_eq!(window.len(), 2);
    assert!(window
        .iter()
        .all(|chunk| chunk.source.key.id != "window-other"));
    let texts = window
        .iter()
        .map(|chunk| {
            (
                (chunk.source.key.id.clone(), chunk.source.key.version),
                chunk.text.clone(),
            )
        })
        .collect();
    let value = json!({"candidates":[{"kind":"world_entity","payload":{"type":"entity","schema_version":2,"entity_id":"cache","entity_kind":"concept","name":"cache","aliases":[],"objective_assertion_id":null},"quote":text,"quote_start":0,"quote_end":text.len(),"epistemic":"inferred","replaces":null,"additional_quotes":[{"source_id":first.key.id,"version":first.key.version,"start":0,"end":16,"quote":"cache introduced"},{"source_id":second.key.id,"version":second.key.version,"start":0,"end":14,"quote":"speed improved"}]}],"no_change":false});
    let parsed = saaa_personal_state_core::world::extraction::Extraction::parse_with_sources(
        &value.to_string(),
        text,
        &texts,
    )
    .unwrap();
    let refs = window
        .iter()
        .map(|chunk| chunk.source.clone())
        .collect::<Vec<_>>();
    extraction::commit_with_sources(
        &c,
        &parsed,
        &primary,
        &refs,
        PROJECT,
        "window-fixture",
        worker::Extractor::provenance(&Extract),
        crate::memory::personal_state::now(),
    )
    .unwrap();
    let ledger = store::load(&c).unwrap();
    let entity = ledger
        .assertions
        .values()
        .find(|a| a.kind == Kind::WorldEntity)
        .unwrap();
    assert_eq!(entity.evidence.len(), 3);
    assert!(entity.evidence.iter().any(|key| key.id == first.key.id));
    let mut wrong = value.clone();
    wrong["candidates"][0]["additional_quotes"][0]["version"] = json!(2);
    assert!(
        saaa_personal_state_core::world::extraction::Extraction::parse_with_sources(
            &wrong.to_string(),
            text,
            &texts
        )
        .is_err()
    );
    c.execute(
        "INSERT INTO personal_tombstones(source_id,forgotten_at) VALUES(?1,?2)",
        rusqlite::params![first.key.id, crate::memory::personal_state::now()],
    )
    .unwrap();
    assert!(store::load(&c)
        .unwrap()
        .assertions
        .values()
        .all(|a| a.kind != Kind::WorldEntity));
}

struct PausedWorldExtract {
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}
#[async_trait::async_trait]
impl worker::Extractor for PausedWorldExtract {
    async fn extract(
        &self,
        input: Value,
        cancel: Arc<crate::RunCancellation>,
    ) -> Result<String, String> {
        Extract.extract(input, cancel).await
    }
    async fn extract_world(
        &self,
        input: Value,
        cancel: Arc<crate::RunCancellation>,
    ) -> Result<Option<String>, String> {
        self.entered.notify_one();
        self.release.notified().await;
        Extract.extract_world(input, cancel).await
    }
    fn provenance(&self) -> Provenance {
        worker::Extractor::provenance(&Extract)
    }
}
#[tokio::test]
async fn world_maintenance_slow_local_generation_releases_writer_and_stale_lease_cannot_commit() {
    let writer = Arc::new(writer_db());
    writer
        .write(|c| {
            insert_source(c, PROJECT, "paused-source", "pending local result");
            c.execute("UPDATE personal_scope SET last_foreground_at=0", [])
                .unwrap();
            Ok(())
        })
        .unwrap();
    let extractor = Arc::new(PausedWorldExtract {
        entered: tokio::sync::Notify::new(),
        release: tokio::sync::Notify::new(),
    });
    let task_writer = writer.clone();
    let task_extractor = extractor.clone();
    let task = tokio::spawn(async move {
        worker::tick_isolated(&task_writer, task_extractor.as_ref(), true).await
    });
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        extractor.entered.notified(),
    )
    .await
    .unwrap();
    let foreground_writer = writer.clone();
    tokio::time::timeout(std::time::Duration::from_secs(2),tokio::task::spawn_blocking(move || foreground_writer.transact(|c| {
        c.execute("UPDATE personal_jobs SET status='queued',lease_generation=lease_generation+1 WHERE status='running'",[]).map_err(crate::database_error)?;
        Ok(())
    }))).await.unwrap().unwrap().unwrap();
    extractor.release.notify_one();
    assert!(task.await.unwrap().is_err());
    assert!(writer
        .read_serialized(store::load)
        .unwrap()
        .assertions
        .values()
        .all(|a| !a.kind.is_world()));
}

#[tokio::test]
async fn world_maintenance_user_goal_uses_shared_objective_without_changing_continuity_scope() {
    let writer = writer_db();
    let scope = writer
        .write(|c| {
            let principal: String = c
                .query_row("SELECT principal FROM personal_scope", [], |r| r.get(0))
                .unwrap();
            let scope = format!("user:{principal}");
            insert_source(c, &scope, "user-goal-source", "日常の目標");
            c.execute("UPDATE personal_scope SET last_foreground_at=0", [])
                .unwrap();
            Ok(scope)
        })
        .unwrap();
    let extractor = StagedExtract {
        continuity: 0.into(),
        world: 0.into(),
    };
    assert!(worker::tick_isolated(&writer, &extractor, true)
        .await
        .is_err());
    writer
        .write(|c| {
            c.execute("UPDATE personal_jobs SET next_attempt_at=0", [])
                .unwrap();
            Ok(())
        })
        .unwrap();
    assert!(worker::tick_isolated(&writer, &extractor, true)
        .await
        .unwrap());
    let ledger = writer.read_serialized(store::load).unwrap();
    let objective = ledger
        .assertions
        .values()
        .find(|a| a.kind == Kind::Objective)
        .unwrap();
    assert!(objective.access.task_request.is_none());
    let goal = ledger
        .assertions
        .values()
        .find(|a| a.kind == Kind::WorldEntity)
        .unwrap();
    assert_eq!(goal.access.task_request.as_deref(), Some(scope.as_str()));
    assert!(goal.depends_on.contains(&objective.id));
}
