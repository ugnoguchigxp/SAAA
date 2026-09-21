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
        assert!(json.contains("situation"));assert!(json.contains("correlates_with"));
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
