#![cfg(test)]
use super::super::worker::{self, UnavailableExtractor};
use super::super::world::test_support::{insert_source, writer_db, PROJECT};
use super::*;
use saaa_personal_state_core::Provenance;
use std::sync::atomic::{AtomicUsize, Ordering};
struct Fixture {
    calls: AtomicUsize,
    change: bool,
}
#[async_trait::async_trait]
impl Extractor for Fixture {
    async fn extract(&self, _: Value, _: Arc<RunCancellation>) -> Result<String, String> {
        Ok("{\"candidates\":[],\"no_change\":true}".into())
    }
    async fn extract_world(
        &self,
        input: Value,
        _: Arc<RunCancellation>,
    ) -> Result<Option<String>, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let text = input["source"]["text"].as_str().unwrap();
        Ok(Some(if self.change {
            json!({"candidates":[{"kind":"world_entity","payload":{"type":"entity","schema_version":2,"entity_id":"cache","entity_kind":"concept","name":"キャッシュ","aliases":[],"objective_assertion_id":null},"quote":text,"quote_start":0,"quote_end":text.len(),"epistemic":"user_reported","replaces":null}],"outcomes":[],"no_change":false}).to_string()
        } else {
            json!({"candidates":[],"outcomes":[],"no_change":true}).to_string()
        }))
    }
    fn provenance(&self) -> Provenance {
        UnavailableExtractor.provenance()
    }
}
fn fixture(n: usize) -> SqliteWriter {
    let w = writer_db();
    w.transact(|c| {
        for i in 0..n {
            insert_source(c, PROJECT, &format!("review-{i}"), "キャッシュの条件");
        }
        c.execute("UPDATE personal_jobs SET status='completed'", [])
            .unwrap();
        c.execute("UPDATE personal_scope SET last_foreground_at=0", [])
            .unwrap();
        jobs::refill_reviews(c)?;
        c.execute("DELETE FROM personal_review_work WHERE lane='current'", [])
            .unwrap();
        Ok(())
    })
    .unwrap();
    w
}
#[tokio::test]
async fn retrospective_preview_then_apply_reuses_proposal_without_inference() {
    let w = fixture(1);
    let e = Fixture {
        calls: AtomicUsize::new(0),
        change: true,
    };
    assert!(worker::tick_isolated(&w, &e, true).await.unwrap());
    assert_eq!(e.calls.load(Ordering::SeqCst), 1);
    w.read_serialized(|c| {
        assert!(store::load(c)?.assertions.is_empty());
        assert_eq!(status(c)?["stages"][0]["stage"], "preview");
        assert_eq!(
            projection::candidates(c)?[0]["sources"][0]["id"],
            "review-0"
        );
        Ok(())
    })
    .unwrap();
    w.transact(|c| set_mode(c, "apply")).unwrap();
    assert!(worker::tick_isolated(&w, &e, true).await.unwrap());
    assert_eq!(e.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        w.read_serialized(|c| Ok(store::load(c)?.assertions.len()))
            .unwrap(),
        1
    );
    assert!(!worker::tick_isolated(&w, &e, true).await.unwrap());
}
#[tokio::test]
async fn retrospective_empty_and_unchanged_work_make_no_extra_model_calls() {
    let w = fixture(1);
    let e = Fixture {
        calls: AtomicUsize::new(0),
        change: false,
    };
    assert!(worker::tick_isolated(&w, &e, true).await.unwrap());
    w.transact(|c| {
        jobs::refill_reviews(c)?;
        c.execute("DELETE FROM personal_review_work WHERE lane='current'", [])
            .unwrap();
        Ok(())
    })
    .unwrap();
    assert!(!worker::tick_isolated(&w, &e, true).await.unwrap());
    assert_eq!(e.calls.load(Ordering::SeqCst), 1);
}
#[test]
fn retrospective_old_owner_fenced_and_forget_erases_proposal() {
    let w = fixture(1);
    w.transact(|c| {
        let old = jobs::claim_review(c, 100000, true)?.unwrap();
        jobs::recover_reviews(c, 160001)?;
        assert!(!jobs::valid_review(c, &old, 160001)?);
        let new = jobs::claim_review(c, 160001, true)?.unwrap();
        assert!(new.generation > old.generation);
        c.execute(
            "INSERT OR IGNORE INTO personal_review_inputs VALUES(?1,'review-0',1)",
            [new.id],
        )
        .unwrap();
        c.execute(
            "UPDATE personal_review_work SET proposal='secret',manifest='secret'",
            [],
        )
        .unwrap();
        c.execute("DELETE FROM conversation_messages WHERE id='review-0'", [])
            .unwrap();
        assert_eq!(
            c.query_row("SELECT count(*) FROM personal_review_work", [], |r| r
                .get::<_, u64>(0))
                .unwrap(),
            0
        );
        insert_source(c, PROJECT, "replacement-source", "別の根拠");
        c.execute("UPDATE personal_jobs SET status='completed'", [])
            .unwrap();
        jobs::refill_reviews(c)?;
        let replacement: u64 = c
            .query_row("SELECT min(id) FROM personal_review_work", [], |r| r.get(0))
            .unwrap();
        assert!(replacement > new.id);
        assert!(!jobs::valid_review(c, &old, 160002)?);
        Ok(())
    })
    .unwrap();
}
#[tokio::test]
async fn retrospective_later_correction_never_activates_old_claim() {
    let w = fixture(5);
    let e = Fixture {
        calls: AtomicUsize::new(0),
        change: true,
    };
    w.transact(|c| set_mode(c, "apply")).unwrap();
    assert!(worker::tick_isolated(&w, &e, true).await.unwrap());
    assert!(w
        .read_serialized(|c| Ok(store::load(c)?.assertions.is_empty()))
        .unwrap());
    assert_eq!(
        w.read_serialized(status).unwrap()["stages"][0]["reason"],
        "historical-currentness-unverified"
    );
}
#[test]
fn retrospective_full_queue_keeps_source_backlog_and_scope_isolation() {
    let w = fixture(1);
    w.transact(|c| {
        c.execute("DELETE FROM personal_review_work", []).unwrap();
        c.execute("DELETE FROM personal_source_scope_refs", [])
            .unwrap();
        jobs::refill_reviews(c)?;
        assert_eq!(
            c.query_row("SELECT count(*) FROM personal_review_work", [], |r| r
                .get::<_, u64>(0))
                .unwrap(),
            0
        );
        Ok(())
    })
    .unwrap();
}

