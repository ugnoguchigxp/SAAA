use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// A user-facing function whose availability the diagnosis answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
#[ts(rename = "DiagnosisCapability")]
pub(crate) enum Capability {
    Storage,
    Conversation,
    VoiceListen,
    VoiceSpeak,
    VoiceEcho,
    Memory,
    Coding,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Weight {
    /// An outage makes SAAA unusable.
    Core,
    /// An outage removes a feature; an unproven state still blocks a green verdict.
    Standard,
    /// Reported, but never decides the overall verdict by being unproven.
    Optional,
}

impl Capability {
    pub(crate) const ALL: [Capability; 7] = [
        Capability::Storage,
        Capability::Conversation,
        Capability::VoiceListen,
        Capability::VoiceSpeak,
        Capability::VoiceEcho,
        Capability::Memory,
        Capability::Coding,
    ];

    pub(crate) fn weight(self) -> Weight {
        match self {
            Self::Storage | Self::Conversation => Weight::Core,
            Self::Coding => Weight::Optional,
            _ => Weight::Standard,
        }
    }

    /// Ready needs a fresh pass from a probe or from real use, not only configuration.
    pub(crate) fn needs_live_proof(self) -> bool {
        matches!(
            self,
            Self::Conversation | Self::VoiceListen | Self::VoiceSpeak | Self::Coding
        )
    }
}

/// How the evidence was obtained.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
#[ts(rename = "DiagnosisTier")]
pub(crate) enum Tier {
    /// Configuration, catalog or contract self-test. Nothing was exercised.
    Static,
    /// Derived from recorded real activity.
    Observed,
    /// The capability itself was exercised just now.
    Probe,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
#[ts(rename = "DiagnosisOutcome")]
pub(crate) enum Outcome {
    Pass,
    Degraded,
    Fail,
    /// No usable evidence: timed out, expired or never measured.
    Unverified,
    /// The user turned this path off. It is not a problem.
    Disabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
#[ts(rename = "DiagnosisImportance")]
pub(crate) enum Importance {
    Required,
    /// Can lower a capability to degraded, never to unavailable.
    Advisory,
}

/// Typed cause. The frontend owns the wording.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
#[ts(rename = "DiagnosisReason")]
pub(crate) enum Reason {
    Ok,
    Disabled,
    NotConfigured,
    NotProven,
    NotObserved,
    Expired,
    Timeout,
    Unreachable,
    AuthFailed,
    NotAdvertised,
    NotReady,
    Unavailable,
    SchemaMismatch,
    CapacityHigh,
    RecentFailure,
    CaptureFailed,
    AecInactive,
    EchoLeak,
    SpeechSuppressed,
    Internal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename = "DiagnosisEvidence")]
pub(crate) struct Evidence {
    /// Stable machine id, such as `larm.asr` or `runs.conversation`.
    pub(crate) source: String,
    pub(crate) capability: Capability,
    /// `core` evidence must all hold. Other routes are alternatives; one working route is enough.
    pub(crate) route: String,
    pub(crate) tier: Tier,
    pub(crate) importance: Importance,
    pub(crate) outcome: Outcome,
    pub(crate) reason: Reason,
    /// User-configured name, such as a provider label.
    pub(crate) subject: Option<String>,
    /// Short redacted note. Never a URL or token.
    pub(crate) detail: Option<String>,
    #[ts(type = "number | null")]
    pub(crate) latency_ms: Option<u64>,
    #[ts(type = "number")]
    pub(crate) observed_at: u64,
    #[ts(type = "number | null")]
    pub(crate) expires_at: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
#[ts(rename = "DiagnosisState")]
pub(crate) enum CapabilityState {
    Ready,
    Degraded,
    Unavailable,
    Unverified,
    Disabled,
}

impl CapabilityState {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Degraded => "degraded",
            Self::Unavailable => "unavailable",
            Self::Unverified => "unverified",
            Self::Disabled => "disabled",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
#[ts(rename = "DiagnosisAction")]
pub(crate) enum Action {
    OpenSettings,
    Retest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename = "DiagnosisCapabilityReport")]
pub(crate) struct CapabilityReport {
    pub(crate) capability: Capability,
    pub(crate) state: CapabilityState,
    pub(crate) reason: Reason,
    /// Newest time a probe or real use proved this capability.
    #[ts(type = "number | null")]
    pub(crate) verified_at: Option<u64>,
    pub(crate) optional: bool,
    pub(crate) actions: Vec<Action>,
    pub(crate) evidence: Vec<Evidence>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DiagnosisReport {
    pub(crate) schema_version: u32,
    #[ts(type = "number")]
    pub(crate) revision: u64,
    #[ts(type = "number")]
    pub(crate) started_at: u64,
    #[ts(type = "number | null")]
    pub(crate) finished_at: Option<u64>,
    pub(crate) running: bool,
    pub(crate) overall: CapabilityState,
    pub(crate) capabilities: Vec<CapabilityReport>,
}

pub(crate) const SCHEMA_VERSION: u32 = 2;

/// What a run covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub(crate) enum DiagnosisScope {
    /// Configuration, recorded activity and cheap local checks. Sends no provider request.
    Quick,
    /// Quick plus live probes of every provider and the LARM session.
    Full,
    /// Every check that contributes to one capability, probes included.
    Capability { capability: Capability },
}

pub(crate) fn typescript_bindings() -> String {
    [
        Capability::decl(&Default::default()),
        Tier::decl(&Default::default()),
        Outcome::decl(&Default::default()),
        Importance::decl(&Default::default()),
        Reason::decl(&Default::default()),
        Evidence::decl(&Default::default()),
        CapabilityState::decl(&Default::default()),
        Action::decl(&Default::default()),
        CapabilityReport::decl(&Default::default()),
        DiagnosisReport::decl(&Default::default()),
        DiagnosisScope::decl(&Default::default()),
    ]
    .map(|decl| format!("export {decl}"))
    .join("\n")
}

pub(crate) fn typescript_file() -> String {
    format!(
        "// Generated from src-tauri/src/diagnosis/contract.rs. Do not edit.\n{}\n",
        typescript_bindings()
    )
}

pub(crate) fn now_ms() -> u64 {
    crate::now_iso().parse().unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enums_serialize_kebab_case() {
        assert_eq!(
            serde_json::to_string(&Capability::VoiceListen).unwrap(),
            "\"voice-listen\""
        );
        assert_eq!(
            serde_json::to_string(&Reason::NotProven).unwrap(),
            "\"not-proven\""
        );
        assert_eq!(
            serde_json::to_string(&DiagnosisScope::Capability {
                capability: Capability::VoiceEcho
            })
            .unwrap(),
            "{\"kind\":\"capability\",\"capability\":\"voice-echo\"}"
        );
    }
}
