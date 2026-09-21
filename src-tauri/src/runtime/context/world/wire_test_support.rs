#![cfg(test)]
use super::{g1_tests as graph, turn::{compose_parts, TurnCompose}};
use crate::memory::personal_state::world::runtime_test_support::{Fixture,RUN_ID};
use std::sync::Arc;
pub(crate) struct Harness { pub fixture: Fixture, pub state: crate::AppState, pub composed: TurnCompose, pub history: Vec<crate::ConversationMessage>, pub session: String }
impl Harness {
    pub(crate) fn new() -> Self {
        let fixture=graph::g1_fixture();
        let capabilities=Arc::new(crate::generated_capabilities::service::CapabilityService::build(fixture.writer.clone(),&std::path::PathBuf::new(),std::path::PathBuf::new(),None));
        let state=crate::test_state::app_state_with_capabilities(fixture.writer.clone(),capabilities);
        let scope=graph::load_scope(&fixture); let access=fixture.access();
        let mut window=graph::window();window.messages.last_mut().unwrap().content="hello".into();
        let composed=compose_parts(true,Some(Arc::new(fixture.service().with_sources(state.situation.clone()))),access.principal,access.policy_revision,Some(graph::graph_request("tech")),RUN_ID,&scope,window,vec![],graph::allowed(&scope)).unwrap();
        let history=composed.envelope.messages.iter().enumerate().map(|(i,m)| crate::ConversationMessage {id:format!("context-{i}"),conversation_id:crate::PRIMARY_CONVERSATION_ID.into(),role:m.role.clone(),content:m.content.clone(),created_at:"1".into(),parts:None}).collect();
        let session=crate::begin_provider_session(&state,RUN_ID,"fixture","openai-compatible",&"a".repeat(64)).unwrap();
        Self {fixture,state,composed,history,session}
    }
    pub(crate) fn assert_wire(&self, body: &serde_json::Value) {
        let content=body["messages"].as_array().unwrap().iter().filter_map(|m|m["content"].as_str()).find(|s|s.contains(super::render::WORLD_HEADER)).unwrap();
        let json=content.split_once(super::render::WORLD_HEADER).unwrap().1.split_once(super::render::WORLD_FOOTER).unwrap().0.trim();
        let frame:serde_json::Value=serde_json::from_str(json).unwrap();
        assert_eq!(frame["captured_at_ms"],self.fixture.now());
        assert!(json.contains("situation"));
        use sha2::Digest;
        let digest=format!("{:x}",sha2::Sha256::digest(serde_json::to_vec(body).unwrap()));
        self.fixture.writer.read_serialized(|c| {
            let recorded:String=c.query_row("SELECT request_digest FROM context_generations ORDER BY ordinal DESC LIMIT 1",[],|r|r.get(0)).map_err(crate::database_error)?;
            assert_eq!(recorded,digest);
            let selected:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM context_generation_inputs WHERE source_kind='world-model' AND selected=1)",[],|r|r.get(0)).map_err(crate::database_error)?;
            assert!(selected);Ok(())
        }).unwrap();
    }
}

pub(crate) const TRANSITIONS: [&str;7] = ["initial","tool-continuation","fallback","scope-switch","correction","forget","session-resume"];
impl Harness {
    pub(crate) fn transition(&self, transition: &str) {
        self.fixture.writer.write(|c| {
            c.execute("UPDATE ui_settings SET enabled=1",[]).map_err(crate::database_error)?;
            let sql=match transition {
                "scope-switch"=>"UPDATE context_scope_epochs SET epoch=epoch+1 WHERE scope_key LIKE 'project:%'",
                "correction"=>"UPDATE personal_assertions SET erased=1 WHERE id='ent_tech'; UPDATE personal_scope SET revision=revision+1 WHERE id='primary'",
                "forget"=>"UPDATE personal_sources SET available=0 WHERE message_id='g1-src'",
                _=>return Ok(()),
            };
            c.execute_batch(sql).map_err(crate::database_error)
        }).unwrap();
    }
    pub(crate) fn matrix_report(&self,route:&str,transition:&str,bodies:&[serde_json::Value],denied:bool) {
        use sha2::Digest;
        let last=bodies.last();
        if denied {assert!(bodies.is_empty());} else {
            let body=last.expect("actual model wire");self.assert_wire(body);
            let frame=wire_frame(body).unwrap();
            if matches!(transition,"correction"|"forget") { assert!(!frame.to_string().contains("Speculative Decoding")); }
        }
        let frame=last.and_then(wire_frame);
        let hash=|value:&serde_json::Value|format!("{:x}",sha2::Sha256::digest(serde_json::to_vec(value).unwrap()));
        println!("WORLD_MATRIX_CASE={}",serde_json::json!({"case_id":format!("{route}:{transition}"),"route":route,"transition":transition,"source_kinds":frame.as_ref().and_then(|f|f["sources"].as_array()).map(|groups|groups.iter().map(|g|g["kind"].clone()).collect::<Vec<_>>()).unwrap_or_default(),"frame_digest":frame.as_ref().map(hash),"wire_digest":last.map(hash),"expected":if denied {"denied-before-model-wire"} else {"current-frame-and-matching-receipt"},"actual":if denied {"denied-before-model-wire"} else {"current-frame-and-matching-receipt"},"pass":true,"verification_level":"offline-wire","omission_reason":if denied {Some("scope-changed")} else {None},"request_count":bodies.len()}));
    }
}
pub(crate) fn wire_frame(body:&serde_json::Value)->Option<serde_json::Value> {
    let content=body["messages"].as_array()?.iter().filter_map(|m|m["content"].as_str()).find(|s|s.contains(super::render::WORLD_HEADER))?;
    let raw=content.split_once(super::render::WORLD_HEADER)?.1.split_once(super::render::WORLD_FOOTER)?.0;
    serde_json::from_str(raw.trim()).ok()
}
