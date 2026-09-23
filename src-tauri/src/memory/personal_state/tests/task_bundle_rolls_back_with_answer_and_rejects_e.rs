use super::*;
#[test]
pub(super) fn task_bundle_rolls_back_with_answer_and_rejects_evidence_outside_manifest() {
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
#[test]
pub(super) fn world_projection_does_not_resurrect_a_deleted_source_on_restore() {
    use crate::memory::personal_state::world::test_support::{
        entity_assertion, insert_source, Committer, PROJECT,
    };
    use saaa_personal_state_core::world::EntityKind;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("world-state.sqlite3");
    let backup = directory.path().join("world-old.sqlite3");
    let writer = SqliteWriter::open(&path).unwrap();
    let source = writer
        .write(|c| Ok(insert_source(c, PROJECT, "ws1", "world restore source")))
        .unwrap();
    let now_ms = now();
    let mut committer = Committer {
        writer: &writer,
        project: PROJECT,
    };
    committer
        .commit(
            "world-restore",
            vec![entity_assertion(
                "went",
                "wentp",
                "we1",
                EntityKind::Concept,
                "Restore Concept",
                &["復元概念"],
                &source,
                PROJECT,
                now_ms,
            )],
        )
        .unwrap();
    // The projection indexes the name and alias before the backup.
    writer
        .read_serialized(|c| {
            let entities: u64 = c
                .query_row(
                    "SELECT count(*) FROM personal_world_entities WHERE entity_id='we1'",
                    [],
                    |r| r.get(0),
                )
                .map_err(crate::database_error)?;
            let aliases: u64 = c
                .query_row(
                    "SELECT count(*) FROM personal_world_aliases WHERE canonical_alias='復元概念'",
                    [],
                    |r| r.get(0),
                )
                .map_err(crate::database_error)?;
            assert_eq!(entities, 1);
            assert_eq!(aliases, 1);
            Ok(())
        })
        .unwrap();
    drop(writer);
    std::fs::copy(&path, &backup).unwrap();
    // The live database tombstones the source after the backup was taken. The
    // forget journal records the tombstone so a restored old DB cannot revive it.
    let writer = SqliteWriter::open(&path).unwrap();
    writer
        .write(|c| {
            c.execute(
                "INSERT OR IGNORE INTO personal_tombstones(source_id,forgotten_at) VALUES('ws1',?1)",
                params![now()],
            )
            .map_err(crate::database_error)?;
            Ok(())
        })
        .unwrap();
    drop(writer);
    // Restoring the old DB while the current journal is kept must not bring the
    // deleted World name, alias or edge back.
    std::fs::copy(&backup, &path).unwrap();
    let writer = SqliteWriter::open(&path).unwrap();
    writer
        .read_serialized(|c| {
            let entities: u64 = c
                .query_row(
                    "SELECT count(*) FROM personal_world_entities WHERE entity_id='we1'",
                    [],
                    |r| r.get(0),
                )
                .map_err(crate::database_error)?;
            let aliases: u64 = c
                .query_row(
                    "SELECT count(*) FROM personal_world_aliases WHERE canonical_alias='復元概念'",
                    [],
                    |r| r.get(0),
                )
                .map_err(crate::database_error)?;
            assert_eq!(entities, 0, "deleted World entity must not resurrect");
            assert_eq!(aliases, 0, "deleted World alias must not resurrect");
            Ok(())
        })
        .unwrap();
    drop(writer);
    std::fs::remove_file(journal::path(&path)).unwrap();
    assert!(SqliteWriter::open(&path).is_err());
}
#[test]
pub(super) fn world_assertions_stay_out_of_the_public_snapshot() {
    use crate::memory::personal_state::world::test_support::{
        entity_assertion, insert_source, Committer, PROJECT,
    };
    use saaa_personal_state_core::world::EntityKind;
    let writer = SqliteWriter::from_connection(db());
    let source = writer
        .write(|c| Ok(insert_source(c, PROJECT, "bs1", "boundary source")))
        .unwrap();
    let now_ms = now();
    let mut committer = Committer {
        writer: &writer,
        project: PROJECT,
    };
    committer
        .commit(
            "boundary-world",
            vec![entity_assertion(
                "bw1",
                "bw1p",
                "bwe1",
                EntityKind::Concept,
                "Boundary Concept",
                &[],
                &source,
                PROJECT,
                now_ms,
            )],
        )
        .unwrap();
    writer
        .read_serialized(|c| {
            let snapshot = super::super::commands::snapshot(c)?;
            let items = snapshot["items"].as_array().cloned().unwrap_or_default();
            assert!(items.iter().all(|item| {
                let kind = item["kind"].as_str().unwrap_or("");
                !kind.starts_with("world_")
            }));
            // The World projection still holds the entity: exclusion is at the
            // continuity/public boundary, not a deletion.
            let projected: u64 = c
                .query_row(
                    "SELECT count(*) FROM personal_world_entities WHERE entity_id='bwe1'",
                    [],
                    |r| r.get(0),
                )
                .map_err(crate::database_error)?;
            assert_eq!(projected, 1);
            Ok(())
        })
        .unwrap();
}
