use super::*;
use saaa_personal_state_core::{Kind, Status};
use serde_json::Value;
use sha2::{Digest, Sha256};

fn grant(c: &Connection) -> String {
    let p: String = c
        .query_row("SELECT principal FROM personal_scope", [], |r| r.get(0))
        .unwrap();
    let key = format!("user:{p}");
    super::super::world::test_support::ensure_scope(c, &key);
    c.execute(
        "INSERT INTO memory_episode_grants(scope_key,enabled) VALUES(?1,1)",
        [&key],
    )
    .unwrap();
    key
}
fn result(c: &Connection, key: &str) -> String {
    let (version,sequence,policy,text,epoch,access):(u64,u64,u64,String,u64,u64)=c.query_row("SELECT version,sequence,policy_revision,content,scope_epoch,access_revision FROM memory_episode_source_v1 WHERE source_id='origin' AND scope_key=?1",[key],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?))).unwrap();
    let principal: String = c
        .query_row("SELECT principal FROM personal_scope", [], |r| r.get(0))
        .unwrap();
    json!({"items":[{"id":"episode","sourceKey":"immutable","sourceContract":{"contractVersion":1,"principal":principal,"sources":[{"sourceId":"origin","version":version,"sequence":sequence,"policyRevision":policy,"scopeEpoch":epoch,"accessRevision":access,"scope":key,"start":0,"end":text.len(),"byteLength":text.len(),"digest":format!("{:x}",Sha256::digest(text.as_bytes()))}]}}]}).to_string()
}

#[test]
fn export_requires_grant_and_current_scope_epoch_and_response_cannot_survive_forgetting() {
    let c = db();
    insert(&c, "origin", "2024年に京都へ行った。");
    assert_eq!(
        c.query_row("SELECT count(*) FROM memory_episode_source_v1", [], |r| r
            .get::<_, u32>(
            0
        ))
        .unwrap(),
        0
    );
    let key = grant(&c);
    let payload = result(&c, &key);
    assert!(episode_export::capture(&c, "run", &payload, &["project:other".into()]).is_err());
    episode_export::capture(&c, "run", &payload, &[key.clone()]).unwrap();
    episode_export::validate_run(&c, "run").unwrap();
    c.execute(
        "UPDATE context_scope_epochs SET epoch=epoch+1 WHERE scope_key=?1",
        [&key],
    )
    .unwrap();
    // Ordinary input watermarks do not revoke otherwise live Episode evidence.
    episode_export::validate_run(&c, "run").unwrap();
    c.execute(
        "INSERT INTO memory_episode_scope_policies(scope_key,revision) VALUES(?1,2)",
        [&key],
    )
    .unwrap();
    assert!(episode_export::validate_run(&c, "run").is_err());
    episode_export::capture(&c, "run", &result(&c, &key), &[key.clone()]).unwrap();
    insert(&c, "answer", "京都についての派生回答");
    c.execute(
        "INSERT INTO memory_episode_artifacts VALUES('run','answer')",
        [],
    )
    .unwrap();
    c.execute("DELETE FROM conversation_messages WHERE id='origin'", [])
        .unwrap();
    assert!(episode_export::validate_run(&c, "run").is_err());
    assert_eq!(
        c.query_row(
            "SELECT count(*) FROM conversation_messages WHERE id='answer'",
            [],
            |r| r.get::<_, u32>(0)
        )
        .unwrap(),
        0
    );
}

