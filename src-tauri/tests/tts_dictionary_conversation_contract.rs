#[path = "../src/tts_dictionary/contracts.rs"]
mod contracts;
#[path = "../src/tts_dictionary/custom_matcher.rs"]
mod custom_matcher;
#[path = "../src/tts_dictionary/proposals.rs"]
mod proposals;
#[path = "../src/tts_dictionary/service.rs"]
mod service;
#[path = "../src/tts_dictionary/tool_definitions.rs"]
mod tool_definitions;
use contracts::{validate, Entry, ExpectedEntry};
use custom_matcher::CompiledDictionary;
use rusqlite::Connection;

fn compile(entries: Vec<Entry>) -> CompiledDictionary {
    CompiledDictionary::new(entries.into_iter().map(|e| (e.written, e.spoken)).collect())
}
fn database() -> Connection {
    let db = Connection::open_in_memory().unwrap();
    db.execute_batch("CREATE TABLE tts_dictionary(written TEXT PRIMARY KEY,spoken TEXT NOT NULL,updated_at TEXT NOT NULL);").unwrap();
    db
}
fn entry(spoken: &str) -> Entry {
    Entry {
        written: "今日".into(),
        spoken: spoken.into(),
    }
}
fn expected(spoken: Option<&str>) -> ExpectedEntry {
    ExpectedEntry {
        spoken: spoken.map(str::to_string),
    }
}
fn save(db: &mut Connection, spoken: &str, old: Option<&str>) -> Result<service::Mutation, String> {
    service::save(
        db,
        old.map(|_| "今日"),
        &entry(spoken),
        &expected(old),
        "new",
        |_| Ok(()),
    )
}

