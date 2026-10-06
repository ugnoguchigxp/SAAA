//! Check registry. A check returns evidence; it never decides a capability state.
use super::contract::{Capability, Evidence, Importance, Outcome, Reason, Tier};
use crate::AppState;
use futures_util::future::BoxFuture;
use std::time::Duration;

mod activity;
mod harness;
mod memory;
mod providers;
mod storage;
mod voice;

pub(super) type CheckFuture<'a> = BoxFuture<'a, Vec<Evidence>>;

pub(super) struct CheckSpec {
    pub(super) id: &'static str,
    /// Included in a quick run: no provider request and no process spawn.
    pub(super) quick: bool,
    pub(super) timeout: Duration,
    /// Applied to evidence that carries no expiry of its own.
    pub(super) ttl: Option<Duration>,
    pub(super) capabilities: &'static [Capability],
    /// Route recorded when the check times out.
    pub(super) timeout_route: &'static str,
    pub(super) run: for<'a> fn(&'a AppState) -> CheckFuture<'a>,
}

const LIVE_TTL: Duration = Duration::from_secs(30 * 60);
const CATALOG_TTL: Duration = Duration::from_secs(10 * 60);

pub(super) const REGISTRY: &[CheckSpec] = &[
    CheckSpec {
        id: "storage",
        quick: true,
        timeout: Duration::from_secs(10),
        ttl: None,
        capabilities: &[Capability::Storage],
        timeout_route: "core",
        run: storage::run,
    },
    CheckSpec {
        id: "activity",
        quick: true,
        timeout: Duration::from_secs(10),
        ttl: None,
        capabilities: &[
            Capability::Conversation,
            Capability::VoiceListen,
            Capability::VoiceSpeak,
            Capability::Coding,
        ],
        timeout_route: "core",
        run: activity::run,
    },
    CheckSpec {
        id: "memory",
        quick: true,
        timeout: Duration::from_secs(15),
        ttl: None,
        capabilities: &[Capability::Memory],
        timeout_route: "core",
        run: memory::run,
    },
    CheckSpec {
        id: "voice.echo",
        quick: true,
        timeout: Duration::from_secs(10),
        ttl: None,
        capabilities: &[Capability::VoiceEcho],
        timeout_route: "core",
        run: voice::run,
    },
    CheckSpec {
        id: "providers.config",
        quick: true,
        timeout: Duration::from_secs(10),
        ttl: None,
        capabilities: &[
            Capability::Conversation,
            Capability::VoiceListen,
            Capability::VoiceSpeak,
        ],
        timeout_route: "providers",
        run: providers::config,
    },
    CheckSpec {
        id: "harness.catalog",
        quick: true,
        timeout: Duration::from_secs(20),
        ttl: Some(CATALOG_TTL),
        capabilities: &[
            Capability::Conversation,
            Capability::VoiceListen,
            Capability::VoiceSpeak,
            Capability::Memory,
        ],
        timeout_route: "larm",
        run: harness::catalog,
    },
    CheckSpec {
        id: "providers.probe",
        quick: false,
        timeout: Duration::from_secs(75),
        ttl: Some(LIVE_TTL),
        capabilities: &[
            Capability::Conversation,
            Capability::VoiceListen,
            Capability::VoiceSpeak,
        ],
        timeout_route: "providers",
        run: providers::probe,
    },
    CheckSpec {
        id: "codex.sdk",
        quick: false,
        timeout: Duration::from_secs(15),
        ttl: Some(LIVE_TTL),
        capabilities: &[Capability::Conversation],
        timeout_route: "codex",
        run: providers::codex,
    },
    CheckSpec {
        id: "harness.session",
        quick: false,
        timeout: Duration::from_secs(75),
        ttl: Some(LIVE_TTL),
        capabilities: &[
            Capability::Conversation,
            Capability::VoiceListen,
            Capability::VoiceSpeak,
            Capability::Memory,
        ],
        timeout_route: "larm",
        run: harness::session,
    },
];

