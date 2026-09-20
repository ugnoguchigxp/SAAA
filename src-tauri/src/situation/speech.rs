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

pub(crate) fn inspect_tts_hold(state: &AppState) -> Option<SpeechHold> {
    let inner = state.situation.inner.lock().ok()?;
    let scene = inner.state.scene.clone();
    let proposed_attention = inner.decision.proposed_attention.clone();
    if !holds_speech(&scene, &proposed_attention) {
        return None;
    }
    let reason_code = inner
        .decision
        .reason_codes
        .first()
        .filter(|code| !code.is_empty())
        .cloned()
        .unwrap_or_else(|| "user-busy".to_string());
    Some(SpeechHold {
        scene,
        proposed_attention,
        reason_code,
    })
}

pub(crate) fn speech_holds_tts(state: &AppState) -> bool {
    inspect_tts_hold(state).is_some()
}

pub(crate) fn record_tts_held(
    state: &AppState,
    run_id: Option<&str>,
    conversation_id: &str,
    hold: &SpeechHold,
) {
    let mut attributes = BTreeMap::new();
    attributes.insert(
        "reasonCode".to_string(),
        AuditAttributeValue::Tag(hold.reason_code.clone()),
    );
    attributes.insert(
        "state".to_string(),
        AuditAttributeValue::Tag(hold.scene.clone()),
    );
    attributes.insert(
        "proposedAttention".to_string(),
        AuditAttributeValue::Tag(hold.proposed_attention.clone()),
    );
    let event = FrontendAuditEventInput {
        component: "situation".to_string(),
        event_name: "tts-held".to_string(),
        phase: "decision".to_string(),
        outcome: Some("blocked".to_string()),
        correlation_id: run_id.map(str::to_string),
        causation_id: None,
        conversation_id: Some(conversation_id.to_string()),
        runtime_run_id: run_id.map(str::to_string),
        session_id: None,
        subject_id: None,
        failure_code: None,
        attributes,
    };
    let _ = record_frontend_event(state, &event);
}

impl super::SituationRuntime {
    #[cfg(test)]
    pub(crate) fn set_scene_attention_for_test(&self, scene: &str, attention: &str) {
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        inner.state.scene = scene.to_string();
        inner.decision.proposed_attention = attention.to_string();
    }
}

#[cfg(test)]
#[path = "speech_tests.rs"]
mod speech_tests;
