#![cfg(test)]
use super::*;
use crate::persistence::sqlite::SqliteWriter;
use rusqlite::{params, Connection};
use serde_json::json;
fn db() -> Connection {
    let c = Connection::open_in_memory().unwrap();
    crate::initialize_database(&c).unwrap();
    c
}
fn insert(c: &Connection, id: &str, text: &str) {
    c.execute(
        "INSERT INTO conversation_messages VALUES(?1,?2,'user',?3,?4)",
        params![
            id,
            crate::PRIMARY_CONVERSATION_ID,
            text,
            (now() - 31000).to_string()
        ],
    )
    .unwrap();
}
#[test]
fn inputs_and_jobs_are_atomic_and_all_messages_are_paged() {
    let c = db();
    for n in 0..405 {
        insert(&c, &format!("s{n}"), "原文");
    }
    let mut after = 0;
    let mut count = 0;
    loop {
        let p = sources::page(&c, after, 64).unwrap();
        if p.is_empty() {
            break;
        }
        after = *p.last().unwrap();
        count += p.len();
    }
    assert_eq!(count, 405);
    assert_eq!(
        c.query_row("SELECT count(*) FROM personal_jobs", [], |r| r
            .get::<_, u64>(0))
            .unwrap(),
        405
    );
    {
        let tx = c.unchecked_transaction().unwrap();
        insert(&tx, "rollback", "未確定");
    }
    assert_eq!(
        c.query_row("SELECT count(*) FROM personal_sources", [], |r| r
            .get::<_, u64>(0))
            .unwrap(),
        405
    );
}
#[test]
fn utf8_loader_preserves_middle_and_tail_without_truncation() {
    let c = db();
    let text = format!(
        "{}中間の訂正{}採用しない",
        "あ".repeat(3000),
        "い".repeat(3000)
    );
    insert(&c, "long", &text);
    let mut offset = 0;
    let mut whole = String::new();
    loop {
        let part = sources::load(&c, 1, offset, 4096).unwrap();
        offset = part.source.key.end;
        whole.push_str(&part.text);
        if offset == part.total_bytes {
            break;
        }
    }
    assert_eq!(whole, text);
}
#[test]
fn edit_invalidates_captured_range_and_delete_blocks_resurrection() {
    let c = db();
    insert(&c, "s1", "Aに送る");
    let captured = sources::load(&c, 1, 0, 4096).unwrap();
    store::remember_source(&c, &captured.source).unwrap();
    c.execute(
        "UPDATE conversation_messages SET content='Bに訂正' WHERE id='s1'",
        [],
    )
    .unwrap();
    assert!(sources::revalidate(&c, &captured.source).is_err());
    assert_eq!(sources::load(&c, 2, 0, 4096).unwrap().source.key.version, 2);
    c.execute("DELETE FROM conversation_messages WHERE id='s1'", [])
        .unwrap();
    assert!(sources::load(&c, 2, 0, 4096).is_err());
    assert!(c
        .execute(
            "INSERT INTO conversation_messages VALUES('s1',?1,'user','復活','1')",
            [crate::PRIMARY_CONVERSATION_ID]
        )
        .is_err());
    assert_eq!(store::load(&c).unwrap().tombstones.len(), 1);
}
#[test]
fn unconfigured_contract_is_explicit_and_memory_off_preserves_tracking() {
    let c = db();
    insert(&c, "s1", "訂正");
    assert!(contract::load(&c).is_err());
    assert!(jobs::claim(&c, now(), false).unwrap().is_none());
    let snapshot = commands::snapshot(&c).unwrap();
    assert_eq!(snapshot["pendingCount"], 1);
    assert_eq!(snapshot["contractReady"], false);
}
#[test]
fn one_claim_and_retry_budget_and_epoch_fence() {
    let c = db();
    insert(&c, "s1", "制約");
    let mut t = now();
    for attempt in 0..4 {
        let j = jobs::claim(&c, t, true).unwrap().unwrap();
        assert!(jobs::claim(&c, t, true).unwrap().is_none());
        jobs::failed(&c, &j, t, false, "fixture-failed").unwrap();
        t += [30000, 120000, 600000, 600000][attempt];
    }
    assert!(jobs::claim(&c, t, true).unwrap().is_none());
    c.execute(
        "UPDATE personal_jobs SET status='queued',attempts=0,next_attempt_at=0",
        [],
    )
    .unwrap();
    let j = jobs::claim(&c, t, true).unwrap().unwrap();
    insert(&c, "s2", "訂正");
    assert!(!jobs::valid(&c, &j, t).unwrap());
}
#[test]
fn restore_requires_current_tombstones_and_merges_before_read() {
    let c = db();
    insert(&c, "s1", "忘れる本文");
    assert!(store::recover(&c, None).is_err());
    assert!(store::load(&c).is_err());
    store::recover(&c, Some(&[("s1".into(), now())])).unwrap();
    assert!(sources::load(&c, 1, 0, 4096).is_err());
    assert!(store::load(&c).is_ok());
}
fn manifest(c: &Connection) -> generation::Manifest {
    let l = store::load(c).unwrap();
    generation::Manifest {
        request_digest: "a".repeat(64),
        generation_id: "g1".into(),
        attempt_id: "attempt1".into(),
        run_id: "run1".into(),
        request_revision: 1,
        input_epoch: l.input_epoch,
        policy_revision: l.policy_revision,
        projection_revision: l.revision,
        purpose: "reasoning".into(),
        sources: vec![sources::load(c, 1, 0, 4096).unwrap().source],
        allocation: "fixture".into(),
        runtime: "fixture".into(),
        release: "fixture".into(),
        view_id: Some("view1".into()),
        view_digest: Some("fixture".into()),
        lease_epoch: 1,
        expires_at: now() + 30000,
    }
}
#[test]
fn generation_forget_fences_before_late_results_and_keeps_materialization_separate() {
    let c = db();
    insert(&c, "s1", "入力");
    generation::prepare(&c, &manifest(&c)).unwrap();
    generation::dispatch(&c, "g1").unwrap();
    assert!(generation::dispatch(&c, "g1").is_err());
    c.execute(
        "UPDATE personal_generations SET materialization='succeeded'",
        [],
    )
    .unwrap();
    assert_eq!(
        c.query_row("SELECT status FROM personal_generations", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "running"
    );
    c.execute("DELETE FROM conversation_messages WHERE id='s1'", [])
        .unwrap();
    assert!(generation::allow(&c, "g1").is_err());
    assert!(generation::allow_run(&c, "run1").is_err());
    generation::finish(&c, "g1", false).unwrap();
}
struct Fixture;
#[async_trait::async_trait]
impl worker::Extractor for Fixture {
    async fn extract(
        &self,
        _: serde_json::Value,
        _: std::sync::Arc<crate::RunCancellation>,
    ) -> Result<String, String> {
        Ok(json!({"candidates":[{"kind":"constraint","semantic_key":"送信","value":"送信しない","status":"active","task_request":null,"replaces":null}],"no_change":false}).to_string())
    }
    fn provenance(&self) -> saaa_personal_state_core::Provenance {
        worker::UnavailableExtractor.provenance()
    }
}
#[tokio::test]
async fn worker_persists_patch_coverage_and_result_in_one_commit_then_forget_erases() {
    let c = db();
    insert(&c, "s1", "送信しない");
    let writer = SqliteWriter::from_connection(c);
    assert!(worker::tick_isolated(&writer, &Fixture, true)
        .await
        .unwrap());
    writer
        .read_serialized(|c| {
            assert_eq!(store::load(c)?.assertions.len(), 1);
            assert_eq!(
                c.query_row("SELECT status FROM personal_jobs", [], |r| r
                    .get::<_, String>(0))
                    .unwrap(),
                "completed"
            );
            Ok(())
        })
        .unwrap();
    writer
        .write(|c| {
            c.execute("DELETE FROM conversation_messages WHERE id='s1'", [])
                .unwrap();
            assert_eq!(
                c.query_row("SELECT count(*) FROM personal_payloads", [], |r| r
                    .get::<_, u64>(0))
                    .unwrap(),
                0
            );
            assert!(store::load(c)?.assertions.is_empty());
            Ok(())
        })
        .unwrap();
}

#[test]
fn derived_answers_and_their_descendants_are_erased_with_the_original() {
    let c = db();
    insert(&c, "s1", "private source");
    let mut m = manifest(&c);
    generation::prepare(&c, &m).unwrap();
    generation::dispatch(&c, &m.generation_id).unwrap();
    generation::finish(&c, &m.generation_id, true).unwrap();
    insert(&c, "answer1", "derived answer");
    c.execute("INSERT INTO personal_artifacts VALUES('g1','answer1')", [])
        .unwrap();
    m.generation_id = "g2".into();
    m.attempt_id = "attempt2".into();
    m.run_id = "run2".into();
    m.view_id = Some("view2".into());
    m.input_epoch = store::load(&c).unwrap().input_epoch;
    m.sources = vec![sources::load(&c, 2, 0, 4096).unwrap().source];
    generation::prepare(&c, &m).unwrap();
    insert(&c, "answer2", "derived derivative");
    c.execute("INSERT INTO personal_artifacts VALUES('g2','answer2')", [])
        .unwrap();
    c.execute("DELETE FROM conversation_messages WHERE id='s1'", [])
        .unwrap();
    assert_eq!(
        c.query_row(
            "SELECT count(*) FROM conversation_messages WHERE id IN ('s1','answer1','answer2')",
            [],
            |r| r.get::<_, u64>(0)
        )
        .unwrap(),
        0
    );
    assert!(generation::allow_run(&c, "run2").is_err());
}

#[test]
fn expired_views_and_unconfirmed_cancellation_prevent_new_claims() {
    let c = db();
    insert(&c, "s1", "source");
    let mut m = manifest(&c);
    m.expires_at = now() - 1;
    generation::prepare(&c, &m).unwrap();
    assert!(generation::dispatch(&c, &m.generation_id).is_err());
    c.execute(
        "UPDATE personal_generations SET cancellation='sent-unconfirmed'",
        [],
    )
    .unwrap();
    assert!(jobs::claim(&c, now(), true).unwrap().is_none());
}
#[test]
fn composer_requires_complete_pending_inputs_and_never_silently_drops_them() {
    let c = db();
    insert(&c, "s1", "送信は禁止");
    assert!(projection::compose(&c, None, 8).is_err());
    let value = projection::compose(&c, None, 4096).unwrap();
    assert!(value.to_string().contains("送信は禁止"));
    assert_eq!(value["instructionAuthority"], "none");
}

#[tokio::test]
async fn partial_messages_stay_candidate_until_full_message_finalization() {
    let c = db();
    insert(
        &c,
        "s1",
        &format!("{}末尾で訂正、送信しない", "あ".repeat(11000)),
    );
    let writer = SqliteWriter::from_connection(c);
    assert!(worker::tick_isolated(&writer, &Fixture, true)
        .await
        .unwrap());
    writer
        .read_serialized(|c| {
            let l = store::load(c)?;
            assert!(l
                .assertions
                .keys()
                .all(|id| l.status(id, now()) == saaa_personal_state_core::Status::Candidate));
            assert_eq!(
                c.query_row("SELECT status FROM personal_jobs", [], |r| r
                    .get::<_, String>(0))
                    .unwrap(),
                "queued"
            );
            Ok(())
        })
        .unwrap();
    assert!(worker::tick_isolated(&writer, &Fixture, true)
        .await
        .unwrap());
    assert!(worker::tick_isolated(&writer, &Fixture, true)
        .await
        .unwrap());
    writer
        .read_serialized(|c| {
            let l = store::load(c)?;
            assert_eq!(
                l.assertions
                    .keys()
                    .filter(|id| l.status(id, now()) == saaa_personal_state_core::Status::Active)
                    .count(),
                1
            );
            assert_eq!(
                c.query_row("SELECT status FROM personal_jobs", [], |r| r
                    .get::<_, String>(0))
                    .unwrap(),
                "completed"
            );
            Ok(())
        })
        .unwrap();
}

#[test]
fn real_db_backup_restore_merges_current_journal_and_missing_journal_blocks_open() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.sqlite3");
    let backup = directory.path().join("old.sqlite3");
    let writer = SqliteWriter::open(&path).unwrap();
    writer
        .write(|c| {
            insert(c, "s1", "must not return");
            Ok(())
        })
        .unwrap();
    drop(writer);
    std::fs::copy(&path, &backup).unwrap();
    let writer = SqliteWriter::open(&path).unwrap();
    writer
        .write(|c| {
            c.execute("DELETE FROM conversation_messages WHERE id='s1'", [])
                .map_err(crate::database_error)?;
            Ok(())
        })
        .unwrap();
    drop(writer);
    std::fs::copy(&backup, &path).unwrap();
    let writer = SqliteWriter::open(&path).unwrap();
    writer
        .read_serialized(|c| {
            assert_eq!(
                c.query_row(
                    "SELECT count(*) FROM conversation_messages WHERE id='s1'",
                    [],
                    |r| r.get::<_, u64>(0)
                )
                .unwrap(),
                0
            );
            Ok(())
        })
        .unwrap();
    drop(writer);
    std::fs::remove_file(journal::path(&path)).unwrap();
    assert!(SqliteWriter::open(&path).is_err());
}