#[test]
fn revoked_grant_or_scope_and_edited_source_invalidate_episode_immediately() {
    let c = db();
    insert(&c, "origin", "甘いものが好きです。");
    let key = grant(&c);
    let payload = result(&c, &key);
    c.execute(
        "UPDATE memory_episode_grants SET enabled=0 WHERE scope_key=?1",
        [&key],
    )
    .unwrap();
    assert!(episode_export::capture(&c, "run", &payload, &[key.clone()]).is_err());
    c.execute(
        "UPDATE memory_episode_grants SET enabled=1,revision=revision+1 WHERE scope_key=?1",
        [&key],
    )
    .unwrap();
    let payload = result(&c, &key);
    c.execute(
        "UPDATE conversation_messages SET content='辛いものが好きです。' WHERE id='origin'",
        [],
    )
    .unwrap();
    assert!(episode_export::capture(&c, "run", &payload, &[key.clone()]).is_err());
    c.execute(
        "UPDATE context_scopes SET state='revoked' WHERE scope_key=?1",
        [&key],
    )
    .unwrap();
    assert_eq!(
        c.query_row("SELECT count(*) FROM memory_episode_source_v1", [], |r| r
            .get::<_, u32>(
            0
        ))
        .unwrap(),
        0
    );
}

#[test]
fn inferred_or_assistant_profile_and_quoted_preferences_remain_uncertain() {
    let c = db();
    insert(&c, "origin", "私は甘いものが好きです。");
    let mut source = sources::load(&c, 1, 0, 4096).unwrap().source;
    for basis in ["explicit", "inferred", "quoted", "hypothetical"] {
        let mut candidate:worker::Candidate=serde_json::from_value(json!({"kind":"preference","semantic_key":"食べ物","value":"甘いもの","status":"active","task_request":null,"replaces":null,"support":{"basis":basis,"quote":"甘いものが好きです。"}})).unwrap();
        admission::gate(&mut candidate, &source, "私は甘いものが好きです。").unwrap();
        assert_eq!(
            candidate.status,
            if basis == "explicit" {
                Status::Active
            } else {
                Status::Candidate
            }
        );
        candidate.kind = Kind::Observation;
        candidate.status = Status::Active;
        admission::gate(&mut candidate, &source, "私は甘いものが好きです。").unwrap();
        assert_eq!(candidate.status, Status::Candidate);
    }
    source.role = saaa_personal_state_core::SourceRole::Assistant;
    let mut candidate:worker::Candidate=serde_json::from_value(json!({"kind":"habit","semantic_key":"食べ物","value":"甘いもの","status":"active","task_request":null,"replaces":null,"support":{"basis":"explicit","quote":"甘いものが好きです。"}})).unwrap();
    admission::gate(&mut candidate, &source, "私は甘いものが好きです。").unwrap();
    assert_eq!(candidate.status, Status::Candidate);
}

#[test]
fn isolated_export_fixture_for_contextstill_uses_the_production_schema() {
    let Some(path) = std::env::var_os("SAAA_EPISODE_FIXTURE_PATH") else {
        return;
    };
    let path = std::path::PathBuf::from(path);
    assert!(
        path.is_absolute() && !path.exists(),
        "fixture must be a new isolated database"
    );
    let c = Connection::open(path).unwrap();
    crate::initialize_database(&c).unwrap();
    insert(&c, "origin", "2024年に京都へ行った。雨で散歩は中止した。");
    insert(&c, "confirmation", "それでいい。次は博物館にしましょう。");
    grant(&c);
    assert_eq!(
        c.query_row("SELECT count(*) FROM memory_episode_source_v1", [], |r| r
            .get::<_, u32>(
            0
        ))
        .unwrap(),
        2
    );
}

