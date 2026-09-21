//! Pure root state machine. The coordinator persists `Transition` before dispatching effects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Phase {
    Queued,
    Responding,
    Draining,
    Completed,
    Cancelled,
    Failed,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Event {
    Start,
    CandidateReady { revision: u32 },
    InputBarrier,
    Resume,
    Cancel,
    Fail,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Effect {
    DispatchActor { revision: u32 },
    CancelChildren,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct State {
    pub(crate) phase: Phase,
    pub(crate) revision: u32,
    pub(crate) barrier: bool,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Transition {
    pub(crate) state: State,
    pub(crate) effects: Vec<Effect>,
}

pub(crate) fn reduce(state: &State, event: Event) -> Transition {
    let mut next = state.clone();
    let mut effects = Vec::new();
    match event {
        Event::Start if next.phase == Phase::Queued => {
            next.phase = Phase::Responding;
            effects.push(Effect::DispatchActor {
                revision: next.revision,
            });
        }
        Event::InputBarrier if matches!(next.phase, Phase::Responding | Phase::Draining) => {
            next.barrier = true;
            next.phase = Phase::Draining;
        }
        Event::Resume if next.phase == Phase::Draining && next.barrier => {
            next.barrier = false;
            next.revision += 1;
            next.phase = Phase::Responding;
            effects.push(Effect::DispatchActor {
                revision: next.revision,
            });
        }
        Event::CandidateReady { revision }
            if next.phase == Phase::Responding && !next.barrier && revision == next.revision =>
        {
            next.phase = Phase::Completed
        }
        Event::Cancel
            if !matches!(
                next.phase,
                Phase::Completed | Phase::Cancelled | Phase::Failed
            ) =>
        {
            next.phase = Phase::Cancelled;
            next.barrier = true;
            effects.push(Effect::CancelChildren);
        }
        Event::Fail if !matches!(next.phase, Phase::Completed | Phase::Cancelled) => {
            next.phase = Phase::Failed
        }
        _ => {}
    }
    Transition {
        state: next,
        effects,
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};
    use std::thread;
    #[test]
    fn rr_17_cancel_prevents_late_candidate_acceptance() {
        let initial = State {
            phase: Phase::Responding,
            revision: 0,
            barrier: false,
        };
        let cancelled = reduce(&initial, Event::Cancel).state;
        assert_eq!(
            reduce(&cancelled, Event::CandidateReady { revision: 0 })
                .state
                .phase,
            Phase::Cancelled
        );
    }
    #[test]
    fn rr_16_result_during_classification_is_held() {
        let initial = State {
            phase: Phase::Responding,
            revision: 0,
            barrier: false,
        };
        let held = reduce(&initial, Event::InputBarrier).state;
        assert_eq!(
            reduce(&held, Event::CandidateReady { revision: 0 })
                .state
                .phase,
            Phase::Draining
        );
    }

    #[test]
    fn rr_29_manual_barrier_holds_a_completion_after_an_update() {
        let release = Arc::new(Barrier::new(2));
        let candidate_release = release.clone();
        let candidate = thread::spawn(move || {
            candidate_release.wait();
            reduce(
                &State {
                    phase: Phase::Draining,
                    revision: 0,
                    barrier: true,
                },
                Event::CandidateReady { revision: 0 },
            )
        });
        // The test intentionally does not sleep: the barrier fixes result delivery after the
        // durable update boundary, which is the ordering an adapter must respect.
        let updated = reduce(
            &State {
                phase: Phase::Responding,
                revision: 0,
                barrier: false,
            },
            Event::InputBarrier,
        );
        release.wait();
        let late = candidate.join().expect("candidate completes");
        assert_eq!(updated.state.phase, Phase::Draining);
        assert_eq!(late.state.phase, Phase::Draining);
        assert!(late.effects.is_empty());
    }
}
