use super::{frontdesk_repository as repo, frontdesk_decision::{ConversationAction,ConversationDecision}};
use crate::PRIMARY_CONVERSATION_ID as CONVERSATION;
fn decision(action: ConversationAction) -> ConversationDecision {ConversationDecision {reply:"はい。".into(),action}}

#[test]
fn handoff_keeps_all_segments_and_claims_one_qwen_run_without_duplicate_input() {
    let c = rusqlite::Connection::open_in_memory().unwrap();
    crate::persistence::schema::initialize_database(&c).unwrap();
    repo::accept(&c,CONVERSATION,"u1","京都へ行きたい。").unwrap();
    assert!(repo::accept(&c,CONVERSATION,"u1","duplicate").is_err());
    assert!(repo::complete(&c,"u1",&decision(ConversationAction::Respond)).unwrap().is_none());
    repo::accept(&c,CONVERSATION,"u2","2泊3日の計画を作って。").unwrap();
    let (handoff,content) = repo::complete(&c,"u2",&decision(ConversationAction::Delegate)).unwrap().unwrap();
    assert_eq!(content,"京都へ行きたい。\n2泊3日の計画を作って。");
    assert!(repo::context(&c,CONVERSATION).unwrap().1);
    let state = crate::test_support::app_state(c);
    let mut input: crate::StartTurnInput = serde_json::from_value(serde_json::json!({
        "runId":"run_handoff_test","conversationId":CONVERSATION,"content":content,
        "sourceId":handoff,"inputOrigin":"voice","presentationMode":"visual"
    })).unwrap();
    assert_eq!(crate::runtime::turns::prepare_runtime_run(&state,&input).unwrap(),"conversation");
    state.sqlite_readers.read(|c| {
        let count:i64=c.query_row("SELECT COUNT(*) FROM conversation_messages WHERE content=?1",[&input.content],|r|r.get(0)).unwrap();
        assert_eq!(count,1);
        let scope:String=c.query_row("SELECT status FROM runtime_scope_resolutions WHERE run_id=?1",[&input.run_id],|r|r.get(0)).unwrap();
        assert_eq!(scope,"resolved");
        Ok(())
    }).unwrap();
    input.run_id="run_duplicate".into();
    assert!(crate::runtime::turns::prepare_runtime_run(&state,&input).unwrap_err().contains("already-claimed"));
    state.sqlite_writer.write(|c| {
        c.execute("UPDATE runtime_runs SET status='completed' WHERE id='run_handoff_test'",[]).unwrap();
        repo::accept(c,CONVERSATION,"u3","ありがとう。")?;
        assert!(!repo::context(c,CONVERSATION)?.1);
        let (_,text)=repo::complete(c,"u3",&decision(ConversationAction::Delegate))?.unwrap();
        assert_eq!(text,"ありがとう。"); // Already delegated segments cannot silently re-enter a request.
        Ok(())
    }).unwrap();
}

#[test]
fn altered_or_cross_conversation_handoff_is_rejected() {
    let c=rusqlite::Connection::open_in_memory().unwrap();
    crate::persistence::schema::initialize_database(&c).unwrap();
    repo::accept(&c,CONVERSATION,"u1","計算して。").unwrap();
    let (handoff,_)=repo::complete(&c,"u1",&decision(ConversationAction::Delegate)).unwrap().unwrap();
    let input:crate::StartTurnInput=serde_json::from_value(serde_json::json!({
        "runId":"run_test","conversationId":CONVERSATION,"content":"違う指示",
        "sourceId":handoff,"inputOrigin":"voice","presentationMode":"visual"
    })).unwrap();
    assert!(repo::claim_handoff(&c,&input).is_err());
}