struct ProfileExtractor;
#[async_trait::async_trait]
impl worker::Extractor for ProfileExtractor {
    async fn extract(
        &self,
        input: serde_json::Value,
        _: std::sync::Arc<crate::RunCancellation>,
    ) -> Result<String, String> {
        let text = input["source"]["text"].as_str().unwrap();
        let previous = input["current"]
            .as_array()
            .and_then(|items| items.iter().find(|a| a["kind"] == "preference"))
            .map(|a| a["id"].clone())
            .unwrap_or(serde_json::Value::Null);
        Ok(json!({"candidates":[{"kind":"preference","semantic_key":"食べ物","value":text,"status":"active","task_request":input["request_scope"],"replaces":previous,"support":{"basis":"explicit","quote":text}}],"no_change":false}).to_string())
    }
    fn provenance(&self) -> saaa_personal_state_core::Provenance {
        worker::UnavailableExtractor.provenance()
    }
}
fn scope_snapshot(c: &Connection, key: &str) -> crate::runtime::context::scope::ScopeSnapshot {
    crate::runtime::context::scope::ScopeSnapshot {
        status: "resolved".into(),
        focus_scope_key: Some(key.into()),
        digest: "fixture".into(),
        reason_code: None,
        scopes: vec![crate::runtime::context::scope::ResolvedScope {
            key: key.into(),
            kind: "user".into(),
            relation: "focus".into(),
            epoch: c
                .query_row(
                    "SELECT epoch FROM context_scope_epochs WHERE scope_key=?1",
                    [key],
                    |r| r.get(0),
                )
                .unwrap(),
        }],
    }
}
fn bind(c: &Connection, id: &str, key: &str) {
    c.execute(
        "INSERT INTO personal_source_scope_refs VALUES(?1,1,?2)",
        params![id, key],
    )
    .unwrap();
    c.execute("UPDATE personal_jobs SET scope_key=?2 WHERE source_sequence=(SELECT sequence FROM personal_sources WHERE message_id=?1 AND available=1)",params![id,key]).unwrap();
}
#[tokio::test]
async fn published_snapshot_is_stable_and_pending_correction_and_forget_disable_old_truth() {
    let c = db();
    let key = grant(&c);
    insert(&c, "origin", "甘いものが好きです。");
    bind(&c, "origin", &key);
    let writer = SqliteWriter::from_connection(c);
    worker::tick_isolated(&writer, &ProfileExtractor, true)
        .await
        .unwrap();
    let old_episode = writer.read_serialized(|c| Ok(result(c, &key))).unwrap();
    let initial = writer
        .read_serialized(|c| {
            snapshots::publish(c, now())?;
            let first = snapshots::read(c, &scope_snapshot(c, &key), "question")?;
            snapshots::publish(c, now() + 1)?;
            assert_eq!(
                first,
                snapshots::read(c, &scope_snapshot(c, &key), "question")?
            );
            assert_eq!(
                c.query_row("SELECT revision FROM personal_snapshots", [], |r| r
                    .get::<_, u32>(0))
                    .unwrap(),
                1
            );
            assert!(first.join("").contains("甘いもの"));
            insert(c, "correction", "訂正します。辛いものが好きです。");
            bind(c, "correction", &key);
            let pending = snapshots::read(c, &scope_snapshot(c, &key), "question")?;
            assert!(!pending.join("").contains("PERSONAL_MEMORY_SNAPSHOT"));
            assert!(pending.join("").contains("辛いもの"));
            Ok(first)
        })
        .unwrap();
    worker::tick_isolated(&writer, &ProfileExtractor, true)
        .await
        .unwrap();
    writer
        .read_serialized(|c| {
            snapshots::publish(c, now())?;
            let current = snapshots::read(c, &scope_snapshot(c, &key), "question")?;
            assert_ne!(initial, current);
            assert!(episode_export::capture(c, "late-old", &old_episode, &[key.clone()]).is_err());
            assert!(!current.join("").contains("甘いもの"));
            c.execute(
                "DELETE FROM conversation_messages WHERE id='correction'",
                [],
            )
            .unwrap();
            assert!(snapshots::read(c, &scope_snapshot(c, &key), "question")?.is_empty());
            Ok(())
        })
        .unwrap();
}

#[test]
fn copied_quotes_are_one_origin_even_with_distinct_ids() {
    let rows = vec![
        json!({"kind":"preference","value":{"support":{"basis":"explicit","quote":"甘いものが好き"}}}),
        json!({"kind":"preference","value":{"support":{"basis":"explicit","quote":"甘いものが好き"}}}),
    ];
    assert_eq!(admission::independent_origins(&rows).len(), 1);
}

