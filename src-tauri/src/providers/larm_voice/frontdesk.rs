//! ASR's sole conversation entry point. Qwen runs independently after a durable LFM handoff.
use crate::{AppState, ipc_contract::RuntimeEvent, persistence::audit::{self, FrontendAuditEventInput}};
use super::{frontdesk_decision, frontdesk_repository as repository, speech_priority};
use serde::Serialize;

// Serialize conversational context, not Qwen reasoning or TTS playback.
static INPUT: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LfmUtteranceResult {
    handoff_id: Option<String>,
    request_content: Option<String>,
    speech_epoch: u64,
}

fn record(state: &AppState, conversation: &str, utterance: &str, name: &str, failure: Option<&str>) {
    let handoff: Option<String> = state.sqlite_readers.read(|c|c.query_row(
        "SELECT handoff_id FROM lfm_voice_utterances WHERE utterance_id=?1",[utterance],|r|r.get(0))
        .map_err(crate::database_error)).ok().flatten();
    let mut attributes = std::collections::BTreeMap::new();
    if let Some(id) = handoff {attributes.insert("handoffId".into(),audit::AuditAttributeValue::Tag(id));}
    let _ = audit::record_frontend_event(state, &FrontendAuditEventInput {
        component: "conversation".into(), event_name: name.into(), phase: "state".into(),
        outcome: failure.map(|_|"failure".into()), correlation_id: Some(utterance.into()),
        causation_id: Some(utterance.into()), conversation_id: Some(conversation.into()),
        runtime_run_id: None, session_id: None, subject_id: Some(utterance.into()),
        failure_code: failure.map(str::to_string), attributes,
    });
}

#[tauri::command]
pub(crate) async fn receive_lfm_utterance(
    state: tauri::State<'_, AppState>, conversation_id: String, utterance_id: String,
    text: String, on_received: tauri::ipc::Channel<()>,
) -> Result<LfmUtteranceResult, String> {
    crate::validate_identifier(&conversation_id,"conversation id")?;
    crate::validate_identifier(&utterance_id,"utterance id")?;
    if text.trim().is_empty() || text.chars().count() > 16_000 { return Err("lfm-utterance-length-invalid".into()); }
    let _serial = INPUT.lock().await;
    // Validate ownership before persisting into a conversation retired by the UI.
    let ready = super::current(&conversation_id).await.map_err(|_|"lfm-session-not-ready")?;
    state.sqlite_writer.write(|c|repository::accept(c,&conversation_id,&utterance_id,text.trim()))?;
    let _ = on_received.send(());
    record(&state,&conversation_id,&utterance_id,"lfm-utterance-received",None);
    let speech_epoch = speech_priority::epoch(&conversation_id);
    let result = async {
        let (history,pending) = state.sqlite_readers.read(|c|repository::context(c,&conversation_id))?;
        record(&state,&conversation_id,&utterance_id,"lfm-response-requested",None);
        let decision = tokio::time::timeout(std::time::Duration::from_secs(20),
            frontdesk_decision::decide(&ready,history,pending)).await
            .map_err(|_|"lfm-response-timeout")?.map_err(str::to_string)?;
        // An owner switch cancels stale decisions instead of handing work to a different chat.
        super::current(&conversation_id).await.map_err(|_|"lfm-session-changed")?;
        let handoff = state.sqlite_writer.write(|c|repository::complete(c,&utterance_id,&decision))?;
        record(&state,&conversation_id,&utterance_id,
            if handoff.is_some() {"lfm-delegated-to-qwen"} else {"lfm-replied-without-delegation"},None);
        let (handoff_id,request_content) = handoff.map(|(id,text)|(Some(id),Some(text))).unwrap_or_default();
        Ok::<_,String>(LfmUtteranceResult {handoff_id,request_content,speech_epoch})
    }.await;
    if let Err(code) = &result {
        // Never expose provider URLs, tokens or arbitrary HTTP bodies as diagnostics.
        let safe = if code.starts_with("lfm-") {code.as_str()} else {"lfm-persistence-failed"};
        let _ = state.sqlite_writer.write(|c| c.execute("UPDATE lfm_voice_utterances SET status='failed',failure_code=?2 WHERE utterance_id=?1",rusqlite::params![utterance_id,safe]).map(|_|()).map_err(crate::database_error));
        record(&state,&conversation_id,&utterance_id,"lfm-response-failed",Some(safe));
        return Err(safe.into());
    }
    result
}

#[tauri::command]
pub(crate) async fn speak_lfm_reply(
    state: tauri::State<'_, AppState>, conversation_id: String, utterance_id: String,
    speech_epoch: u64, on_event: tauri::ipc::Channel<RuntimeEvent>,
) -> Result<(),String> {
    let presentation = crate::voice_behavior::effective_presentation(&state,None,&conversation_id)?;
    if presentation.decision != "speak" {return Ok(());}
    super::current(&conversation_id).await.map_err(|_|"lfm-session-not-ready")?;
    let text: String = state.sqlite_readers.read(|c|c.query_row("SELECT m.content FROM lfm_voice_utterances u JOIN conversation_messages m ON m.id=u.reply_message_id WHERE u.utterance_id=?1 AND u.conversation_id=?2",rusqlite::params![utterance_id,conversation_id],|r|r.get(0)).map_err(crate::database_error))?;
    let run = crate::new_id("lfm_speech");
    state.streaming_tts.begin(&state,&run,true,on_event,Some(&conversation_id)).await.map_err(|_|"lfm-speech-prepare-failed")?;
    if crate::voice_behavior::effective_presentation(&state,None,&conversation_id)?.decision != "speak" {
        state.streaming_tts.cancel(&run);
        return Ok(());
    }
    let queued = speech_priority::queue_reply(&state.streaming_tts,&conversation_id,&run,&text,speech_epoch)
        .map_err(|_|"lfm-speech-queue-failed")?;
    record(&state,&conversation_id,&utterance_id,if queued {"lfm-speech-queued"} else {"lfm-speech-preempted-by-answer"},None);
    Ok(())
}
