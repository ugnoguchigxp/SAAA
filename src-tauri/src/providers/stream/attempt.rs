use super::super::session_store::mark_provider_output_started;
use crate::AppState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProviderFailureKind {
    Authentication,
    Contract,
    Protocol,
    RequestTooLarge,
    RequiredContextOverflow,
    ContextScopeChanged,
    RequiredContextUnavailable,
    Policy,
    Capacity,
    Unavailable,
    Upstream,
    Connect,
    ResponseInterrupted,
    Network,
    Timeout,
    AllocationLost,
    PartialOutput,
    ClientDisconnected,
    Cancelled,
    Internal,
}

impl ProviderFailureKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Authentication => "authentication",
            Self::Contract => "contract",
            Self::Protocol => "protocol",
            Self::RequestTooLarge => "request-too-large",
            Self::RequiredContextOverflow => "required-context-overflow",
            Self::ContextScopeChanged => "context-scope-changed",
            Self::RequiredContextUnavailable => "required-context-unavailable",
            Self::Policy => "policy",
            Self::Capacity => "capacity",
            Self::Unavailable => "unavailable",
            Self::Upstream => "upstream",
            Self::Connect => "connect",
            Self::ResponseInterrupted => "response-interrupted",
            Self::Network => "network",
            Self::Timeout => "timeout",
            Self::AllocationLost => "allocation-lost",
            Self::PartialOutput => "partial-output",
            Self::ClientDisconnected => "client-disconnected",
            Self::Cancelled => "cancelled",
            Self::Internal => "internal",
        }
    }

    pub(crate) fn public_message(self) -> BoundedProviderMessage {
        let message = match self {
            Self::Authentication => {
                "Provider authentication failed. Check the configured credential."
            }
            Self::Contract => "Provider settings or request contract are invalid.",
            Self::Protocol => "Provider returned an invalid or incomplete response.",
            Self::RequestTooLarge => "Provider request or response exceeded the configured limit.",
            Self::RequiredContextOverflow => {
                "Required context does not fit this provider. Narrow the task scope or correct the saved memory."
            }
            Self::ContextScopeChanged => {
                "Context scope changed before the action could run. Narrow the task scope and try again."
            }
            Self::RequiredContextUnavailable => {
                "A required source is not available before the action could run. Review or correct the saved memory."
            }
            Self::Policy => "Provider policy rejected the request.",
            Self::Capacity => "Provider capacity is currently exhausted.",
            Self::Unavailable => "Provider is currently unavailable.",
            Self::Upstream => "Provider could not complete the upstream request.",
            Self::Connect => "SAAA could not connect to the provider.",
            Self::ResponseInterrupted => {
                "Provider connection ended after the response had started."
            }
            Self::Network => "Provider connection ended before the response completed.",
            Self::Timeout => "Provider request reached its timeout.",
            Self::AllocationLost => "The selected local runtime allocation is no longer available.",
            Self::PartialOutput => {
                "Provider reached the output token limit; the response is incomplete."
            }
            Self::ClientDisconnected => "The response consumer disconnected.",
            Self::Cancelled => "Provider execution was cancelled.",
            Self::Internal => "SAAA could not complete the provider attempt.",
        };
        BoundedProviderMessage(message)
    }

    pub(crate) fn persistence_str(self) -> &'static str {
        match self {
            Self::Connect | Self::ResponseInterrupted => "network",
            _ => self.as_str(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BoundedProviderMessage(&'static str);

impl BoundedProviderMessage {
    /// Only locally authored static diagnostics; never provider response bodies.
    pub(crate) fn from_static_diagnostic(message: &'static str) -> Self {
        Self(message)
    }
    pub(crate) fn as_str(self) -> &'static str {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CleanupOutcome {
    NotApplicable,
    NotStarted,
    Pending,
    Released,
    ReleaseFailed { kind: &'static str },
    DynamicLanDeferredToTtl { kind: &'static str },
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ProviderAttemptOutcome {
    Completed {
        content: String,
        cleanup: CleanupOutcome,
    },
    Cancelled {
        output_started: bool,
        cleanup: CleanupOutcome,
    },
    Failed {
        kind: ProviderFailureKind,
        public_message: BoundedProviderMessage,
        output_started: bool,
        cleanup: CleanupOutcome,
    },
}

impl ProviderAttemptOutcome {
    pub(crate) fn with_cleanup(self, cleanup: CleanupOutcome) -> Self {
        match self {
            Self::Completed { content, .. } => Self::Completed { content, cleanup },
            Self::Cancelled { output_started, .. } => Self::Cancelled {
                output_started,
                cleanup,
            },
            Self::Failed {
                kind,
                public_message,
                output_started,
                ..
            } => Self::Failed {
                kind,
                public_message,
                output_started,
                cleanup,
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProviderAttemptError {
    Cancelled {
        output_started: bool,
    },
    Failed {
        kind: ProviderFailureKind,
        output_started: bool,
    },
}

impl ProviderAttemptError {
    pub(crate) fn failed(kind: ProviderFailureKind, output_started: bool) -> Self {
        Self::Failed {
            kind,
            output_started,
        }
    }
}

pub(crate) fn provider_failure_from_dynamic_lan(
    kind: crate::providers::dynamic_lan::ErrorKind,
) -> ProviderFailureKind {
    use crate::providers::dynamic_lan::ErrorKind as DynamicLan;
    match kind {
        DynamicLan::Authentication => ProviderFailureKind::Authentication,
        DynamicLan::Contract => ProviderFailureKind::Contract,
        DynamicLan::Capacity => ProviderFailureKind::Capacity,
        DynamicLan::Unavailable => ProviderFailureKind::Unavailable,
        DynamicLan::Upstream => ProviderFailureKind::Upstream,
        DynamicLan::Network => ProviderFailureKind::Network,
        DynamicLan::Timeout => ProviderFailureKind::Timeout,
        DynamicLan::StaleConnection => ProviderFailureKind::AllocationLost,
        DynamicLan::Cancelled => ProviderFailureKind::Cancelled,
        DynamicLan::Internal => ProviderFailureKind::Internal,
    }
}

#[derive(Clone, Copy)]
pub(crate) struct ProviderOutputPersistence<'a> {
    pub(crate) state: &'a AppState,
    pub(crate) session_id: &'a str,
    pub(crate) world: Option<&'a crate::runtime::context::world::turn::WorldLive>,
}

impl ProviderOutputPersistence<'_> {
    pub(crate) fn mark_started(self) -> Result<(), ProviderAttemptError> {
        mark_provider_output_started(self.state, self.session_id)
            .map_err(|_| ProviderAttemptError::failed(ProviderFailureKind::Internal, false))
    }

    pub(crate) fn begin_context_generation(
        self,
        run_id: &str,
        purpose: &str,
        request_payload: &[u8],
        envelope_payload: &[u8],
        current_instruction_count: usize,
    ) -> Result<crate::runtime::context::generation::GenerationHandle, ProviderFailureKind> {
        crate::runtime::context::generation::begin(
            self.state,
            crate::runtime::context::generation::BeginGeneration {
                run_id,
                provider_session_id: Some(self.session_id),
                provider_id: None,
                purpose,
                request_payload,
                envelope_payload,
                current_instruction_count,
            },
        )
        .map_err(|_| ProviderFailureKind::Internal)
    }

    pub(crate) fn record_transport_event(
        self,
        request_id: &str,
        stage: &str,
        transport: &str,
        model: &str,
        endpoint: &str,
        detail: Option<&str>,
    ) {
        let _ = self.state.sqlite_writer.write(|connection| {
            if stage == "prepared" {
                connection
                    .execute(
                        "UPDATE provider_sessions SET request_id=?1,updated_at=?2 WHERE id=?3 AND status='running'",
                        rusqlite::params![request_id, crate::now_iso(), self.session_id],
                    )
                    .map_err(crate::database_error)?;
            }
            connection
                .execute(
                    "INSERT INTO provider_transport_events(id,provider_session_id,request_id,stage,transport,model,endpoint,detail,recorded_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
                    rusqlite::params![crate::new_id("transport-event"), self.session_id, request_id, stage, transport, model, endpoint, detail, crate::now_iso()],
                )
                .map_err(crate::database_error)?;
            Ok(())
        });
    }

    pub(crate) fn bind_transport(self, allocation_id: Option<&str>) {
        let route_id = if allocation_id.is_some() {
            "allocated-http"
        } else {
            "direct-http"
        };
        let _ = self.state.sqlite_writer.write(|connection| {
            connection
                .execute(
                    "UPDATE provider_sessions SET route_id=?1,allocation_id=?2,updated_at=?3 WHERE id=?4 AND status='running'",
                    rusqlite::params![route_id, allocation_id, crate::now_iso(), self.session_id],
                )
                .map_err(crate::database_error)?;
            Ok(())
        });
    }
}
