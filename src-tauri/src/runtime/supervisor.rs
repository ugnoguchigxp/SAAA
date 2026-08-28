use super::contracts::{
    RunFailureCode, RunOutcome, RunPhase, RunSignal, RunSupervisionPolicy, SupervisorAction,
    TerminalStatus,
};

#[derive(Debug, Clone)]
pub struct RunSupervisor {
    policy: RunSupervisionPolicy,
    phase: RunPhase,
    request_started_at_ms: Option<u64>,
    hard_started_at_ms: Option<u64>,
    last_progress_at_ms: Option<u64>,
    assistant_completed_at_ms: Option<u64>,
    interrupt_started_at_ms: Option<u64>,
    pending_outcome: Option<RunOutcome>,
    interrupt_sent: bool,
}

impl RunSupervisor {
    pub fn new(policy: RunSupervisionPolicy, _now_ms: u64) -> Self {
        Self {
            policy,
            phase: RunPhase::Starting,
            request_started_at_ms: None,
            hard_started_at_ms: None,
            last_progress_at_ms: None,
            assistant_completed_at_ms: None,
            interrupt_started_at_ms: None,
            pending_outcome: None,
            interrupt_sent: false,
        }
    }

    pub fn last_progress_at_ms(&self) -> Option<u64> {
        self.last_progress_at_ms
    }

    pub fn begin_request(&mut self, now_ms: u64) {
        if self.phase == RunPhase::Starting {
            self.request_started_at_ms = Some(now_ms);
        }
    }

    pub fn complete_request(&mut self) {
        self.request_started_at_ms = None;
    }

    pub fn pending_outcome(&self) -> Option<RunOutcome> {
        self.pending_outcome
    }

    pub fn next_deadline_ms(&self) -> Option<u64> {
        match self.phase {
            RunPhase::Starting => self
                .request_started_at_ms
                .map(|started| started.saturating_add(self.policy.request_timeout_ms)),
            RunPhase::Running => earliest_deadline([
                self.hard_started_at_ms
                    .map(|started| started.saturating_add(self.policy.hard_timeout_ms)),
                self.last_progress_at_ms
                    .map(|started| started.saturating_add(self.policy.progress_idle_timeout_ms)),
            ]),
            RunPhase::Draining => earliest_deadline([
                self.hard_started_at_ms
                    .map(|started| started.saturating_add(self.policy.hard_timeout_ms)),
                self.assistant_completed_at_ms
                    .map(|started| started.saturating_add(self.policy.terminal_gap_timeout_ms)),
            ]),
            RunPhase::Interrupting => self
                .interrupt_started_at_ms
                .map(|started| started.saturating_add(self.policy.interrupt_grace_ms)),
            RunPhase::Completed | RunPhase::Failed | RunPhase::Cancelled => None,
        }
    }

    pub fn apply(&mut self, now_ms: u64, signal: RunSignal) -> Vec<SupervisorAction> {
        if self.phase.is_terminal() {
            return Vec::new();
        }
        match signal {
            RunSignal::TurnStarted if self.phase == RunPhase::Starting => {
                self.phase = RunPhase::Running;
                self.hard_started_at_ms = Some(now_ms);
                self.record_progress(now_ms);
                Vec::new()
            }
            RunSignal::AssistantDelta { non_empty } => {
                if non_empty && matches!(self.phase, RunPhase::Running | RunPhase::Draining) {
                    self.record_progress(now_ms);
                }
                Vec::new()
            }
            RunSignal::ItemStarted { .. } | RunSignal::ItemCompleted { .. } => {
                if matches!(self.phase, RunPhase::Running | RunPhase::Draining) {
                    self.record_progress(now_ms);
                }
                Vec::new()
            }
            RunSignal::AssistantOutputCompleted => {
                if matches!(self.phase, RunPhase::Running | RunPhase::Draining) {
                    self.record_progress(now_ms);
                    self.assistant_completed_at_ms = Some(now_ms);
                    self.phase = RunPhase::Draining;
                }
                Vec::new()
            }
            RunSignal::Terminal { status } => self.finish_from_terminal(status),
            RunSignal::CancelRequested if self.phase == RunPhase::Starting => {
                self.finish(RunOutcome::Cancelled)
            }
            RunSignal::CancelRequested => self.begin_interrupt(now_ms, RunOutcome::Cancelled),
            RunSignal::ChildExited if self.phase == RunPhase::Interrupting => self.finish(
                self.pending_outcome
                    .unwrap_or(RunOutcome::Failed(RunFailureCode::InternalError)),
            ),
            RunSignal::ChildExited => self.finish(RunOutcome::Failed(RunFailureCode::ChildExited)),
            RunSignal::PolicyViolated => {
                self.begin_interrupt(now_ms, RunOutcome::Failed(RunFailureCode::PolicyViolation))
            }
            RunSignal::FailureDetected { code } => {
                self.begin_interrupt(now_ms, RunOutcome::Failed(code))
            }
            _ => Vec::new(),
        }
    }

