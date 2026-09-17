use sha2::{Digest, Sha256};

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Requirement {
    Must,
    Should,
    May,
}

impl Requirement {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Must => "must",
            Self::Should => "should",
            Self::May => "may",
        }
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Authority {
    TrustedPolicy,
    Instruction,
    UntrustedData,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Placement {
    Base,
    ToolSchema,
    Reference,
}

impl Placement {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Base => "base",
            Self::ToolSchema => "tool-schema",
            Self::Reference => "reference",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Candidate {
    pub(crate) candidate_id: String,
    pub(crate) source_kind: String,
    #[allow(dead_code)]
    pub(crate) scope_refs: Vec<String>,
    pub(crate) requirement: Requirement,
    pub(crate) authority: Authority,
    pub(crate) source_id: String,
    pub(crate) source_version: u64,
    pub(crate) source_digest: String,
    pub(crate) placement: Placement,
    pub(crate) cost_bytes: usize,
    pub(crate) utility: u16,
    pub(crate) content: String,
}

impl Candidate {
    #[allow(clippy::too_many_arguments)] // Candidate construction keeps every trust field explicit.
    pub(crate) fn untrusted(
        candidate_id: String,
        source_kind: &str,
        scope_refs: Vec<String>,
        requirement: Requirement,
        source_id: String,
        source_version: u64,
        utility: u16,
        content: String,
    ) -> Self {
        let source_digest = format!("{:x}", Sha256::digest(content.as_bytes()));
        Self {
            candidate_id,
            source_kind: source_kind.into(),
            scope_refs,
            requirement,
            authority: Authority::UntrustedData,
            source_id,
            source_version,
            source_digest,
            placement: Placement::Base,
            cost_bytes: content.len(),
            utility,
            content,
        }
    }
}
