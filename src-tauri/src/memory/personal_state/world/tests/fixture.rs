use super::*;
pub(super) struct Fixture {
    pub(super) writer: crate::persistence::sqlite::SqliteWriter,
    pub(super) source: SourceRef,
    pub(super) built_at: i64,
    pub(super) objective: String,
}
pub(super) fn world_entity_rows(c: &rusqlite::Connection) -> Result<Vec<(String, String, String)>, String> {
    let mut statement = c
        .prepare(
            "SELECT assertion_id,entity_id,status FROM personal_world_entities ORDER BY assertion_id",
        )
        .map_err(crate::database_error)?;
    let rows = statement
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .map_err(crate::database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(crate::database_error)?;
    Ok(rows)
}
pub(super) fn fixture() -> Fixture {
    let writer = writer_db();
    let now_ms = now();
    let source = writer
        .write(|c| {
            Ok(insert_source(
                c,
                PROJECT,
                "s1",
                "投機的デコードの導入は応答遅延を下げるかもしれない",
            ))
        })
        .unwrap();
    let mut committer = Committer {
        writer: &writer,
        project: PROJECT,
    };
    committer
        .commit(
            "fix-objective",
            vec![objective_assertion(
                "obj1", "obj1p", &source, PROJECT, now_ms,
            )],
        )
        .unwrap();
    committer
        .commit(
            "fix-entities",
            vec![
                entity_assertion(
                    "ent1",
                    "ent1p",
                    "e1",
                    EntityKind::Concept,
                    "Speculative Decodingの導入",
                    &["投機的デコードの導入"],
                    &source,
                    PROJECT,
                    now_ms,
                ),
                entity_assertion(
                    "ent2",
                    "ent2p",
                    "e2",
                    EntityKind::Metric,
                    "Decode Latency",
                    &[],
                    &source,
                    PROJECT,
                    now_ms,
                ),
                entity_assertion(
                    "ent3",
                    "ent3p",
                    "e3",
                    EntityKind::Metric,
                    "Voice Response Latency",
                    &[],
                    &source,
                    PROJECT,
                    now_ms,
                ),
                entity_assertion(
                    "ent4",
                    "ent4p",
                    "e4",
                    EntityKind::Project,
                    "SAAA",
                    &[],
                    &source,
                    PROJECT,
                    now_ms,
                ),
            ],
        )
        .unwrap();
    let support = |stance| vec![(source.key.clone(), stance)];
    committer
        .commit(
            "fix-relations",
            vec![
                relation_assertion(
                    "rel1",
                    "rel1p",
                    "e1",
                    "e2",
                    RelationType::Decreases,
                    Some(EffectInput::Intervention),
                    &[("config", "fixture-a")],
                    Basis::ModelHypothesis,
                    support(Stance::Supports),
                    &source,
                    PROJECT,
                    std::collections::BTreeSet::from(["ent1".to_string(), "ent2".to_string()]),
                    now_ms,
                ),
                relation_assertion(
                    "rel2",
                    "rel2p",
                    "e2",
                    "e3",
                    RelationType::Increases,
                    Some(EffectInput::QuantityIncrease),
                    &[("config", "fixture-a")],
                    Basis::ModelHypothesis,
                    support(Stance::Supports),
                    &source,
                    PROJECT,
                    std::collections::BTreeSet::from(["ent2".to_string(), "ent3".to_string()]),
                    now_ms,
                ),
                relation_assertion(
                    "rel3",
                    "rel3p",
                    "e3",
                    "e4",
                    RelationType::ImportantFor,
                    None,
                    &[],
                    Basis::UserStatement,
                    support(Stance::Supports),
                    &source,
                    PROJECT,
                    std::collections::BTreeSet::from(["ent3".to_string(), "ent4".to_string()]),
                    now_ms,
                ),
            ],
        )
        .unwrap();
    committer
        .commit(
            "fix-focus",
            vec![focus_assertion(
                "foc1",
                "foc1p",
                "e4",
                FocusReason::CurrentWork,
                Some("obj1"),
                &source,
                PROJECT,
                std::collections::BTreeSet::from(["ent4".to_string(), "obj1".to_string()]),
                now_ms,
            )],
        )
        .unwrap();
    // The adapter has processed s1; its job is complete before World commit.
    writer
        .write(|c| {
            c.execute(
                "UPDATE personal_jobs SET status='completed' WHERE source_sequence=(
                   SELECT sequence FROM personal_sources WHERE message_id='s1')",
                [],
            )
            .map_err(crate::database_error)?;
            Ok(())
        })
        .unwrap();
    Fixture {
        writer,
        source,
        built_at: now_ms,
        objective: "obj1".into(),
    }
}
pub(super) fn query(f: &Fixture, seeds: &[WorldSeed]) -> Result<WorldSlice, String> {
    f.writer.read_serialized(|c| {
        let ledger = store::load(c)?;
        let request = access(&ledger, PROJECT);
        activate(
            c,
            &ActivateInput {
                project_scope: PROJECT,
                access: &request,
                now: now(),
                seeds,
                causal_direction: CausalDirection::Forward,
                limits: Limits::default(),
                max_bytes: 8_192,
            },
        )
    })
}
#[test]
pub(super) fn t22_end_to_end_fixture_relevance_causal_and_gaps() {
    let f = fixture();
    let slice = query(&f, &[WorldSeed::EntityId("e1".into())]).unwrap();
    assert!(slice.notices.is_empty(), "notices: {:?}", slice.notices);
    let relevance: Vec<Vec<String>> = slice
        .relevance_paths
        .iter()
        .map(|p| p.nodes.clone())
        .collect();
    assert!(
        relevance
            .iter()
            .any(|nodes| nodes == &vec!["e1", "e2", "e3", "e4"]),
        "relevance={relevance:?}"
    );
    assert_eq!(slice.causal_paths.len(), 1);
    assert_eq!(slice.causal_paths[0].nodes, vec!["e1", "e2", "e3"]);
    // R1/R2 model hypotheses with declared conditions => one gap each.
    assert_eq!(slice.research_gaps.len(), 2, "{:?}", slice.research_gaps);
    assert!(slice
        .research_gaps
        .iter()
        .all(|g| g.reason == "hypothesis_unverified"));
    // R3 important_for never becomes a causal gap.
    assert!(slice
        .research_gaps
        .iter()
        .all(|g| g.relation_assertion_id != "rel3"));
    // Hypothesis labelling and evidence survive.
    let rel1 = slice
        .relations
        .iter()
        .find(|r| r.assertion_id == "rel1")
        .expect("rel1 present");
    assert_eq!(rel1.basis, "model_hypothesis");
    assert_eq!(rel1.conditions, vec![vec!["config", "fixture-a"]]);
    assert_eq!(rel1.evidence.len(), 1);
}
#[test]
pub(super) fn t09_rebuild_is_reproducible_and_empty_is_fine() {
    let f = fixture();
    let first = f.writer.read_serialized(world_entity_rows);
    // Rebuild twice, compare.
    for _ in 0..2 {
        f.writer.write(|c| store::rebuild(c, now())).unwrap();
    }
    let second = f.writer.read_serialized(world_entity_rows);
    assert_eq!(first, second);
    // A World-free database still builds an empty, usable projection.
    let empty = writer_db();
    empty
        .write(|c| {
            assert_eq!(
                c.query_row("SELECT count(*) FROM personal_world_entities", [], |r| r
                    .get::<_, u64>(0))
                    .map_err(crate::database_error)?,
                0
            );
            assert!(c
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM personal_world_projection_meta WHERE id=1)",
                    [],
                    |r| r.get::<_, bool>(0)
                )
                .map_err(crate::database_error)?);
            Ok(())
        })
        .unwrap();
}
#[test]
pub(super) fn t08_schema_migration_is_idempotent_and_adds_world_tables() {
    let c = memory_db();
    // Re-running the initializer must not fail or duplicate.
    crate::initialize_database(&c).unwrap();
    for table in [
        "personal_world_projection_meta",
        "personal_world_entities",
        "personal_world_aliases",
        "personal_world_relations",
        "personal_world_focus",
    ] {
        let exists: bool = c
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
                [table],
                |r| r.get(0),
            )
            .unwrap();
        assert!(exists, "missing {table}");
    }
    let indexes: i64 = c
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='index' AND name LIKE 'personal_world_%'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(indexes >= 5);
    // The forget trigger exists and is not a rewrite of an existing trigger.
    let trigger: bool = c
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='trigger' AND name='personal_world_forget')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(trigger);
    // D18/C6: the projection-format version column exists exactly once and
    // defaults to 1, and re-running the initializer does not duplicate it.
    let version_columns: i64 = c
        .query_row(
            "SELECT count(*) FROM pragma_table_info('personal_world_projection_meta') WHERE name='projection_version'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(version_columns, 1);
    let default_value: Option<String> = c
        .query_row(
            "SELECT dflt_value FROM pragma_table_info('personal_world_projection_meta') WHERE name='projection_version'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(default_value.as_deref(), Some("1"));
}
#[test]
pub(super) fn t10_direct_commit_cannot_bypass_world_validation() {
    let f = fixture();
    let now_ms = now();
    // A relation whose endpoint entity does not exist must be rejected even
    // when handed straight to store::commit.
    let source = f.source.clone();
    let bad = relation_assertion(
        "bad-rel",
        "bad-rel-p",
        "missing",
        "e2",
        RelationType::Decreases,
        Some(EffectInput::Intervention),
        &[],
        Basis::ModelHypothesis,
        vec![(source.key.clone(), Stance::Supports)],
        &source,
        PROJECT,
        std::collections::BTreeSet::from(["missing".to_string(), "ent2".to_string()]),
        now_ms,
    );
    let mut committer = Committer {
        writer: &f.writer,
        project: PROJECT,
    };
    let error = committer.commit("bad", vec![bad]).unwrap_err();
    assert_eq!(error, "world-invalid-reference");
    // Nothing partial was persisted.
    f.writer
        .read_serialized(|c| {
            assert_eq!(
                c.query_row(
                    "SELECT count(*) FROM personal_assertions WHERE id='bad-rel'",
                    [],
                    |r| r.get::<_, u64>(0)
                )
                .map_err(crate::database_error)?,
                0
            );
            Ok(())
        })
        .unwrap();
}
#[test]
pub(super) fn t12_forget_erases_projected_names_and_edges() {
    let f = fixture();
    let slice = query(&f, &[WorldSeed::ExactName("SAAA".into())]).unwrap();
    assert!(!slice.nodes.is_empty());
    // Memory OFF is irrelevant: the tombstone trigger wipes the projection.
    f.writer
        .write(|c| {
            c.execute("DELETE FROM conversation_messages WHERE id='s1'", [])
                .map_err(crate::database_error)?;
            Ok(())
        })
        .unwrap();
    f.writer
        .read_serialized(|c| {
            for table in [
                "personal_world_entities",
                "personal_world_aliases",
                "personal_world_relations",
                "personal_world_focus",
            ] {
                let count: u64 = c
                    .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
                    .map_err(crate::database_error)?;
                assert_eq!(count, 0, "{table} not erased");
            }
            assert!(store::load(c)?.assertions.is_empty());
            Ok(())
        })
        .unwrap();
}
#[test]
pub(super) fn t13_source_edit_uses_only_the_current_version() {
    let writer = writer_db();
    let now_ms = now();
    let source = writer
        .write(|c| Ok(insert_source(c, PROJECT, "s1", "旧い仮説")))
        .unwrap();
    let mut committer = Committer {
        writer: &writer,
        project: PROJECT,
    };
    committer
        .commit(
            "edit-obj",
            vec![objective_assertion(
                "obj1", "obj1p", &source, PROJECT, now_ms,
            )],
        )
        .unwrap();
    committer
        .commit(
            "edit-ent",
            vec![entity_assertion(
                "ent1",
                "ent1p",
                "e1",
                EntityKind::Concept,
                "Old",
                &[],
                &source,
                PROJECT,
                now_ms,
            )],
        )
        .unwrap();
    // Edit the source: the old version becomes unavailable.
    writer
        .write(|c| {
            c.execute(
                "UPDATE conversation_messages SET content='新しい仮説' WHERE id='s1'",
                [],
            )
            .map_err(crate::database_error)?;
            Ok(())
        })
        .unwrap();
    writer
        .read_serialized(|c| {
            let ledger = store::load(c)?;
            assert_ne!(ledger.status("ent1", now()), Status::Active);
            // Editing the source changes the input epoch, so the old projection
            // is unreadable until a rebuild removes the unavailable version.
            let request = access(&ledger, PROJECT);
            let slice = activate(
                c,
                &ActivateInput {
                    project_scope: PROJECT,
                    access: &request,
                    now: now(),
                    seeds: &[WorldSeed::EntityId("e1".into())],
                    causal_direction: CausalDirection::Forward,
                    limits: Limits::default(),
                    max_bytes: 8_192,
                },
            )?;
            assert!(slice.notices.contains(&"projection_stale".to_string()));
            assert_eq!(sources::load(c, 2, 0, 64)?.source.key.version, 2);
            Ok(())
        })
        .unwrap();
    writer.write(|c| store::rebuild(c, now())).unwrap();
    writer
        .read_serialized(|c| {
            assert_eq!(
                c.query_row("SELECT count(*) FROM personal_world_entities", [], |r| r
                    .get::<_, u64>(0))
                    .map_err(crate::database_error)?,
                0
            );
            Ok(())
        })
        .unwrap();
}
#[test]
pub(super) fn forgotten_source_version_is_not_revived_by_a_new_version_with_the_same_id() {
    let f = fixture();
    f.writer
        .write(|c| {
            c.execute(
                "UPDATE personal_sources SET available=0 WHERE message_id=?1 AND version=?2",
                rusqlite::params![f.source.key.id, f.source.key.version],
            )
            .map_err(crate::database_error)?;
            c.execute(
                "INSERT INTO personal_sources(message_id,version,role,bytes,recorded_at,available)
                 VALUES(?1,?2,'user',0,?3,1)",
                rusqlite::params![f.source.key.id, f.source.key.version + 1, now()],
            )
            .map_err(crate::database_error)?;
            Ok(())
        })
        .unwrap();

    let slice = query(&f, &[WorldSeed::EntityId("e1".into())]).unwrap();
    assert!(slice.nodes.is_empty());
    assert!(slice.relations.is_empty());
}
