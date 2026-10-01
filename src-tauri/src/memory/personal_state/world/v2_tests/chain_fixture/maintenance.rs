//! Continuous-maintenance outcome and evidence-history regressions.
#![cfg(test)]
use super::*;
#[test]
fn world_maintenance_quoted_outcome_updates_only_matching_counterexample() {
    use saaa_personal_state_core::world::extraction::Extraction;
    for (direction, comparison, condition, disputed) in [
        ("decrease", "cmp", "a", true),
        ("unchanged", "cmp", "a", false),
        ("mixed", "cmp", "a", false),
        ("increase", "cmp", "a", false),
        ("decrease", "other", "a", false),
        ("decrease", "cmp", "b", false),
    ] {
        let f = fixture_v2();
        let text = format!("comparison {comparison} config {condition}: {direction}");
        f.writer.write(|c| {
            let source = insert_source(c, PROJECT, "observed-result", &text);
            let extraction = Extraction::parse(&json!({"candidates":[],"outcomes":[{"prior_assertion_id":"rel_inc","metric_entity_id":"m1","comparison_id":comparison,"conditions":[{"key":"config","value":condition}],"direction":direction,"quote":text,"quote_start":0,"quote_end":text.len()}],"no_change":false}).to_string(), &text).unwrap();
            let before = store::load(c)?;
            super::super::super::extracted_outcomes::commit(c, &extraction, &source, PROJECT, "outcome-fixture", now())?;
            let after = store::load(c)?;
            if disputed {
                assert_eq!(after.status("rel_inc", now()), Status::Superseded);
                assert!(after.assertions.values().any(|a| a.id != "rel_inc" && a.kind == Kind::WorldRelation && after.status(&a.id, now()) == Status::Disputed));
            } else {
                assert_eq!(after.revision, before.revision);
                assert_eq!(after.status("rel_inc", now()), Status::Active);
            }
            let revision = after.revision;
            assert_eq!(c.query_row("SELECT count(*) FROM personal_world_observations",[],|r|r.get::<_,u64>(0)).unwrap(),1);
            super::super::super::extracted_outcomes::commit(c,&extraction,&source,PROJECT,"outcome-fixture",now())?;
            assert_eq!(store::load(c)?.revision,revision);
            assert_eq!(c.query_row("SELECT count(*) FROM personal_world_observations",[],|r|r.get::<_,u64>(0)).unwrap(),1);
            let visible = super::super::super::super::commands::snapshot(c)?;
            assert_eq!(visible["worldItems"].as_array().unwrap().iter().filter(|v| v["kind"] == "world_observation").count(), 1);
            c.execute("DELETE FROM personal_source_scope_refs WHERE source_id=?1", [&source.key.id]).unwrap();
            let revoked = super::super::super::super::commands::snapshot(c)?;
            assert!(revoked["worldItems"].as_array().unwrap().iter().all(|v| v["kind"] != "world_observation"));
            c.execute("INSERT INTO personal_tombstones(source_id,forgotten_at) VALUES(?1,?2)",rusqlite::params![source.key.id,now()]).unwrap();
            assert_eq!(c.query_row("SELECT count(*) FROM personal_world_observations",[],|r|r.get::<_,u64>(0)).unwrap(),0);
            Ok(())
        }).unwrap();
    }
}

#[test]
fn world_maintenance_evidence_history_exceeds_payload_window_without_losing_dependencies() {
    let f = fixture_v2();
    f.writer.write(|c| {
        let original = store::load(c)?.assertions["rel_inc"].clone();
        let mut payload = super::super::super::validation::load_payload_json(c, &original.payload_ref)?;
        payload["confidence"] = serde_json::Value::Null;
        payload["strength"] = serde_json::Value::Null;
        payload["assessment_refs"] = json!([]);
        payload["evidence_stances"] = json!([]);
        payload["epistemic"] = json!("hypothesis");
        let text = "cmp config a: repeated increase report";
        for i in 0..6 {
            let source = insert_source(c,PROJECT,&format!("repeat-{i}"),text);
            let extracted = saaa_personal_state_core::world::extraction::Extraction::parse(&json!({"candidates":[{"kind":"world_relation","payload":payload,"quote":text,"quote_start":0,"quote_end":text.len(),"epistemic":"inferred","replaces":null}],"no_change":false}).to_string(),text).unwrap();
            super::super::super::extraction::commit(c,&extracted,&source,PROJECT,"history-fixture",original.provenance.clone(),now())?;
        }
        let ledger = store::load(c)?;
        let latest = ledger.assertions.values().find(|a| a.semantic_key == original.semantic_key && ledger.status(&a.id,now()) == Status::Active).unwrap();
        assert_eq!(latest.evidence.len(),7);
        assert_eq!(latest.valid_from,original.valid_from);
        assert_eq!(latest.input_dependencies.len(),7);
        let value = super::super::super::validation::load_payload_json(c,&latest.payload_ref)?;
        assert_eq!(value["evidence_stances"].as_array().unwrap().len(),4);
        assert_eq!(value["confidence"]["value"],800);
        let latest_id = latest.id.clone();
        c.execute("INSERT INTO personal_tombstones(source_id,forgotten_at) VALUES('repeat-1',?1)",[now()]).unwrap();
        assert!(!store::load(c)?.assertions.contains_key(&latest_id));
        Ok(())
    }).unwrap();
}