    pub fn tick(&mut self, now_ms: u64) -> Vec<SupervisorAction> {
        if self.phase.is_terminal() {
            return Vec::new();
        }
        if self.phase == RunPhase::Interrupting {
            let interrupt_at = self.interrupt_started_at_ms.unwrap_or(now_ms);
            if now_ms.saturating_sub(interrupt_at) >= self.policy.interrupt_grace_ms {
                let outcome = self
                    .pending_outcome
                    .unwrap_or(RunOutcome::Failed(RunFailureCode::InternalError));
                let mut actions = vec![SupervisorAction::ForceKill];
                actions.extend(self.finish(outcome));
                return actions;
            }
            return Vec::new();
        }

        if self.hard_started_at_ms.is_some_and(|turn_started| {
            now_ms.saturating_sub(turn_started) >= self.policy.hard_timeout_ms
        }) {
            return self.begin_interrupt(now_ms, RunOutcome::Failed(RunFailureCode::HardTimeout));
        }
        if self.phase == RunPhase::Draining
            && self.assistant_completed_at_ms.is_some_and(|completed_at| {
                now_ms.saturating_sub(completed_at) >= self.policy.terminal_gap_timeout_ms
            })
        {
            return self
                .begin_interrupt(now_ms, RunOutcome::Failed(RunFailureCode::TerminalTimeout));
        }
        if self.phase == RunPhase::Running
            && self.last_progress_at_ms.is_some_and(|last_progress| {
                now_ms.saturating_sub(last_progress) >= self.policy.progress_idle_timeout_ms
            })
        {
            return self
                .begin_interrupt(now_ms, RunOutcome::Failed(RunFailureCode::ProgressTimeout));
        }
        if self.phase == RunPhase::Starting
            && self.request_started_at_ms.is_some_and(|request_started| {
                now_ms.saturating_sub(request_started) >= self.policy.request_timeout_ms
            })
        {
            return self.finish(RunOutcome::Failed(RunFailureCode::RequestTimeout));
        }
        Vec::new()
    }

    fn record_progress(&mut self, now_ms: u64) {
        if self.phase == RunPhase::Draining {
            self.phase = RunPhase::Running;
            self.assistant_completed_at_ms = None;
        }
        self.last_progress_at_ms = Some(now_ms);
    }

    fn begin_interrupt(&mut self, now_ms: u64, outcome: RunOutcome) -> Vec<SupervisorAction> {
        if self.phase == RunPhase::Interrupting {
            return Vec::new();
        }
        self.phase = RunPhase::Interrupting;
        self.interrupt_started_at_ms = Some(now_ms);
        self.pending_outcome = Some(outcome);
        if self.interrupt_sent {
            Vec::new()
        } else {
            self.interrupt_sent = true;
            vec![SupervisorAction::SendInterrupt]
        }
    }

    fn finish_from_terminal(&mut self, status: TerminalStatus) -> Vec<SupervisorAction> {
        let outcome = if let Some(pending) = self.pending_outcome {
            pending
        } else {
            match status {
                TerminalStatus::Completed => RunOutcome::Completed,
                TerminalStatus::Failed | TerminalStatus::Interrupted => {
                    RunOutcome::Failed(RunFailureCode::ProviderError)
                }
            }
        };
        self.finish(outcome)
    }

