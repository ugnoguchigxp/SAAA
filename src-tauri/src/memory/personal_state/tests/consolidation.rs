use super::memory_contract::{bind, grant};
use super::*;
use serde_json::Value;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
struct ObservationExtractor {
    writer: Arc<SqliteWriter>,
    calls: AtomicUsize,
    disable_on_consolidation: bool,
}
#[async_trait::async_trait]
impl worker::Extractor for ObservationExtractor {
    async fn extract(
        &self,
        input: Value,
        _: Arc<crate::RunCancellation>,
    ) -> Result<String, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.writer.read_serialized(|c| {
            assert!(
                c.is_autocommit(),
                "model wait must not hold a writer transaction"
            );
            Ok(())
        })?;
        let consolidating = input["instruction"] == admission::CONSOLIDATION_INSTRUCTION;
        if consolidating && self.disable_on_consolidation {
            self.writer.transact(|c| {
                c.execute("UPDATE personal_consolidation_settings SET enabled=0", [])
                    .map_err(crate::database_error)?;
                Ok(())
            })?;
        }
        Ok(json!({"candidates":[{"kind":"observation","semantic_key":"食べ物","value":if consolidating{"甘いものに関する未確定の傾向"}else{"食べ物についての本人発話"},"status":"candidate","task_request":input["request_scope"],"replaces":null,"support":{"basis":if consolidating{"inferred"}else{"explicit"},"quote":if consolidating{Value::Null}else{input["source"]["text"].clone()}}}],"no_change":false}).to_string())
    }
    fn provenance(&self) -> saaa_personal_state_core::Provenance {
        worker::UnavailableExtractor.provenance()
    }
}
async fn exercise(disable: bool) {
    let c = db();
    let key = grant(&c);
    c.execute("UPDATE personal_consolidation_settings SET enabled=1", [])
        .unwrap();
    insert(&c, "one", "私は甘いものが好きです。");
    bind(&c, "one", &key);
    let writer = Arc::new(SqliteWriter::from_connection(c));
    let extractor = ObservationExtractor {
        writer: writer.clone(),
        calls: AtomicUsize::new(0),
        disable_on_consolidation: disable,
    };
    worker::tick_isolated(&writer, &extractor, true)
        .await
        .unwrap();
    writer
        .read_serialized(|c| {
            assert_eq!(
                c.query_row("SELECT stage FROM personal_jobs", [], |r| r
                    .get::<_, String>(0))
                    .unwrap(),
                "consolidation"
            );
            assert_eq!(
                c.query_row("SELECT status FROM personal_jobs", [], |r| r
                    .get::<_, String>(0))
                    .unwrap(),
                "queued"
            );
            Ok(())
        })
        .unwrap();
    // Restart/next tick resumes the persisted stage. One origin is not enough for an LLM call.
    worker::tick_isolated(&writer, &extractor, true)
        .await
        .unwrap();
    assert_eq!(extractor.calls.load(Ordering::SeqCst), 1);
    writer
        .read_serialized(|c| {
            insert(c, "two", "今日もケーキを選びました。");
            bind(c, "two", &key);
            Ok(())
        })
        .unwrap();
    worker::tick_isolated(&writer, &extractor, true)
        .await
        .unwrap();
    worker::tick_isolated(&writer, &extractor, true)
        .await
        .unwrap();
    assert_eq!(extractor.calls.load(Ordering::SeqCst), 3);
    writer
        .read_serialized(|c| {
            let l = store::load(c)?;
            assert!(l
                .assertions
                .values()
                .all(|a| a.kind == saaa_personal_state_core::Kind::Observation
                    && l.status(&a.id, now()) == saaa_personal_state_core::Status::Candidate));
            let merged: Vec<_> = l
                .assertions
                .values()
                .filter(|a| a.semantic_key.starts_with("consolidated:"))
                .collect();
            assert_eq!(merged.len(), usize::from(!disable));
            if let Some(a) = merged.first() {
                let raw: String = c
                    .query_row(
                        "SELECT value_json FROM personal_payloads WHERE id=?1",
                        [&a.payload_ref],
                        |r| r.get(0),
                    )
                    .unwrap();
                let v: Value = serde_json::from_str(&raw).unwrap();
                assert_eq!(v["independentOrigins"].as_array().unwrap().len(), 2);
            }
            Ok(())
        })
        .unwrap();
}
#[tokio::test]
async fn durable_consolidation_requires_two_origins_and_remains_candidate() {
    exercise(false).await;
}
#[tokio::test]
async fn disabling_consolidation_during_generation_discards_the_proposal() {
    exercise(true).await;
}
