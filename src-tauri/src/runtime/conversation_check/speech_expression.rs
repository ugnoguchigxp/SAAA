//! One Laya decision for the exact TTS chunk, shared by voice and avatar.
use crate::{
    providers::laya, voice::cloud_tts::speech_directive::SpeechExpression, RunCancellation,
};
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
use tauri::Emitter;

static IN_FLIGHT: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
pub(super) struct Expression {
    pub(super) motion: String,
    pub(super) voice: SpeechExpression,
    pub(super) fallback: bool,
}

impl Default for Expression {
    fn default() -> Self {
        Self {
            motion: "neutral".into(),
            voice: SpeechExpression::Natural,
            fallback: true,
        }
    }
}

pub(super) fn parse(response: &Value) -> Result<Expression, String> {
    let motion = response["answers"]["motion"]["choice"]
        .as_str()
        .ok_or("missing motion")?;
    if laya::avatar_question()["motion"]["criteria"]
        .get(motion)
        .is_none()
    {
        return Err("unknown motion".into());
    }
    let voice = match response["answers"]["voice"]["choice"].as_str() {
        Some("natural") => SpeechExpression::Natural,
        Some("bright") => SpeechExpression::Bright,
        Some("gentle") => SpeechExpression::Gentle,
        Some("serious") => SpeechExpression::Serious,
        Some("excited") => SpeechExpression::Excited,
        _ => return Err("unknown voice expression".into()),
    };
    Ok(Expression {
        motion: motion.into(),
        voice,
        fallback: false,
    })
}

pub(super) async fn decide(
    harness: &crate::HarnessSettings,
    text: &str,
    cancellation: &RunCancellation,
    budget_ms: u64,
) -> Result<Expression, String> {
    if cancellation.is_cancelled() {
        return Err("Speech cancelled".into());
    }
    // A timed-out task finishes releasing its LARM session in the background.
    // Keep this slot until cleanup ends; later chunks never build a waiting backlog.
    let Ok(slot) = IN_FLIGHT.try_lock() else {
        return Ok(Expression::default());
    };
    let Ok(credential) = crate::providers::dynamic_lan::credential::load() else {
        return Ok(Expression::default());
    };
    let harness = harness.clone();
    let state = json!({"utterance": text});
    let task = tokio::spawn(async move {
        let _slot = slot;
        let result = laya::choose(
            &harness,
            credential.token(),
            &state,
            laya::speech_question(),
            &|_| {},
        )
        .await?;
        parse(&result.response)
    });
    await_decision(task, cancellation, budget_ms).await
}

async fn await_decision(
    task: tokio::task::JoinHandle<Result<Expression, String>>,
    cancellation: &RunCancellation,
    budget_ms: u64,
) -> Result<Expression, String> {
    tokio::select! { biased;
        _ = cancellation.cancelled() => Err("Speech cancelled".into()),
        result = tokio::time::timeout(std::time::Duration::from_millis(budget_ms), task) =>
            Ok(result.ok().and_then(Result::ok).and_then(Result::ok).unwrap_or_default()),
    }
}

pub(super) struct ChunkCue<R: tauri::Runtime> {
    app: tauri::AppHandle<R>,
    payload: Value,
}

impl<R: tauri::Runtime> ChunkCue<R> {
    pub(super) async fn prepare(
        app: &tauri::AppHandle<R>,
        input_id: &str,
        state: &crate::AppState,
        harness: &crate::HarnessSettings,
        text: &str,
        cancellation: &RunCancellation,
        budget_ms: u64,
    ) -> Result<(SpeechExpression, Self), String> {
        // The expression hint comes from LARM; skip it while LARM is known to be unreachable.
        let decision = if crate::providers::service_registry::LocalAvailability::of(state).larm
            == crate::providers::reachability::Reachability::Unreachable
        {
            Default::default()
        } else {
            decide(harness, text, cancellation, budget_ms).await?
        };
        let expression = decision.voice;
        let cue = Self::new(app, input_id, decision);
        Ok((expression, cue))
    }

    pub(super) fn new(app: &tauri::AppHandle<R>, input_id: &str, expression: Expression) -> Self {
        Self {
            app: app.clone(),
            payload: json!({
                "conversationId": crate::PRIMARY_CONVERSATION_ID, "inputId": input_id,
                "chunkId": SEQUENCE.fetch_add(1, Ordering::Relaxed) + 1,
                "phase": "started", "motion": expression.motion,
                "voice": expression.voice.audit_value(), "fallback": expression.fallback,
            }),
        }
    }

    pub(super) fn on_audio_ready(
        &self,
        cancellation: Arc<RunCancellation>,
    ) -> impl FnOnce() + Send + 'static {
        let app = self.app.clone();
        let payload = self.payload.clone();
        move || {
            if !cancellation.is_cancelled() {
                let _ = app.emit("conversation-speech-expression", payload);
            }
        }
    }
}

impl<R: tauri::Runtime> Drop for ChunkCue<R> {
    fn drop(&mut self) {
        self.payload["phase"] = json!("ended");
        let _ = self
            .app
            .emit("conversation-speech-expression", &self.payload);
    }
}

#[cfg(test)]
#[path = "speech_expression_tests.rs"]
mod tests;