    fn finish(&mut self, outcome: RunOutcome) -> Vec<SupervisorAction> {
        self.phase = match outcome {
            RunOutcome::Completed => RunPhase::Completed,
            RunOutcome::Failed(_) => RunPhase::Failed,
            RunOutcome::Cancelled => RunPhase::Cancelled,
        };
        vec![SupervisorAction::Finish(outcome)]
    }
}

fn earliest_deadline<const N: usize>(deadlines: [Option<u64>; N]) -> Option<u64> {
    deadlines.into_iter().flatten().min()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::contracts::{ActivityKind, RunSupervisionPolicy};

    fn policy() -> RunSupervisionPolicy {
        RunSupervisionPolicy {
            request_timeout_ms: 20,
            progress_idle_timeout_ms: 60,
            terminal_gap_timeout_ms: 10,
            interrupt_grace_ms: 3,
            hard_timeout_ms: 300,
        }
    }

    #[test]
    fn normal_completion_and_late_terminal_are_idempotent() {
        let mut supervisor = RunSupervisor::new(policy(), 0);
        assert!(supervisor.apply(1, RunSignal::TurnStarted).is_empty());
        assert!(supervisor
            .apply(2, RunSignal::AssistantDelta { non_empty: true })
            .is_empty());
        assert_eq!(supervisor.last_progress_at_ms(), Some(2));
        assert_eq!(
            supervisor.apply(
                3,
                RunSignal::Terminal {
                    status: TerminalStatus::Completed,
                },
            ),
            vec![SupervisorAction::Finish(RunOutcome::Completed)]
        );
        assert!(supervisor
            .apply(
                4,
                RunSignal::Terminal {
                    status: TerminalStatus::Completed,
                },
            )
            .is_empty());
    }

    #[test]
    fn request_progress_terminal_and_hard_timeouts_are_distinct() {
        let mut request = RunSupervisor::new(policy(), 0);
        request.begin_request(0);
        assert_eq!(request.next_deadline_ms(), Some(20));
        assert_eq!(
            request.tick(20),
            vec![SupervisorAction::Finish(RunOutcome::Failed(
                RunFailureCode::RequestTimeout
            ))]
        );

        let mut progress = RunSupervisor::new(policy(), 0);
        progress.apply(1, RunSignal::TurnStarted);
        assert_eq!(progress.next_deadline_ms(), Some(61));
        assert_eq!(progress.tick(61), vec![SupervisorAction::SendInterrupt]);
        assert_eq!(
            progress.tick(64),
            vec![
                SupervisorAction::ForceKill,
                SupervisorAction::Finish(RunOutcome::Failed(RunFailureCode::ProgressTimeout))
            ]
        );

        let mut terminal = RunSupervisor::new(policy(), 0);
        terminal.apply(1, RunSignal::TurnStarted);
        terminal.apply(2, RunSignal::AssistantOutputCompleted);
        assert_eq!(terminal.next_deadline_ms(), Some(12));
        assert_eq!(terminal.tick(12), vec![SupervisorAction::SendInterrupt]);

        let mut hard = RunSupervisor::new(
            RunSupervisionPolicy {
                hard_timeout_ms: 10,
                ..policy()
            },
            0,
        );
        hard.apply(1, RunSignal::TurnStarted);
        hard.apply(2, RunSignal::AssistantOutputCompleted);
        assert_eq!(hard.tick(12), vec![SupervisorAction::SendInterrupt]);
        assert_eq!(
            hard.tick(15),
            vec![
                SupervisorAction::ForceKill,
                SupervisorAction::Finish(RunOutcome::Failed(RunFailureCode::HardTimeout))
            ]
        );
    }

    #[test]
    fn completed_request_and_pre_turn_cancel_have_no_interrupt() {
        let mut completed = RunSupervisor::new(policy(), 0);
        completed.begin_request(0);
        completed.complete_request();
        assert!(completed.tick(20).is_empty());

        let mut cancelled = RunSupervisor::new(policy(), 0);
        cancelled.begin_request(0);
        assert_eq!(
            cancelled.apply(1, RunSignal::CancelRequested),
            vec![SupervisorAction::Finish(RunOutcome::Cancelled)]
        );
    }

    #[test]
    fn only_meaningful_progress_updates_the_clock() {
        let mut supervisor = RunSupervisor::new(policy(), 0);
        supervisor.apply(1, RunSignal::TurnStarted);
        supervisor.apply(5, RunSignal::AssistantDelta { non_empty: false });
        assert_eq!(supervisor.last_progress_at_ms(), Some(1));
        supervisor.apply(
            8,
            RunSignal::ItemStarted {
                kind: ActivityKind::Command,
            },
        );
        assert_eq!(supervisor.last_progress_at_ms(), Some(8));
    }

    #[test]
    fn new_item_after_assistant_completion_returns_to_progress_watch() {
        let mut supervisor = RunSupervisor::new(policy(), 0);
        supervisor.apply(1, RunSignal::TurnStarted);
        supervisor.apply(2, RunSignal::AssistantOutputCompleted);
        supervisor.apply(
            8,
            RunSignal::ItemStarted {
                kind: ActivityKind::Command,
            },
        );
        assert!(supervisor.tick(12).is_empty());
        assert_eq!(supervisor.last_progress_at_ms(), Some(8));
        assert_eq!(supervisor.tick(68), vec![SupervisorAction::SendInterrupt]);
    }

    #[test]
    fn repeated_cancel_interrupts_once_and_preserves_cancelled_outcome() {
        let mut supervisor = RunSupervisor::new(policy(), 0);
        supervisor.apply(1, RunSignal::TurnStarted);
        assert_eq!(
            supervisor.apply(2, RunSignal::CancelRequested),
            vec![SupervisorAction::SendInterrupt]
        );
        assert!(supervisor.apply(2, RunSignal::CancelRequested).is_empty());
        assert_eq!(
            supervisor.apply(
                3,
                RunSignal::Terminal {
                    status: TerminalStatus::Interrupted,
                },
            ),
            vec![SupervisorAction::Finish(RunOutcome::Cancelled)]
        );
    }

    #[test]
    fn protocol_policy_and_child_exit_have_structured_outcomes() {
        for (signal, code, interrupts) in [
            (
                RunSignal::FailureDetected {
                    code: RunFailureCode::ProtocolError,
                },
                RunFailureCode::ProtocolError,
                true,
            ),
            (
                RunSignal::PolicyViolated,
                RunFailureCode::PolicyViolation,
                true,
            ),
            (RunSignal::ChildExited, RunFailureCode::ChildExited, false),
        ] {
            let mut supervisor = RunSupervisor::new(policy(), 0);
            supervisor.apply(1, RunSignal::TurnStarted);
            let actions = supervisor.apply(2, signal);
            if interrupts {
                assert_eq!(actions, vec![SupervisorAction::SendInterrupt]);
                assert_eq!(
                    supervisor.tick(5),
                    vec![
                        SupervisorAction::ForceKill,
                        SupervisorAction::Finish(RunOutcome::Failed(code))
                    ]
                );
            } else {
                assert_eq!(
                    actions,
                    vec![SupervisorAction::Finish(RunOutcome::Failed(code))]
                );
            }
        }
    }

    #[test]
    fn child_exit_while_interrupting_preserves_the_first_failure() {
        let mut supervisor = RunSupervisor::new(policy(), 0);
        supervisor.apply(1, RunSignal::TurnStarted);
        assert_eq!(
            supervisor.apply(
                2,
                RunSignal::FailureDetected {
                    code: RunFailureCode::ProtocolError,
                },
            ),
            vec![SupervisorAction::SendInterrupt]
        );
        assert_eq!(
            supervisor.apply(3, RunSignal::ChildExited),
            vec![SupervisorAction::Finish(RunOutcome::Failed(
                RunFailureCode::ProtocolError
            ))]
        );
    }
}
