//! Operator diagnostic using production discovery, credentials, budgets and inference.
//! Does not mutate conversation history or print credentials/provider response bodies.
use crate::providers::stream::{CleanupOutcome, ModelStreamContext, ProviderAttemptOutcome};
pub use crate::role_routing::operator_configuration::enable as enable_role_routing;
use std::sync::Arc;

/// Uses the production LFM parser and session lease; no conversation data or settings are written.
pub async fn check_frontdesk(base: &str) -> Result<String, String> {
    let credential =
        crate::providers::dynamic_lan::credential::load().map_err(|e| e.code().to_string())?;
    let (_cancel, receiver) = tokio::sync::watch::channel(false);
    eprintln!("stage=voice-session-prepare; status=started");
    let session = saaa_larm_session::Session::connect_with_profile_and_credential(
        base,
        saaa_larm_session::DEFAULT_PROFILE,
        credential.token().into(),
        receiver,
    )
    .await
    .map_err(|e| e.to_string())?;
    let result = async {
        let ready = crate::larm_voice::Ready {session:session.clone()};
        let mut history = Vec::new();
        let mut already_greeted = false;
        for (text, expected_think) in [
            ("こんにちは。", false),
            ("旅行の予定を考えているんだけど。", false),
            ("東京から京都へ2泊3日で行きます。移動時間も考えて、お寺を巡る具体的な旅行計画を比較して作ってください。", true),
        ] {
            history.push(serde_json::json!({"role":"user","content":text}));
            let started = std::time::Instant::now();
            let decision = crate::larm_voice::frontdesk_decision::decide(&ready,history.clone(),false,already_greeted).await.map_err(str::to_string)?;
            eprintln!("stage=lfm-response; think={}; elapsed_ms={}; say={:?}",decision.think,started.elapsed().as_millis(),decision.say);
            if decision.think != expected_think {return Err("lfm-live-reasoning-request-mismatch".into());}
            if decision.reply_key == Some("greeting") { already_greeted = true; }
            if let Some(say) = decision.say { history.push(serde_json::json!({"role":"assistant","content":say})); }
        }
        let qwen = async {
            let lease = session.acquire("llm").await.map_err(str::to_string)?;
            let provider = lease.provider();
            let request = "東京から京都へ2泊3日でお寺を巡る計画を、移動時間も検討して日本語100文字以内で答えてください。";
            let input:crate::StartTurnInput=serde_json::from_value(serde_json::json!({
                "runId":"diagnostic_qwen","conversationId":"conversation_diagnostic","content":request,
                "inputOrigin":"voice","presentationMode":"visual"
            })).unwrap();
            let model = crate::OpenAiCompatibleProviderSettings {id:"diagnostic-qwen".into(),enabled:true,
                label:"Qwen reasoning".into(),location:"local".into(),endpoint:provider.base_url.to_string(),
                model:provider.model.clone(),authentication:"api-key".into(),request_options:None};
            let messages = vec![crate::ipc_contract::ConversationMessage {id:"diagnostic_input".into(),conversation_id:input.conversation_id.clone(),role:"user".into(),content:request.into(),created_at:"1".into(),parts:None}];
            let started=std::time::Instant::now();
            eprintln!("stage=qwen-reasoning; status=started; model={}",provider.model);
            let outcome=crate::providers::stream::stream_model_provider_with_api_key(&model,&messages,240_000,
                Some(provider.token()),Some(lease.allocation_id()),ModelStreamContext {
                    reasoning_effort:"medium",max_output_tokens:2048,input:&input,on_event:&DiscardDiagnosticEvents,
                    cancellation:Arc::default(),context_health:"green",context_sources:&[],context_omissions:&[],output_persistence:None,
                }).await;
            match outcome {
                ProviderAttemptOutcome::Completed {content,..} if !content.trim().is_empty() => {
                    eprintln!("stage=qwen-final; status=received; elapsed_ms={}; chars={}",started.elapsed().as_millis(),content.chars().count());
                    Ok::<_,String>(())
                },
                ProviderAttemptOutcome::Failed {public_message,..}=>Err(public_message.as_str().into()),
                _=>Err("qwen-final-not-received".into()),
            }
        };
        let follow_up = async {
            history.push(serde_json::json!({"role":"user","content":"はい、お願いします。"}));
            let started=std::time::Instant::now();
            let decision=crate::larm_voice::frontdesk_decision::decide(&ready,history,true,already_greeted).await.map_err(str::to_string)?;
            eprintln!("stage=lfm-while-qwen-pending; think={}; elapsed_ms={}",decision.think,started.elapsed().as_millis());
            if decision.think {return Err("lfm-duplicated-pending-request".into());}
            Ok::<_,String>(())
        };
        let (answer,follow_up)=tokio::join!(qwen,follow_up);
        answer?;follow_up?;
        Ok("LFM: greeting=reply; incomplete-request=reply; coherent-request=reasoning-requested; pending-follow-up=reply; Qwen=final-received".into())
    }.await;
    let released = session.close().await.map_err(str::to_string);
    released?;
    result
}