#[test]
fn pending_backlog_preserves_conversation_and_speaker_edit_creates_new_version() {
    let c = db();
    let key = grant(&c);
    for i in 0..40 {
        insert(&c, &format!("pending-{i}"), "未処理の記録です。");
    }
    let pending = snapshots::read(&c, &scope_snapshot(&c, &key), "question").unwrap();
    assert!(pending.join("").contains("PERSONAL_MEMORY_UNAVAILABLE"));
    c.execute(
        "UPDATE conversation_messages SET role='assistant' WHERE id='pending-0'",
        [],
    )
    .unwrap();
    assert_eq!(
        c.query_row(
            "SELECT version FROM personal_sources WHERE message_id='pending-0' AND available=1",
            [],
            |r| r.get::<_, u64>(0)
        )
        .unwrap(),
        2
    );
}

struct FixedProfile {
    value: &'static str,
    replaces: bool,
}
#[async_trait::async_trait]
impl worker::Extractor for FixedProfile {
    async fn extract(
        &self,
        input: Value,
        _: std::sync::Arc<crate::RunCancellation>,
    ) -> Result<String, String> {
        let previous = if self.replaces {
            input["current"]
                .as_array()
                .and_then(|v| v.iter().find(|a| a["kind"] == "preference"))
                .map(|v| v["id"].clone())
                .unwrap_or(Value::Null)
        } else {
            Value::Null
        };
        Ok(json!({"candidates":[{"kind":"preference","semantic_key":"食べ物","value":self.value,"status":"active","task_request":input["request_scope"],"replaces":previous,"support":{"basis":"explicit","quote":input["source"]["text"]}}],"no_change":false}).to_string())
    }
    fn provenance(&self) -> saaa_personal_state_core::Provenance {
        worker::UnavailableExtractor.provenance()
    }
}
#[tokio::test]
async fn same_body_new_evidence_changes_private_stamp_and_ambiguous_update_is_held() {
    let c = db();
    let key = grant(&c);
    insert(&c, "origin", "甘いものが好きです。");
    bind(&c, "origin", &key);
    let writer = SqliteWriter::from_connection(c);
    worker::tick_isolated(
        &writer,
        &FixedProfile {
            value: "甘いもの",
            replaces: true,
        },
        true,
    )
    .await
    .unwrap();
    let (body, stamp) = writer
        .read_serialized(|c| {
            snapshots::publish(c, now())?;
            Ok((
                snapshots::read(c, &scope_snapshot(c, &key), "question")?,
                snapshots::stamp(c, &scope_snapshot(c, &key))?,
            ))
        })
        .unwrap();
    writer
        .read_serialized(|c| {
            insert(c, "confirm", "今も甘いものが好きです。");
            bind(c, "confirm", &key);
            Ok(())
        })
        .unwrap();
    worker::tick_isolated(
        &writer,
        &FixedProfile {
            value: "甘いもの",
            replaces: true,
        },
        true,
    )
    .await
    .unwrap();
    writer
        .read_serialized(|c| {
            snapshots::publish(c, now())?;
            assert_eq!(
                body,
                snapshots::read(c, &scope_snapshot(c, &key), "question")?
            );
            assert_ne!(stamp, snapshots::stamp(c, &scope_snapshot(c, &key))?);
            assert!(
                episode_export::capture(c, "same-body", &result(c, &key), &[key.clone()]).is_ok()
            );
            insert(c, "ambiguous", "辛いものが好きです。");
            bind(c, "ambiguous", &key);
            Ok(())
        })
        .unwrap();
    worker::tick_isolated(
        &writer,
        &FixedProfile {
            value: "辛いもの",
            replaces: false,
        },
        true,
    )
    .await
    .unwrap();
    writer
        .read_serialized(|c| {
            snapshots::publish(c, now())?;
            assert!(snapshots::read(c, &scope_snapshot(c, &key), "question")?.is_empty());
            let l = store::load(c)?;
            assert!(l
                .assertions
                .values()
                .all(|a| l.status(&a.id, now()) != Status::Active));
            Ok(())
        })
        .unwrap();
}
