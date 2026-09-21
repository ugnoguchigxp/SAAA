//! Verifies model claims before selecting text for persistence and speech.
use super::*;
pub(super) fn accept(
    state: &AppState,
    input: &StartTurnInput,
    world: Option<&crate::runtime::context::world::turn::WorldLive>,
    raw: &str,
    events: &dyn RuntimeEventSender,
) -> (String, bool) {
    let rendered = world
        .ok_or_else(|| "state-claim-unavailable".to_string())
        .and_then(|world| world.accept_claims(raw, &input.content));
    let (text, verified) = match rendered {
        Ok(text) => (text, true),
        Err(reason) => {
            let _ = events.send(RuntimeEvent::Activity {
                run_id: input.run_id.clone(),
                kind: "state-answer-fallback".into(),
                summary: reason,
            });
            (
                crate::runtime::context::world::host_answer::card(
                    state,
                    &input.run_id,
                    &input.content,
                ),
                false,
            )
        }
    };
    events.set_completion_speech(&input.run_id, text.clone());
    (text, verified)
}
