use super::memory_contract::{grant, result};

use super::*;

#[test]
fn saved_memory_answers_leave_history_on_edit_revoke_retirement_and_scope_change() {
    for change in ["edit", "revoke", "retire", "scope", "membership"] {
        let c = db();
        insert(&c, "origin", "2024年に京都へ行った。");
        insert(&c, "unrelated", "別の会話です。");
        let key = grant(&c);
        episode_export::capture(&c, "run", &result(&c, &key), std::slice::from_ref(&key)).unwrap();
        c.execute("INSERT INTO conversation_messages VALUES('answer',?1,'assistant','京都旅行を覚えています。',?2)",params![crate::PRIMARY_CONVERSATION_ID,now().to_string()]).unwrap();
        c.execute(
            "INSERT INTO memory_episode_artifacts VALUES('run','answer')",
            [],
        )
        .unwrap();
        c.execute("INSERT INTO memory_snapshot_run_inputs SELECT 'run',message_id,version FROM personal_sources WHERE message_id='origin' AND available=1", []).unwrap();
        match change {
            "edit" => {
                c.execute(
                    "UPDATE conversation_messages SET content='訂正後の原記録' WHERE id='origin'",
                    [],
                )
                .unwrap();
            }
            "revoke" => {
                c.execute("UPDATE memory_episode_grants SET enabled=0,revision=revision+1 WHERE scope_key=?1",[&key]).unwrap();
            }
            "retire" => {
                c.execute(
                    "INSERT INTO memory_episode_retired_sources VALUES('origin',1)",
                    [],
                )
                .unwrap();
            }
            "scope" => {
                c.execute(
                    "UPDATE context_scopes SET state='revoked' WHERE scope_key=?1",
                    [&key],
                )
                .unwrap();
            }
            "membership" => {
                crate::runtime::context::scope::register(&c, "project", "demo").unwrap();
                c.execute("INSERT INTO conversation_message_scopes VALUES('origin','project:demo','focus')", []).unwrap();
            }
            _ => unreachable!(),
        }
        let saved: bool = c
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM conversation_messages WHERE id='answer')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(!saved, "saved answer must be unavailable after {change}");
        let originals: u32 = c
            .query_row(
                "SELECT count(*) FROM conversation_messages WHERE id IN ('origin','unrelated')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(originals, 2, "raw histories remain after {change}");
    }
}

#[test]
fn snapshot_only_saved_answer_is_removed_on_raw_edit_without_episode_grant() {
    let c = db();
    insert(&c, "origin", "私は紅茶が好きです。");
    insert(&c, "answer", "紅茶を選びます。");
    c.execute(
        "INSERT INTO memory_episode_artifacts VALUES('run','answer')",
        [],
    )
    .unwrap();
    c.execute("INSERT INTO memory_snapshot_run_inputs SELECT 'run',message_id,version FROM personal_sources WHERE message_id='origin'", []).unwrap();
    c.execute(
        "UPDATE conversation_messages SET role='assistant' WHERE id='origin'",
        [],
    )
    .unwrap();
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
