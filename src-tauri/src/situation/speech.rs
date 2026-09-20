use std::collections::BTreeMap;

use crate::persistence::audit::{
    record_frontend_event, AuditAttributeValue, FrontendAuditEventInput,
};
use crate::AppState;

pub(crate) struct SpeechHold {
    pub(crate) scene: String,
    pub(crate) proposed_attention: String,
    pub(crate) reason_code: String,
}

pub(crate) fn holds_speech(scene: &str, proposed_attention: &str) -> bool {
    scene == "MEETING" && matches!(proposed_attention, "IGNORE" | "OBSERVE")
}

pub(crate) fn speech_holds_tts(state: &AppState) -> bool {
    speech_holds_runtime(&state.situation)
}

/// A lightweight, lock-only check for the instant just before audio is queued. Long-running
/// voice work holds an `Arc<SituationRuntime>`, not `AppState`, so acknowledgement speech can
/// observe a meeting hold that began after ASR/turn dispatch.
pub(crate) fn speech_holds_runtime(runtime: &crate::situation::SituationRuntime) -> bool {
    runtime
        .inner
        .lock()
        .ok()
        .is_some_and(|inner| holds_speech(&inner.state.scene, &inner.decision.proposed_attention))
}

pub(crate) fn inspect_tts_hold(state: &AppState) -> Option<SpeechHold> {
    let inner = state.situation.inner.lock().ok()?;
    if !holds_speech(&inner.state.scene, &inner.decision.proposed_attention) {
        return None;
    }
    Some(SpeechHold {
        scene: inner.state.scene.clone(),
        proposed_attention: inner.decision.proposed_attention.clone(),
        reason_code: inner
            .decision
            .reason_codes
            .first()
            .cloned()
            .unwrap_or_else(|| "user-busy".into()),
    })
}

pub(crate) fn apply_tts_hold(
    state: &AppState,
    run_id: Option<&str>,
    conversation_id: &str,
) -> bool {
    inspect_tts_hold(state).is_some_and(|hold| {
        record_tts_held(state, run_id, conversation_id, &hold);
        true
    })
}

fn record_tts_held(
    state: &AppState,
    run_id: Option<&str>,
    conversation_id: &str,
    hold: &SpeechHold,
) {
    let Some(run_id) = run_id else { return };
    let Ok(mut inner) = state.situation.inner.lock() else {
        return;
    };
    if inner.tts_hold_audit_run.as_deref() == Some(run_id) {
        return;
    }
    inner.tts_hold_audit_run = Some(run_id.to_string());
    drop(inner);
    let tag = |key: &str, value: &str| (key.into(), AuditAttributeValue::Tag(value.into()));
    let _ = record_frontend_event(
        state,
        &FrontendAuditEventInput {
            component: "situation".into(),
            event_name: "tts-held".into(),
            phase: "decision".into(),
            outcome: Some("blocked".into()),
            correlation_id: Some(run_id.into()),
            causation_id: None,
            conversation_id: Some(conversation_id.into()),
            runtime_run_id: Some(run_id.into()),
            session_id: None,
            subject_id: None,
            failure_code: None,
            attributes: BTreeMap::from([
                tag("reasonCode", &hold.reason_code),
                tag("state", &hold.scene),
                tag("proposedAttention", &hold.proposed_attention),
            ]),
        },
    );
}

#[cfg(test)]
mod tests {
    include!("speech_tests.rs");
}
