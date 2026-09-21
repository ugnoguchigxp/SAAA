//! Async driver scaffolding for role routing.
//!
//! The driver owns the per-conversation execution slot and runs a compiled recipe step by step.
//! Effects are only executed after the caller has committed the corresponding ledger transition
//! (the coordinator does that). While an actor future is pending, a cancel/input signal is still
//! observed through `select!`, so IO never blocks later input handling.
#![allow(dead_code)]

use super::recipe::PlannedStep;
use std::collections::HashSet;
use std::sync::Mutex;
use tokio::sync::{mpsc, watch};

/// Terminal outcome of a single step dispatch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StepOutcome {
    pub(crate) ordinal: u32,
    pub(crate) status: &'static str,
}

impl StepOutcome {
    pub(crate) fn succeeded(ordinal: u32) -> Self {
        Self {
            ordinal,
            status: "succeeded",
        }
    }
    pub(crate) fn failed(ordinal: u32) -> Self {
        Self {
            ordinal,
            status: "failed",
        }
    }
    pub(crate) fn cancelled(ordinal: u32) -> Self {
        Self {
            ordinal,
            status: "cancelled",
        }
    }
}

/// Completion events emitted by the driver. They are plain data; the coordinator owns persisting
/// them. An unbounded channel keeps the driver from awaiting on a slow observer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DriverEvent {
    StepSucceeded(u32),
    StepFailed(u32),
    StepCancelled(u32),
}

impl DriverEvent {
    fn for_outcome(outcome: &StepOutcome) -> Self {
        match outcome.status {
            "succeeded" => DriverEvent::StepSucceeded(outcome.ordinal),
            "failed" => DriverEvent::StepFailed(outcome.ordinal),
            _ => DriverEvent::StepCancelled(outcome.ordinal),
        }
    }
}

/// Ensures at most one driver runs per conversation. The guard releases the slot on drop, which
/// also covers an aborted task.
#[derive(Default)]
pub(crate) struct ConversationRegistry {
    active: Mutex<HashSet<String>>,
}

pub(crate) struct ConversationGuard<'a> {
    registry: &'a ConversationRegistry,
    conversation_id: String,
}

impl ConversationRegistry {
    pub(crate) fn try_acquire(
        &self,
        conversation_id: &str,
    ) -> Result<ConversationGuard<'_>, String> {
        let mut active = self
            .active
            .lock()
            .map_err(|_| "Role-routing registry lock is poisoned".to_string())?;
        if !active.insert(conversation_id.to_string()) {
            return Err("Role-routing driver is already active for this conversation".into());
        }
        Ok(ConversationGuard {
            registry: self,
            conversation_id: conversation_id.to_string(),
        })
    }

    pub(crate) fn is_active(&self, conversation_id: &str) -> bool {
        self.active
            .lock()
            .map(|active| active.contains(conversation_id))
            .unwrap_or(false)
    }
}

impl Drop for ConversationGuard<'_> {
    fn drop(&mut self) {
        if let Ok(mut active) = self.registry.active.lock() {
            active.remove(&self.conversation_id);
        }
    }
}