struct ScopedFixture;
#[async_trait::async_trait]
impl worker::Extractor for ScopedFixture {
    async fn extract(
        &self,
        input: serde_json::Value,
        _: std::sync::Arc<crate::RunCancellation>,
    ) -> Result<String, String> {
        Ok(json!({"candidates":[{"kind":"constraint","semantic_key":"language","value":"日本語","status":"active","task_request":input["request_scope"],"replaces":null}],"no_change":false}).to_string())
    }
    fn provenance(&self) -> saaa_personal_state_core::Provenance {
        <Fixture as worker::Extractor>::provenance(&Fixture)
    }
}
#[tokio::test]
async fn request_local_extraction_never_appears_in_another_request_or_shared_projection() {
    let c = db();
    insert(&c, "local-request", "この依頼だけ日本語で");
    let writer = SqliteWriter::from_connection(c);
    assert!(worker::tick_isolated(&writer, &ScopedFixture, true)
        .await
        .unwrap());
    writer
        .read_serialized(|c| {
            let own = projection::compose(c, Some("local-request"), 262144)?;
            assert_eq!(own["items"].as_array().unwrap().len(), 1);
            assert!(projection::compose(c, None, 262144)?["items"]
                .as_array()
                .unwrap()
                .is_empty());
            assert!(projection::compose(c, Some("other"), 262144)?["items"]
                .as_array()
                .unwrap()
                .is_empty());
            Ok(())
        })
        .unwrap();
}