pub async fn check_response(host: &str) -> Result<String, String> {
    let started = std::time::Instant::now();
    let cancellation = Arc::new(crate::RunCancellation::default());
    let provider = crate::DynamicLanProviderSettings {
        id: "harness-diagnostic".into(),
        enabled: true,
        label: "Harness diagnostic".into(),
        location: "local".into(),
        host: host.into(),
        request_options: None,
    };
    let prompt = "Reply with exactly: SAAA_DYNAMIC_OK";
    let input = crate::StartTurnInput {
        run_id: format!("diagnostic_{}", uuid::Uuid::new_v4().simple()),
        conversation_id: "conversation_diagnostic".into(),
        content: prompt.into(),
        workspace_path: None,
        retry_input_message_id: None,
        source_id: None,
        scope_refs: vec![],
        input_origin: "voice".into(),
        presentation_mode: "visual".into(),
    };
    let history = [crate::ipc_contract::ConversationMessage {
        parts: None,
        id: "diagnostic_prompt".into(),
        conversation_id: input.conversation_id.clone(),
        role: "user".into(),
        content: prompt.into(),
        created_at: String::new(),
    }];
    eprintln!("stage=harness-llm-roundtrip; status=started; timeout_ms=240000");
    let outcome = crate::providers::stream::stream_dynamic_lan_provider(
        &provider,
        &history,
        240_000,
        cancellation.clone(),
        ModelStreamContext {
            reasoning_effort: "low",
            max_output_tokens: crate::providers::completion::DEFAULT_MAX_OUTPUT_TOKENS,
            input: &input,
            on_event: &DiscardDiagnosticEvents,
            cancellation,
            context_health: "green",
            context_sources: &[],
            context_omissions: &[],
            output_persistence: None,
        },
    )
    .await;
    match outcome {
        ProviderAttemptOutcome::Completed {
            content,
            cleanup: CleanupOutcome::Released,
        } if content.trim() == "SAAA_DYNAMIC_OK" => Ok(format!(
            "provider=harness-default; response=exact-match; connection=released; elapsed_ms={}",
            started.elapsed().as_millis()
        )),
        ProviderAttemptOutcome::Failed { public_message, .. } => {
            Err(public_message.as_str().into())
        }
        ProviderAttemptOutcome::Cancelled { .. } => Err("diagnostic-cancelled".into()),
        _ => Err("diagnostic-response-mismatch-or-release-incomplete".into()),
    }
}

#[derive(Clone, Copy)]
struct DiscardDiagnosticEvents;
impl crate::runtime::event_hub::RuntimeEventSender for DiscardDiagnosticEvents {
    fn send(&self, _: crate::ipc_contract::RuntimeEvent) -> tauri::Result<()> {
        Ok(())
    }
    fn clone_box(&self) -> Box<dyn crate::runtime::event_hub::RuntimeEventSender> {
        Box::new(*self)
    }
}
