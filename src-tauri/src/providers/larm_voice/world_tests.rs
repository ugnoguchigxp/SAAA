#![cfg(test)]
use super::*;
use axum::{extract::{Request,State}, response::{IntoResponse,Response},Json,Router};
use serde_json::{json,Value};
use std::sync::{atomic::AtomicI64,Mutex as StdMutex};
struct Fake { base:String, clock:Arc<AtomicI64>, body:StdMutex<Option<Value>>, released:AtomicBool }
async fn handle(State(f):State<Arc<Fake>>,request:Request)->Response {
    let path=request.uri().path().to_string();
    if path.starts_with("/v1/agent-connections") {
        if request.method()=="DELETE" { f.released.store(true,Ordering::SeqCst);return axum::http::StatusCode::NO_CONTENT.into_response(); }
        let mut value=json!({"id":"world-fixture","status":"ready","allocationId":"world-allocation","expiresAt":(chrono::Utc::now()+chrono::Duration::seconds(600)).to_rfc3339()});
        if path.ends_with("/claim") {
            value["providers"]=json!([("llm","openai.chat-completions.v1"),("tts","openai.audio-speech.v1"),("decision-default","openai.chat-completions.v1"),("asr","openai.audio-transcriptions.v1")].iter().map(|(name,protocol)|json!({"name":name,"protocol":protocol,"configuration":{"fields":{"baseURL":format!("{}/{name}/v1",f.base),"model":"fixture"}},"credential":{"token":format!("token-{name}")},"health":{"url":format!("{}/{name}/health",f.base),"maxAgeMs":10000}})).collect::<Vec<_>>());
            return Json(value).into_response();
        }
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        f.clock.fetch_add(3000,Ordering::SeqCst);
        return (axum::http::StatusCode::CREATED,Json(value)).into_response();
    }
    let name=path.split('/').nth(1).unwrap();
    assert_eq!(request.headers()["authorization"],format!("Bearer token-{name}"));
    if path.ends_with("/health") {
        let protocol=match name {"asr"=>"openai.audio-transcriptions.v1","tts"=>"openai.audio-speech.v1",_=>"openai.chat-completions.v1"};
        return Json(json!({"ready":true,"acceptingRequests":true,"probe":{"validated":true,"protocol":protocol}})).into_response();
    }
    assert_eq!(path,"/llm/v1/chat/completions");
    let body:Value=serde_json::from_slice(&axum::body::to_bytes(request.into_body(),65536).await.unwrap()).unwrap();
    let streaming=body["stream"]==true;
    *f.body.lock().unwrap()=Some(body);
    if !streaming { return Json(json!({"choices":[{"index":0,"message":{"role":"assistant","content":"fixture"},"finish_reason":"stop"}]})).into_response(); }
    ([ (axum::http::header::CONTENT_TYPE,"text/event-stream") ],format!("data: {}\n\ndata: [DONE]\n\n",json!({"choices":[{"index":0,"delta":{"content":"fixture"},"finish_reason":"stop"}]}))).into_response()
}
#[tokio::test]
async fn wr_t13_shared_voice_lease_wait_refreshes_actual_http_frame() {
    let _environment=crate::test_environment::larm_lock().lock().await;
    let h=crate::runtime::context::world::wire_test_support::Harness::new();
    let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let f=Arc::new(Fake {base:format!("http://{}",listener.local_addr().unwrap()),clock:h.fixture.clock.clone(),body:StdMutex::new(None),released:AtomicBool::new(false)});
    let router=Router::new().fallback(handle).with_state(f.clone());
    let server=tokio::spawn(async move {axum::serve(listener,router).await.unwrap();});
    let (cancel,_)=watch::channel(false);
    *OWNER.lock().await=Some(Arc::new(Owner {id:"world-owner".into(),conversation:crate::PRIMARY_CONVERSATION_ID.into(),base:f.base.clone(),profile:"fixture".into(),cancel,ready:OnceCell::new(),started:AtomicBool::new(false)}));
    let input:crate::StartTurnInput=serde_json::from_value(json!({"runId":crate::memory::personal_state::world::runtime_test_support::RUN_ID,"conversationId":crate::PRIMARY_CONVERSATION_ID,"content":"hello","inputOrigin":"voice","presentationMode":"visual"})).unwrap();
    let sink=tauri::ipc::Channel::<crate::RuntimeEvent>::new(|_|Ok(()));
    let outcome=crate::providers::stream::stream_voice_aware_dynamic_lan_provider(
        &crate::DynamicLanProviderSettings {id:"fixture".into(),enabled:true,label:"fixture".into(),location:"local".into(),host:"127.0.0.1".into(),request_options:None},
        &crate::HarnessSettings {address:f.base.clone(),larm_profile:Some("fixture".into()),tts_voice:None},true,crate::PRIMARY_CONVERSATION_ID,&h.history,10000,
        crate::providers::stream::ModelStreamContext {reasoning_effort:"low",max_output_tokens:128,input:&input,on_event:&sink,cancellation:Arc::default(),context_health:"green",context_sources:&h.composed.envelope.selected,context_omissions:&h.composed.envelope.omitted,output_persistence:Some(crate::ProviderOutputPersistence {state:&h.state,session_id:&h.session,world:h.composed.world.as_ref()})}).await;
    end("world-owner").await.unwrap();
    server.abort();
    assert!(matches!(outcome,crate::ProviderAttemptOutcome::Completed {..}),"{outcome:?}");
    h.assert_wire(f.body.lock().unwrap().as_ref().unwrap());
    assert!(f.released.load(Ordering::SeqCst));
}
