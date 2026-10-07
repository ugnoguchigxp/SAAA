use super::memory_contract::{grant, result};
use super::*;

fn resolve(
    c: &Connection,
    run: &str,
    message: &str,
    project: Option<&str>,
) -> crate::runtime::context::scope::ScopeSnapshot {
    let refs = project
        .map(|p| vec![json!({"kind":"project","id":p,"relation":"focus"})])
        .unwrap_or_default();
    let input:crate::StartTurnInput=serde_json::from_value(json!({"runId":run,"conversationId":crate::PRIMARY_CONVERSATION_ID,"content":"続けてください","workspacePath":null,"scopeRefs":refs,"inputOrigin":"text","presentationMode":"visual"})).unwrap();
    c.execute("INSERT INTO runtime_runs(id,conversation_id,route_kind,status,input_message_id,started_at) VALUES(?1,?2,'conversation.respond','running',?3,'1')",params![run,crate::PRIMARY_CONVERSATION_ID,message]).unwrap();
    crate::runtime::context::scope::resolve(c, &input, message, false).unwrap()
}
fn previous(c: &Connection) -> String {
    insert(c, "origin", "2024年に京都へ行った。");
    let key = grant(c);
    resolve(c, "previous", "origin", None);
    episode_export::capture(c, "previous", &result(c, &key), std::slice::from_ref(&key)).unwrap();
    c.execute("INSERT INTO conversation_messages VALUES('answer',?1,'assistant','京都旅行の記憶に基づく回答です。',?2)",params![crate::PRIMARY_CONVERSATION_ID,now().to_string()]).unwrap();
    c.execute(
        "INSERT INTO memory_episode_artifacts VALUES('previous','answer')",
        [],
    )
    .unwrap();
    crate::runtime::context::scope::attach_output(c, "previous", "answer").unwrap();
    key
}

#[test]
fn continuing_scope_reuses_exact_episode_refs_and_raw_fetch_without_another_search() {
    let c = db();
    let key = previous(&c);
    insert(&c, "current", "その時は何をしましたか？");
    let scope = resolve(&c, "current-run", "current", None);
    let (text, inputs) = episode_export::history_view(&c, &scope, "current").unwrap();
    let text = text.unwrap();
    assert!(text.contains("RECENT_EPISODE_REFERENCES") && text.contains("immutable"));
    assert!(!text.contains("sqlitePath") && !text.contains("sourceContract"));
    assert_eq!(inputs.len(), 1);
    assert!(inputs.iter().any(|s| s.id == "origin" && s.version == 1));
    let raw = episode_export::fetch_source(
        &c,
        "current-run",
        &json!({"id":"episode","sourceKey":"immutable","sourceId":"origin"}).to_string(),
    )
    .unwrap();
    assert!(raw.contains("2024年に京都へ行った。"));
    episode_export::reuse_history(&c, "current-run", &scope, "current").unwrap();
    c.execute("INSERT INTO conversation_messages VALUES('next-answer',?1,'assistant','元の記憶を継続して参照した回答です。',?2)",params![crate::PRIMARY_CONVERSATION_ID,now().to_string()]).unwrap();
    c.execute(
        "INSERT INTO memory_episode_artifacts VALUES('current-run','next-answer')",
        [],
    )
    .unwrap();
    c.execute(
        "UPDATE memory_episode_grants SET enabled=0,revision=revision+1 WHERE scope_key=?1",
        [key],
    )
    .unwrap();
    assert!(episode_export::history_view(&c, &scope, "current")
        .unwrap()
        .0
        .is_none());
    assert!(episode_export::fetch_source(
        &c,
        "current-run",
        &json!({"id":"episode","sourceKey":"immutable","sourceId":"origin"}).to_string()
    )
    .is_err());
    assert_eq!(
        c.query_row(
            "SELECT count(*) FROM conversation_messages WHERE id IN ('answer','next-answer')",
            [],
            |r| r.get::<_, u32>(0)
        )
        .unwrap(),
        0
    );
}

#[test]
fn reference_sets_do_not_cross_focus_scope_or_the_recent_history_boundary() {
    let c = db();
    previous(&c);
    crate::runtime::context::scope::register(&c, "project", "different").unwrap();
    insert(&c, "project-turn", "別のProjectの話をします。");
    let project = resolve(&c, "project-run", "project-turn", Some("different"));
    assert!(episode_export::history_view(&c, &project, "project-turn")
        .unwrap()
        .0
        .is_none());
    for i in 0..16 {
        insert(&c, &format!("later-{i}"), "後の話題です。");
    }
    insert(&c, "later-current", "今の話を続けます。");
    let later = resolve(&c, "later-run", "later-current", None);
    assert!(episode_export::history_view(&c, &later, "later-current")
        .unwrap()
        .0
        .is_none());
}