#[test]
fn add_update_and_same_reading_share_saved_state_and_immutable_index() {
    let mut db = database();
    let added = save(&mut db, "こんにち", None).unwrap();
    let old_index = added.dictionary.unwrap();
    assert_eq!(old_index.apply("今日の話"), "こんにちの話");
    let updated = save(&mut db, "きょう", Some("こんにち")).unwrap();
    assert_eq!(updated.previous.as_deref(), Some("こんにち"));
    assert_eq!(updated.dictionary.unwrap().apply("今日の話"), "きょうの話");
    assert_eq!(old_index.apply("今日の話"), "こんにちの話");
    db.execute("UPDATE tts_dictionary SET updated_at='sentinel'", [])
        .unwrap();
    let no_op = save(&mut db, "きょう", Some("stale")).unwrap();
    assert!(no_op.dictionary.is_none());
    assert_eq!(
        db.query_row("SELECT updated_at FROM tts_dictionary", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "sentinel"
    );
}

#[test]
fn concurrent_change_deletion_and_new_registration_are_not_overwritten() {
    let mut db = database();
    save(&mut db, "こんにち", None).unwrap();
    assert_eq!(
        save(&mut db, "きょう", Some("old")).err().unwrap(),
        service::CONFLICT
    );
    assert_eq!(
        save(&mut db, "きょう", None).err().unwrap(),
        service::CONFLICT
    );
    assert!(service::delete(&mut db, "今日", &expected(Some("old"))).is_err());
    service::delete(&mut db, "今日", &expected(Some("こんにち"))).unwrap();
    assert!(save(&mut db, "きょう", Some("こんにち")).is_err());
    assert_eq!(service::lookup(&db, "今日").unwrap(), None);
}

#[test]
fn rename_guards_source_and_destination_and_read_skip_is_distinct_from_absence() {
    let mut db = database();
    save(&mut db, "", None).unwrap();
    assert_eq!(service::lookup(&db, "今日").unwrap(), Some(String::new()));
    let renamed = Entry {
        written: "本日".into(),
        spoken: "ほんじつ".into(),
    };
    assert!(service::save(
        &mut db,
        Some("今日"),
        &renamed,
        &expected(None),
        "new",
        |_| Ok(())
    )
    .is_err());
    service::save(
        &mut db,
        Some("今日"),
        &renamed,
        &expected(Some("")),
        "new",
        |_| Ok(()),
    )
    .unwrap();
    assert!(service::lookup(&db, "今日").unwrap().is_none());
    save(&mut db, "きょう", None).unwrap();
    assert!(service::save(
        &mut db,
        Some("今日"),
        &renamed,
        &expected(Some("きょう")),
        "new",
        |_| Ok(())
    )
    .is_err());
    assert_eq!(
        service::lookup(&db, "今日").unwrap().as_deref(),
        Some("きょう")
    );
}

#[test]
fn cancellation_before_commit_and_database_failure_roll_back_saved_rows() {
    let mut db = database();
    let calls = std::cell::Cell::new(0);
    let result = service::save(
        &mut db,
        None,
        &entry("きょう"),
        &expected(None),
        "new",
        |_| {
            calls.set(calls.get() + 1);
            if calls.get() == 2 {
                Err("cancelled".into())
            } else {
                Ok(())
            }
        },
    );
    assert!(result.is_err());
    assert!(service::lookup(&db, "今日").unwrap().is_none());
    db.execute_batch("CREATE TRIGGER deny_dictionary BEFORE INSERT ON tts_dictionary BEGIN SELECT RAISE(ABORT,'fixture failure'); END;").unwrap();
    assert!(save(&mut db, "きょう", None).is_err());
    assert!(service::lookup(&db, "今日").unwrap().is_none());
}

#[test]
fn publication_confirmation_keep_consumption_and_scope_are_bound() {
    let store = proposals::Proposals::default();
    let p = store.issue(
        "conversation",
        "run_a",
        "今日".into(),
        Some("こんにち".into()),
        Some("きょう".into()),
    );
    assert!(store.get("other", "run_a", &p.lookup_id, false).is_none());
    assert!(store
        .get("conversation", "run_b", &p.lookup_id, false)
        .is_none());
    store.require_confirmation(&p);
    assert!(store.pending("conversation").is_none());
    assert!(store
        .get("conversation", "run_b", &p.lookup_id, true)
        .is_none());
    store.finish_turn("conversation", "run_a", true);
    assert!(store.pending("conversation").is_some());
    assert!(store
        .get("conversation", "run_a", &p.lookup_id, true)
        .is_none());
    assert!(store
        .get("conversation", "run_b", &p.lookup_id, true)
        .is_some());
    store.consume(&p);
    assert!(store.pending("conversation").is_none());
    assert!(store
        .get("conversation", "run_b", &p.lookup_id, true)
        .is_none());
}

#[test]
fn unpublished_cancelled_replaced_and_intervening_turn_proposals_expire() {
    for outcome in ["unpublished", "cancelled", "replaced", "intervening"] {
        let store = proposals::Proposals::default();
        let p = store.issue(
            "c",
            "run_a",
            "今日".into(),
            Some("こんにち".into()),
            Some("きょう".into()),
        );
        store.require_confirmation(&p);
        match outcome {
            "unpublished" => store.finish_turn("c", "run_a", false),
            "cancelled" => store.cancel_run("run_a"),
            "replaced" => {
                store.issue("c", "run_b", "明日".into(), None, Some("あした".into()));
            }
            _ => {
                store.finish_turn("c", "run_a", true);
                store.finish_turn("c", "run_b", true);
                // The host expires only after checking persisted input order.
                store.consume(&p);
            }
        }
        assert!(
            store.get("c", "run_c", &p.lookup_id, true).is_none(),
            "{outcome}"
        );
    }
}

#[test]
fn tool_surface_is_small_and_preserves_local_dictionary_boundary() {
    let definitions = tool_definitions::definitions();
    assert_eq!(definitions.len(), 2);
    assert_eq!(
        definitions[1]["function"]["parameters"]["additionalProperties"],
        false
    );
    assert_eq!(
        definitions[1]["function"]["parameters"]["required"],
        serde_json::json!(["lookupId", "mode"])
    );
    assert!(validate(&entry("きょう\n")).is_err());
}

#[test]
fn large_dictionary_updates_preserve_longest_match_and_stream_boundaries() {
    let mut db = database();
    let start = std::time::Instant::now();
    for index in 0..5000 {
        db.execute(
            "INSERT INTO tts_dictionary VALUES(?1,?2,'fixture')",
            rusqlite::params![format!("用語{index:04}"), format!("よみ{index}")],
        )
        .unwrap();
    }
    let added = save(&mut db, "きょう", None).unwrap();
    assert_eq!(added.entries.len(), 5001);
    let index = added.dictionary.unwrap();
    assert_eq!(index.apply("今日 用語4999"), "きょう よみ4999");
    assert_eq!(index.ready_prefix_len("話の今"), "話の".len());
    eprintln!("5001 entries fixture and save: {:?}", start.elapsed());
}

#[test]
fn a_late_failed_or_completed_turn_cannot_erase_a_newer_confirmation() {
    let store = proposals::Proposals::default();
    let p = store.issue(
        "c",
        "run_new",
        "今日".into(),
        Some("こんにち".into()),
        Some("きょう".into()),
    );
    store.require_confirmation(&p);
    store.finish_turn("c", "run_old", false);
    store.finish_turn("c", "run_new", true);
    store.finish_turn("c", "run_old", true);
    assert!(store.get("c", "run_next", &p.lookup_id, true).is_some());
}

#[test]
fn indexed_lookup_cold_build_save_and_warm_apply_at_three_sizes() {
    for size in [1, 5000, 10000] {
        let mut db = database();
        {
            let tx = db.transaction().unwrap();
            for n in 0..size {
                tx.execute(
                    "INSERT INTO tts_dictionary VALUES(?1,?2,'fixture')",
                    rusqlite::params![format!("用語{n:05}"), format!("よみ{n}")],
                )
                .unwrap();
            }
            tx.commit().unwrap();
        }
        let started = std::time::Instant::now();
        for _ in 0..1000 {
            assert_eq!(
                service::lookup(&db, "用語00000").unwrap().as_deref(),
                Some("よみ0")
            );
        }
        let lookup = started.elapsed();
        let started = std::time::Instant::now();
        let index = compile(service::list(&db).unwrap());
        let cold = started.elapsed();
        let started = std::time::Instant::now();
        for _ in 0..1000 {
            assert_eq!(index.apply("用語00000の話"), "よみ0の話");
        }
        let warm = started.elapsed();
        let started = std::time::Instant::now();
        let changed = save(&mut db, "きょう", None).unwrap();
        assert!(changed.dictionary.is_some());
        let write = started.elapsed();
        eprintln!("size={size} lookup1000={lookup:?} cold={cold:?} warm1000={warm:?} save_build={write:?}");
    }
}