/// Evidence builder. `Required`, `core` route, observed now.
pub(super) fn evidence(
    source: &str,
    capability: Capability,
    tier: Tier,
    outcome: Outcome,
    reason: Reason,
) -> Evidence {
    Evidence {
        source: source.to_string(),
        capability,
        route: "core".to_string(),
        tier,
        importance: Importance::Required,
        outcome,
        reason,
        subject: None,
        detail: None,
        latency_ms: None,
        observed_at: super::contract::now_ms(),
        expires_at: None,
    }
}

pub(super) fn pass(source: &str, capability: Capability, tier: Tier) -> Evidence {
    evidence(source, capability, tier, Outcome::Pass, Reason::Ok)
}

impl Evidence {
    pub(super) fn route(mut self, route: impl Into<String>) -> Self {
        self.route = route.into();
        self
    }

    pub(super) fn advisory(mut self) -> Self {
        self.importance = Importance::Advisory;
        self
    }

    pub(super) fn subject(mut self, subject: &str) -> Self {
        self.subject = Some(crate::redact_runtime_text(subject));
        self
    }

    pub(super) fn detail(mut self, detail: &str) -> Self {
        let redacted = crate::redact_runtime_text(detail.trim());
        let short: String = redacted.chars().take(160).collect();
        self.detail = (!short.is_empty()).then_some(short);
        self
    }

    pub(super) fn latency(mut self, latency_ms: Option<u64>) -> Self {
        self.latency_ms = latency_ms;
        self
    }
}

/// Maps an error text to a typed cause. Wording never reaches the UI from here.
pub(super) fn classify_failure(text: &str) -> Reason {
    let lower = text.to_ascii_lowercase();
    let has = |needles: &[&str]| needles.iter().any(|needle| lower.contains(needle));
    if has(&[
        "unauthor",
        "forbidden",
        "api key",
        "api_key",
        "credential",
        "authenticat",
        "http 401",
        "http 403",
        "status 401",
        "status 403",
    ]) {
        Reason::AuthFailed
    } else if has(&["timed out", "timeout", "deadline"]) {
        Reason::Timeout
    } else if has(&[
        "connect",
        "refused",
        "dns",
        "resolve",
        "unreachable",
        "network",
        "no route",
        "tcp",
    ]) {
        Reason::Unreachable
    } else if has(&["not advertised"]) {
        Reason::NotAdvertised
    } else if has(&["not ready", "not-ready", "warming"]) {
        Reason::NotReady
    } else {
        Reason::Unavailable
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failures_map_to_typed_reasons() {
        assert_eq!(
            classify_failure("HTTP 401 Unauthorized"),
            Reason::AuthFailed
        );
        assert_eq!(
            classify_failure("request timed out after 8s"),
            Reason::Timeout
        );
        assert_eq!(classify_failure("connection refused"), Reason::Unreachable);
        assert_eq!(classify_failure("model exploded"), Reason::Unavailable);
        // Mentioning "token" or a number is not an authentication failure.
        assert_eq!(
            classify_failure(
                "Provider reached the output token limit; the response is incomplete."
            ),
            Reason::Unavailable
        );
        assert_eq!(classify_failure("returned 4013 items"), Reason::Unavailable);
    }

    #[test]
    fn registry_ids_are_unique_and_every_capability_is_covered() {
        let mut ids: Vec<_> = REGISTRY.iter().map(|spec| spec.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), REGISTRY.len());
        for capability in Capability::ALL {
            if capability == Capability::Coding {
                continue;
            }
            assert!(
                REGISTRY
                    .iter()
                    .any(|spec| spec.quick && spec.capabilities.contains(&capability)),
                "{capability:?} has no quick check"
            );
        }
    }

    #[test]
    fn detail_is_redacted_and_bounded() {
        let long = "x".repeat(400);
        let built = pass("t", Capability::Storage, Tier::Static).detail(&long);
        assert_eq!(built.detail.unwrap().chars().count(), 160);
    }
}
