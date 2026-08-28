pub const SUPERVISOR_VERSION: &str = "mvp2.5-supervisor-v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunPhase {
    Starting,
    Running,
    Interrupting,
    Draining,
    Completed,
    Failed,
    Cancelled,
}

impl RunPhase {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivityKind {
    Command,
    Reasoning,
    Plan,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalStatus {
    Completed,
    Failed,
    Interrupted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunFailureCode {
    UserCancelled,
    AppRestarted,
    ConfigurationError,
    ChildStartFailed,
    RequestTimeout,
    ProgressTimeout,
    TerminalTimeout,
    HardTimeout,
    ChildExited,
    ProtocolError,
    PolicyViolation,
    ProviderError,
    ResponseTooLarge,
    InternalError,
}

impl RunFailureCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UserCancelled => "user-cancelled",
            Self::AppRestarted => "app-restarted",
            Self::ConfigurationError => "configuration-error",
            Self::ChildStartFailed => "child-start-failed",
            Self::RequestTimeout => "request-timeout",
            Self::ProgressTimeout => "progress-timeout",
            Self::TerminalTimeout => "terminal-timeout",
            Self::HardTimeout => "hard-timeout",
            Self::ChildExited => "child-exited",
            Self::ProtocolError => "protocol-error",
            Self::PolicyViolation => "policy-violation",
            Self::ProviderError => "provider-error",
            Self::ResponseTooLarge => "response-too-large",
            Self::InternalError => "internal-error",
        }
    }

    pub const fn recovery(self) -> &'static str {
        match self {
            Self::ConfigurationError => "Check the Codex workspace and Settings, then retry.",
            Self::ChildStartFailed => {
                "Check the Codex installation and bundled runtime, then retry."
            }
            Self::RequestTimeout => "Restart the Codex runtime and retry.",
            Self::ProgressTimeout => {
                "Retry the run. For long tasks, review the coding route timeout."
            }
            Self::TerminalTimeout => "Restart the Codex runtime and retry.",
            Self::HardTimeout => {
                "Increase the coding timeout only if the task requires it, then retry."
            }
            Self::ChildExited => "Check the Codex runtime status and retry.",
            Self::ProtocolError => "Update or reinstall the Codex runtime, then retry.",
            Self::PolicyViolation => {
                "The requested operation is unavailable on the read-only coding route."
            }
            Self::ProviderError => "Review the bounded error and retry.",
            Self::ResponseTooLarge => "Ask for a shorter response and retry.",
            Self::UserCancelled => "Start a new run when you are ready.",
            Self::AppRestarted | Self::InternalError => {
                "Restart the application and retry the run."
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunOutcome {
    Completed,
    Failed(RunFailureCode),
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunSignal {
    TurnStarted,
    AssistantDelta { non_empty: bool },
    ItemStarted { kind: ActivityKind },
    ItemCompleted { kind: ActivityKind },
    AssistantOutputCompleted,
    Terminal { status: TerminalStatus },
    CancelRequested,
    ChildExited,
    PolicyViolated,
    FailureDetected { code: RunFailureCode },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SupervisorAction {
    SendInterrupt,
    ForceKill,
    Finish(RunOutcome),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunSupervisionPolicy {
    pub request_timeout_ms: u64,
    pub progress_idle_timeout_ms: u64,
    pub terminal_gap_timeout_ms: u64,
    pub interrupt_grace_ms: u64,
    pub hard_timeout_ms: u64,
}

impl RunSupervisionPolicy {
    pub fn for_route(hard_timeout_ms: u64) -> Result<Self, String> {
        if !(1_000..=300_000).contains(&hard_timeout_ms) {
            return Err("Invalid Supervisor hard timeout".to_string());
        }
        Ok(Self {
            request_timeout_ms: 20_000,
            progress_idle_timeout_ms: 60_000,
            terminal_gap_timeout_ms: 10_000,
            interrupt_grace_ms: 3_000,
            hard_timeout_ms,
        })
    }
}
