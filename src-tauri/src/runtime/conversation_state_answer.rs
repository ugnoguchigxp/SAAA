//! Verifies model claims before selecting text for persistence and speech.
#[path = "conversation_state_card.rs"]
mod card;
use super::*;
pub(super) use card::persist_card;
pub(super) enum Validation {
    None,
    Model,
    Host(Box<crate::runtime::context::world::app_frame::Prepared>),
}
pub(super) fn accept(
    state: &AppState,
    input: &StartTurnInput,
    world: Option<&crate::runtime::context::world::turn::WorldLive>,
    raw: &str,
    events: &dyn RuntimeEventSender,
) -> (String, Validation) {
    let rendered = world
        .ok_or_else(|| "state-claim-unavailable".to_string())
        .and_then(|world| world.accept_claims(raw, &input.content));
    let (text, validation) = match rendered {
        Ok(text) => (text, Validation::Model),
        Err(reason) => {
            let _ = events.send(RuntimeEvent::Activity {
                run_id: input.run_id.clone(),
                kind: "state-answer-fallback".into(),
                summary: reason,
            });
            let card = crate::runtime::context::world::host_answer::prepare_card(
                state,
                &input.run_id,
                &input.content,
            );
            let validation = card
                .world
                .map(Box::new)
                .map(Validation::Host)
                .unwrap_or(Validation::None);
            (card.text, validation)
        }
    };
    events.set_completion_speech(&input.run_id, text.clone());
    (text, validation)
}
