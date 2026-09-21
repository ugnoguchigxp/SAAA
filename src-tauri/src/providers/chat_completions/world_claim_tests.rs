#![cfg(test)]
use super::*;
use crate::runtime::context::world::{wire_test_support::Harness,state_claim::ClaimEvents};
use crate::runtime::event_hub::RuntimeEventSender;
use std::sync::{Arc,atomic::{AtomicUsize,Ordering}};
#[tokio::test]
async fn wr_t18_real_wire_claims_are_validated_before_display_and_changed_sources_rejected() {
    for mode in ["valid","forged","changed"] {
        let h=Harness::new();
        h.fixture.writer.write(|c| {c.execute("INSERT INTO schedule_entries(id,scope_ref,revision,status,due_at,kind,subject_ref,origin,created_at) VALUES('claim-deadline',?1,1,'scheduled',10000,'reminder','fixture','user_explicit',1000)",[&h.fixture.project]).map_err(crate::database_error)?;Ok(())}).unwrap();
        let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let endpoint=format!("http://{}/v1",listener.local_addr().unwrap());
        let writer=h.fixture.writer.clone();
        let router=axum::Router::new().route("/v1/chat/completions",axum::routing::post(move |axum::Json(body):axum::Json<Value>| {let writer=writer.clone();async move {
            let content=body["messages"].as_array().unwrap().iter().filter_map(|m|m["content"].as_str()).find(|v|v.contains(crate::runtime::context::world::render::WORLD_HEADER)).unwrap();
            let raw=content.split_once(crate::runtime::context::world::render::WORLD_HEADER).unwrap().1.split_once(crate::runtime::context::world::render::WORLD_FOOTER).unwrap().0;
            let frame:Value=serde_json::from_str(raw).unwrap();
            let source=&frame["sources"].as_array().unwrap().iter().find(|g|g["kind"]=="schedule").unwrap()["entries"][0];
            let mut claim=json!({"kind":"schedule","source_ref":source["source_id"],"source_version_or_digest":source["digest"],"value":source["payload"],"as_of_ms":source["as_of_ms"]});
            if mode=="forged" {claim["source_ref"]=json!("invented-deadline");}
            if mode=="changed" {writer.write(|c| {c.execute("UPDATE schedule_entries SET status='withdrawn' WHERE id='claim-deadline'",[]).map_err(crate::database_error)?;Ok(())}).unwrap();}
            axum::Json(json!({"choices":[{"index":0,"message":{"role":"assistant","content":json!({"claims":[claim]}).to_string()},"finish_reason":"stop"}]}))
        }}));
        let server=tokio::spawn(async move {axum::serve(listener,router).await.unwrap();});
        let deltas=Arc::new(AtomicUsize::new(0));let received=deltas.clone();
        let sink=tauri::ipc::Channel::<RuntimeEvent>::new(move |body| {if format!("{body:?}").contains("delta") {received.fetch_add(1,Ordering::SeqCst);} Ok(())});
        let guard=ClaimEvents(Box::new(sink));
        let input:crate::StartTurnInput=serde_json::from_value(json!({"runId":crate::memory::personal_state::world::runtime_test_support::RUN_ID,"conversationId":crate::PRIMARY_CONVERSATION_ID,"content":"hello","inputOrigin":"text","presentationMode":"visual"})).unwrap();
        let raw=run_mode(&endpoint,None,"fixture",&h.history,5000,ModelStreamContext {reasoning_effort:"low",max_output_tokens:256,input:&input,on_event:&guard,cancellation:Arc::default(),context_health:"green",context_sources:&h.composed.envelope.selected,context_omissions:&h.composed.envelope.omitted,output_persistence:Some(crate::ProviderOutputPersistence {state:&h.state,session_id:&h.session,world:h.composed.world.as_ref()})},RequestMode::JsonProbe).await;
        server.abort();
        assert_eq!(deltas.load(Ordering::SeqCst),0);
        assert!(!guard.voice_response_enabled());
        if mode=="changed" {assert!(raw.is_err());continue;}
        let answer=h.composed.world.as_ref().unwrap().accept_claims(&raw.unwrap(),"次の期限は？");
        assert_eq!(answer.is_ok(),mode=="valid");
        if let Ok(answer)=answer {assert!(answer.contains("claim-deadline"));assert!(!answer.contains("claims"));}
    }
}