#[tokio::test]
async fn retrospective_disabled_and_foreground_never_infer() {
    let w = fixture(1);
    let e = Fixture {
        calls: AtomicUsize::new(0),
        change: true,
    };
    assert!(!worker::tick_isolated(&w, &e, false).await.unwrap());
    w.transact(|c| {
        c.execute(
            "UPDATE personal_scope SET last_foreground_at=?1",
            [super::super::now()],
        )
        .unwrap();
        Ok(())
    })
    .unwrap();
    assert!(!worker::tick_isolated(&w, &e, true).await.unwrap());
    assert_eq!(e.calls.load(Ordering::SeqCst), 0);
}
#[test]
fn retrospective_edit_of_queued_input_refills_the_new_version() {
    let w = fixture(1);
    w.transact(|c| {
        c.execute(
            "UPDATE conversation_messages SET content='訂正後の条件' WHERE id='review-0'",
            [],
        )
        .unwrap();
        assert_eq!(
            c.query_row("SELECT count(*) FROM personal_review_work", [], |r| r
                .get::<_, u64>(0))
                .unwrap(),
            0
        );
        c.execute(
            "INSERT OR IGNORE INTO personal_source_scope_refs VALUES('review-0',2,?1)",
            [PROJECT],
        )
        .unwrap();
        jobs::refill_reviews(c)?;
        let seq: u64 = c
            .query_row("SELECT sequence FROM personal_review_work", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(seq, 2);
        Ok(())
    })
    .unwrap();
}
#[tokio::test]
async fn retrospective_window_budget_holds_without_model_call() {
    let w = fixture(1);
    let e = Fixture {
        calls: AtomicUsize::new(0),
        change: true,
    };
    w.transact(|c| {
        c.execute(
            "UPDATE conversation_messages SET content=?1 WHERE id='review-0'",
            ["長".repeat(11000)],
        )
        .unwrap();
        c.execute("UPDATE personal_jobs SET status='completed'", [])
            .unwrap();
        c.execute("UPDATE personal_scope SET last_foreground_at=0", [])
            .unwrap();
        c.execute(
            "INSERT OR IGNORE INTO personal_source_scope_refs VALUES('review-0',2,?1)",
            [PROJECT],
        )
        .unwrap();
        jobs::refill_reviews(c)
    })
    .unwrap();
    assert!(worker::tick_isolated(&w, &e, true).await.unwrap());
    assert_eq!(e.calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        w.read_serialized(status).unwrap()["stages"][0]["reason"],
        "evidence-window-incomplete"
    );
}
struct ForgetDuringInference {
    writer: Arc<SqliteWriter>,
}
#[async_trait::async_trait]
impl Extractor for ForgetDuringInference {
    async fn extract(&self, _: Value, _: Arc<RunCancellation>) -> Result<String, String> {
        unreachable!()
    }
    async fn extract_world(
        &self,
        _: Value,
        _: Arc<RunCancellation>,
    ) -> Result<Option<String>, String> {
        // A Writer transaction succeeds inside the inference await: no DB lock is held.
        self.writer.transact(|c| {
            c.execute("DELETE FROM conversation_messages WHERE id='review-0'", [])
                .unwrap();
            Ok(())
        })?;
        Ok(Some(
            json!({"candidates":[],"outcomes":[],"no_change":true}).to_string(),
        ))
    }
    fn provenance(&self) -> Provenance {
        UnavailableExtractor.provenance()
    }
}
#[tokio::test]
async fn retrospective_forget_during_inference_discards_late_response_without_db_lock() {
    let w = Arc::new(fixture(1));
    let e = ForgetDuringInference { writer: w.clone() };
    assert!(worker::tick_isolated(&w, &e, true).await.is_err());
    assert_eq!(
        w.read_serialized(|c| Ok(c
            .query_row("SELECT count(*) FROM personal_review_work", [], |r| r
                .get::<_, u64>(0))
            .unwrap()))
            .unwrap(),
        0
    );
}
#[test]
fn retrospective_measured_context_includes_output_and_safety_reserve() {
    assert!(token_budget("world-review-id", 60000, 2000, 2000).is_ok());
    assert!(token_budget("world-review-id", 60001, 2000, 2000).is_err());
    assert!(token_budget("world-review-id", u64::MAX, 2000, 2000).is_err());
    assert!(token_budget("extract-id", 100000, 2000, 2000).is_ok());
}
#[test]
fn retrospective_and_ordinary_jobs_share_capacity_without_losing_sources() {
    let w = fixture(1);
    w.transact(|c| {
        for i in 0..1025 {
            c.execute("INSERT INTO conversation_messages(id,conversation_id,role,content,created_at) VALUES(?1,?2,'user','bounded','1')",params![format!("capacity-{i}"),crate::PRIMARY_CONVERSATION_ID]).unwrap();
        }
        let (jobs,reviews):(u64,u64)=c.query_row("SELECT (SELECT count(*) FROM personal_jobs WHERE status IN ('queued','running')),(SELECT count(*) FROM personal_review_work WHERE status IN ('queued','running'))",[],|r|Ok((r.get(0)?,r.get(1)?))).unwrap();
        assert_eq!(jobs+reviews,1024);
        let sources:u64=c.query_row("SELECT count(*) FROM personal_sources",[],|r|r.get(0)).unwrap();
        assert_eq!(sources,1026);
        c.execute("UPDATE personal_jobs SET status='completed'",[]).unwrap();
        jobs::refill(c)?;
        assert_eq!(c.query_row("SELECT count(*) FROM personal_jobs",[],|r|r.get::<_,u64>(0)).unwrap(),sources);
        Ok(())
    }).unwrap();
}
#[tokio::test]
async fn retrospective_policy_change_requires_reauthorization_without_resetting_old_jobs() {
    let w = fixture(1);
    let e = Fixture {
        calls: AtomicUsize::new(0),
        change: false,
    };
    assert!(worker::tick_isolated(&w, &e, true).await.unwrap());
    w.transact(|c| {
        c.execute("UPDATE personal_scope SET policy_revision=2", [])
            .unwrap();
        jobs::refill_reviews(c)?;
        c.execute("DELETE FROM personal_review_work WHERE lane='current'", [])
            .unwrap();
        assert_eq!(
            c.query_row("SELECT status FROM personal_jobs", [], |r| r
                .get::<_, String>(0))
                .unwrap(),
            "completed"
        );
        Ok(())
    })
    .unwrap();
    assert!(worker::tick_isolated(&w, &e, true).await.unwrap());
    assert_eq!(e.calls.load(Ordering::SeqCst), 1);
    assert!(w.read_serialized(status).unwrap()["stages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|s| s["reason"] == "policy-reauthorization-required"));
    w.transact(|c| {
        jobs::refill_reviews(c)?;
        c.execute("DELETE FROM personal_review_work WHERE lane='current'", [])
            .unwrap();
        Ok(())
    })
    .unwrap();
    assert!(!worker::tick_isolated(&w, &e, true).await.unwrap());
}
#[tokio::test]
async fn retrospective_new_evidence_does_not_skip_unprocessed_history() {
    let w = fixture(5);
    let e = Fixture {
        calls: AtomicUsize::new(0),
        change: true,
    };
    w.transact(|c| {
        set_mode(c, "apply")?;
        jobs::refill_reviews(c)
    })
    .unwrap();
    assert!(worker::tick_isolated(&w, &e, true).await.unwrap());
    w.read_serialized(|c| {
        assert_eq!(
            c.query_row("SELECT count(*) FROM personal_review_cursor", [], |r| r
                .get::<_, u64>(0))
                .unwrap(),
            0
        );
        assert_eq!(store::load(c)?.assertions.len(), 1);
        Ok(())
    })
    .unwrap();
    assert!(worker::tick_isolated(&w, &e, true).await.unwrap());
    w.read_serialized(|c| {
        assert_eq!(
            c.query_row("SELECT sequence FROM personal_review_cursor", [], |r| r
                .get::<_, u64>(0))
                .unwrap(),
            4
        );
        Ok(())
    })
    .unwrap();
    w.transact(jobs::refill_reviews).unwrap();
    assert!(worker::tick_isolated(&w, &e, true).await.unwrap());
    w.transact(jobs::refill_reviews).unwrap();
    assert!(!worker::tick_isolated(&w, &e, true).await.unwrap());
    assert_eq!(e.calls.load(Ordering::SeqCst), 2);
}