/// Runs compiled steps in order until the recipe finishes or a cancel is observed. The caller is
/// responsible for committing the step claim before calling `execute` for that step. A cancelled
/// or failed step stops every later step from starting.
pub(crate) async fn run_steps_until_cancel<F, Fut>(
    steps: Vec<PlannedStep>,
    mut cancel: watch::Receiver<bool>,
    events: mpsc::UnboundedSender<DriverEvent>,
    mut execute: F,
) -> Vec<StepOutcome>
where
    F: FnMut(PlannedStep) -> Fut,
    Fut: std::future::Future<Output = StepOutcome>,
{
    let mut outcomes = Vec::new();
    for step in steps {
        if *cancel.borrow() {
            break;
        }
        let ordinal = step.ordinal;
        let outcome = tokio::select! {
            outcome = execute(step) => outcome,
            _ = cancel.changed() => StepOutcome::cancelled(ordinal),
        };
        let _ = events.send(DriverEvent::for_outcome(&outcome));
        let stop = outcome.status != "succeeded";
        outcomes.push(outcome);
        if stop || *cancel.borrow() {
            break;
        }
    }
    outcomes
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use tokio::sync::oneshot;

    fn step(ordinal: u32) -> PlannedStep {
        PlannedStep {
            ordinal,
            purpose: "respond",
            role: "reasoner".into(),
            actor_id: "qwen".into(),
            depends_on: Vec::new(),
        }
    }

    #[test]
    fn rr_05_one_actor_per_conversation() {
        let registry = ConversationRegistry::default();
        let first = registry.try_acquire("c1").expect("first");
        assert!(registry.is_active("c1"));
        assert!(registry.try_acquire("c1").is_err());
        let other = registry.try_acquire("c2").expect("other conversation");
        assert!(registry.is_active("c2"));
        drop(first);
        assert!(!registry.is_active("c1"));
        assert!(registry.try_acquire("c1").is_ok());
        drop(other);
    }

    #[tokio::test]
    async fn rr_05_io_does_not_block_input() {
        let (cancel_tx, cancel_rx) = watch::channel(false);
        let (event_tx, mut event_rx) = mpsc::unbounded_channel();
        let (started_tx, started_rx) = oneshot::channel::<()>();
        let started_tx = Arc::new(Mutex::new(Some(started_tx)));
        let gate = Arc::new(tokio::sync::Semaphore::new(0));
        let ran_second = Arc::new(AtomicUsize::new(0));
        let ran_second_task = ran_second.clone();

        let handle = tokio::spawn(run_steps_until_cancel(
            vec![step(0), step(1), step(2)],
            cancel_rx,
            event_tx,
            move |step| {
                let started_tx = started_tx.clone();
                let gate = gate.clone();
                let ran_second = ran_second_task.clone();
                async move {
                    match step.ordinal {
                        0 => StepOutcome::succeeded(0),
                        1 => {
                            if let Some(sender) = started_tx.lock().expect("lock").take() {
                                let _ = sender.send(());
                            }
                            // Pending IO that never completes on its own.
                            let _permit = gate.acquire().await;
                            StepOutcome::succeeded(1)
                        }
                        _ => {
                            ran_second.fetch_add(1, Ordering::SeqCst);
                            StepOutcome::succeeded(step.ordinal)
                        }
                    }
                }
            },
        ));

        started_rx.await.expect("step 1 started");
        // The cancel arrives while step 1 IO is pending. The driver must observe it without the
        // IO having completed, and the following step must never start.
        cancel_tx.send(true).expect("cancel");
        let outcomes = handle.await.expect("driver joins");
        assert_eq!(
            outcomes,
            vec![StepOutcome::succeeded(0), StepOutcome::cancelled(1)]
        );
        assert_eq!(ran_second.load(Ordering::SeqCst), 0);
        let mut events = Vec::new();
        while let Ok(event) = event_rx.try_recv() {
            events.push(event);
        }
        assert_eq!(
            events,
            vec![DriverEvent::StepSucceeded(0), DriverEvent::StepCancelled(1)]
        );
    }

    #[tokio::test]
    async fn rr_05_two_steps_run_in_order() {
        let (_cancel_tx, cancel_rx) = watch::channel(false);
        let (event_tx, mut event_rx) = mpsc::unbounded_channel();
        let order = Arc::new(Mutex::new(Vec::new()));
        let order_task = order.clone();
        let outcomes =
            run_steps_until_cancel(vec![step(0), step(1)], cancel_rx, event_tx, move |step| {
                let order = order_task.clone();
                async move {
                    order.lock().expect("lock").push(step.ordinal);
                    StepOutcome::succeeded(step.ordinal)
                }
            })
            .await;
        assert_eq!(outcomes.len(), 2);
        assert_eq!(*order.lock().expect("lock"), vec![0, 1]);
        let mut events = Vec::new();
        while let Ok(event) = event_rx.try_recv() {
            events.push(event);
        }
        assert_eq!(
            events,
            vec![DriverEvent::StepSucceeded(0), DriverEvent::StepSucceeded(1)]
        );
    }
}
