use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Purpose {
    Tactical,
    Reasoning,
    StateExtract,
    Diagnostics,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Classification {
    Public,
    Internal,
    Confidential,
    Restricted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccessScope {
    pub principal: String,
    pub scope: String,
    /// None is an explicit scope-wide assertion; never inferred from a task.
    pub task_request: Option<String>,
    pub purposes: BTreeSet<Purpose>,
    pub classification: Classification,
    pub policy_revision: u64,
}

impl AccessScope {
    pub fn permits(&self, access: &AccessRequest<'_>) -> bool {
        access.authorized
            && self.principal == access.principal
            && self.scope == access.scope
            && (self.task_request.is_none() || self.task_request.as_deref() == access.task_request)
            && self.purposes.contains(&access.purpose)
            && self.classification <= access.max_classification
            && self.policy_revision == access.policy_revision
    }

    pub(crate) fn derives_from(&self, parent: &Self) -> bool {
        self.principal == parent.principal
            && self.scope == parent.scope
            && (parent.task_request.is_none() || self.task_request == parent.task_request)
            && self.purposes.is_subset(&parent.purposes)
            && self.classification >= parent.classification
            && self.policy_revision == parent.policy_revision
    }
}

/// `authorized` must come from destination/user policy, never from an extractor.
#[derive(Debug, Clone)]
pub struct AccessRequest<'a> {
    pub principal: &'a str,
    pub scope: &'a str,
    pub task_request: Option<&'a str>,
    pub purpose: Purpose,
    pub max_classification: Classification,
    pub policy_revision: u64,
    pub authorized: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceKey {
    pub id: String,
    pub version: u64,
    pub start: u64,
    pub end: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceRole {
    User,
    Assistant,
    Tool,
    Runtime,
    External,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceRef {
    pub key: SourceKey,
    pub digest: String,
    pub sequence: u64,
    pub recorded_at: i64,
    pub role: SourceRole,
    pub access: AccessScope,
    pub available: bool,
    pub valid_until: Option<i64>,
    /// False for fragments whose surrounding message has not been finalized.
    pub finalized: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Objective,
    Constraint,
    Decision,
    PendingDecision,
    OpenLoop,
    ActiveReferent,
    ProgressRef,
    WorldEntity,
    WorldRelation,
    WorldFocus,
}

impl Kind {
    /// World assertions are owned by the World projection and never mixed into
    /// ordinary continuity extraction or Context candidates.
    pub fn is_world(&self) -> bool {
        matches!(
            self,
            Self::WorldEntity | Self::WorldRelation | Self::WorldFocus
        )
    }

    /// The original seven continuity kinds. Kept explicit so adding World does
    /// not silently change existing behavior.
    pub fn is_continuity(&self) -> bool {
        !self.is_world()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Candidate,
    Active,
    Disputed,
    Resolved,
    Superseded,
    Retracted,
    Invalidated,
    Stale,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Provenance {
    pub model: String,
    pub release: String,
    pub extractor_version: String,
    pub prompt_digest: String,
    pub schema_version: String,
    pub config_digest: String,
    pub runtime_event: Option<(String, u64)>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Assertion {
    pub id: String,
    pub kind: Kind,
    pub semantic_key: String,
    /// Opaque reference to erasable storage; the ledger never contains the value.
    pub payload_ref: String,
    pub access: AccessScope,
    pub evidence: BTreeSet<SourceKey>,
    pub depends_on: BTreeSet<String>,
    /// Every input exposed to the generation, including base messages.
    pub input_dependencies: BTreeSet<SourceKey>,
    pub provenance: Provenance,
    pub observed_at: i64,
    pub effective_at: i64,
    pub recorded_at: i64,
    pub valid_from: i64,
    pub valid_until: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    Assert,
    Activate,
    Dispute,
    Supersede { by: String },
    Retract,
    Invalidate,
    Resolve,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Transition {
    pub id: String,
    pub sequence: u64,
    pub assertion_id: String,
    pub action: Action,
    pub reason_code: String,
    pub evidence: BTreeSet<SourceKey>,
    pub input_dependencies: BTreeSet<SourceKey>,
    pub recorded_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Coverage {
    Applied,
    NoChange,
    Candidate,
    Pending,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StatePatch {
    pub id: String,
    pub base_revision: u64,
    pub input_epoch: u64,
    pub policy_revision: u64,
    pub fence: String,
    pub assertions: Vec<Assertion>,
    pub transitions: Vec<Transition>,
    pub coverage: Vec<(SourceKey, Coverage)>,
}

/// Ephemeral trusted adapter facts. Re-check in the writer transaction at commit.
#[derive(Debug, Clone)]
pub struct CommitContext<'a> {
    pub access: AccessRequest<'a>,
    pub enabled: bool,
    pub now: i64,
    pub live_fence: &'a str,
    pub issued_patch_id: &'a str,
    pub issued_assertion_ids: BTreeSet<String>,
    /// Encoded UTF-8 value bytes in the adapter's erasable payload store.
    pub issued_payload_bytes: BTreeMap<String, usize>,
    pub evidence_allowlist: BTreeSet<SourceKey>,
    pub input_dependencies: BTreeSet<SourceKey>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ledger {
    pub principal: String,
    pub scope: String,
    pub revision: u64,
    pub input_epoch: u64,
    pub policy_revision: u64,
    pub sources: BTreeMap<SourceKey, SourceRef>,
    pub assertions: BTreeMap<String, Assertion>,
    pub transitions: Vec<Transition>,
    /// Source IDs are opaque. Tombstones survive all versions and feature OFF.
    pub tombstones: BTreeSet<String>,
    pub coverage: BTreeMap<SourceKey, Coverage>,
    /// Adapter-computed patch fingerprint; no payload is retained here.
    pub applied_patches: BTreeMap<String, String>,
}

impl Ledger {
    pub fn new(principal: String, scope: String, policy_revision: u64) -> Self {
        Self {
            principal,
            scope,
            policy_revision,
            revision: 0,
            input_epoch: 0,
            sources: BTreeMap::new(),
            assertions: BTreeMap::new(),
            transitions: Vec::new(),
            tombstones: BTreeSet::new(),
            coverage: BTreeMap::new(),
            applied_patches: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Unauthorized,
    Disabled,
    Conflict,
    StalePatch,
    InvalidFence,
    InvalidSource,
    UnknownDependency,
    Cycle,
    InvalidTransition,
    InvalidPatch,
    Limit,
    RequiredMissing,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for Error {}