#[test]
fn task_bundle_rolls_back_with_answer_and_rejects_evidence_outside_manifest() {
    let c = db();
    insert(&c, "s1", "この依頼だけ日本語で");
    let m = manifest(&c);
    generation::prepare(&c, &m).unwrap();
    generation::dispatch(&c, &m.generation_id).unwrap();
    let candidate = || task_bundle::SupportedCandidate {
        candidate: worker::Candidate {
            kind: saaa_personal_state_core::Kind::Constraint,
            semantic_key: "language".into(),
            value: json!("日本語"),
            status: saaa_personal_state_core::Status::Active,
            task_request: Some("s1".into()),
            replaces: None,
        },
        evidence: std::collections::BTreeSet::from([m.sources[0].key.clone()]),
    };
    {
        let tx = c.unchecked_transaction().unwrap();
        task_bundle::adopt(
            &tx,
            &m,
            vec![candidate()],
            <Fixture as worker::Extractor>::provenance(&Fixture),
        )
        .unwrap();
        assert_eq!(store::load(&tx).unwrap().assertions.len(), 1);
        // Simulates failure to persist the accompanying answer/run completion.
    }
    assert!(store::load(&c).unwrap().assertions.is_empty());
    let mut bad = candidate();
    bad.evidence = std::collections::BTreeSet::from([saaa_personal_state_core::SourceKey {
        id: "unexposed".into(),
        version: 1,
        start: 0,
        end: 3,
    }]);
    {
        let tx = c.unchecked_transaction().unwrap();
        assert!(task_bundle::adopt(
            &tx,
            &m,
            vec![bad],
            <Fixture as worker::Extractor>::provenance(&Fixture)
        )
        .is_err());
    }
    let tx = c.unchecked_transaction().unwrap();
    task_bundle::adopt(
        &tx,
        &m,
        vec![candidate()],
        <Fixture as worker::Extractor>::provenance(&Fixture),
    )
    .unwrap();
    tx.commit().unwrap();
    assert_eq!(store::load(&c).unwrap().assertions.len(), 1);
    assert!(store::load(&c).unwrap().coverage.is_empty());
    c.execute("DELETE FROM conversation_messages WHERE id='s1'", [])
        .unwrap();
    assert!(store::load(&c).unwrap().assertions.is_empty());
}
