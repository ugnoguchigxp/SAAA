//! Capability assessment for externally supplied evidence (R8).
//!
//! The current contextStill memory-recall-v1 contract has no stable resource
//! id, no immutable revision, no revalidation, no deletion contract and no
//! scope proof. Its results are therefore classified `transient_only`: useful
//! for one turn at most, never a persistent World source. This module never
//! relaxes the existing parser and never returns `persistent_eligible` in
//! production.
#![allow(dead_code)]

use serde::Serialize;

pub(crate) const CONTEXT_STILL_PROVIDER: &str = "context_still";
pub(crate) const MEMORY_RECALL_V1_CONTRACT: &str = "memory_recall_v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum EvidenceEligibility {
    TransientOnly,
    PersistentEligible,
}

#[allow(clippy::enum_variant_names)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum EvidenceIneligibility {
    MissingStableId,
    MissingRevision,
    MissingRevalidation,
    MissingDeletionContract,
    MissingScopeProof,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct EvidenceAssessment {
    pub(crate) provider: &'static str,
    pub(crate) contract: &'static str,
    pub(crate) eligibility: EvidenceEligibility,
    pub(crate) reasons: Vec<EvidenceIneligibility>,
}

/// The fixed assessment of the current contract. The five reasons are always
/// returned in definition order; a caller cannot pass a flag to upgrade the
/// result.
pub(crate) fn assess_contextstill_v1() -> EvidenceAssessment {
    EvidenceAssessment {
        provider: CONTEXT_STILL_PROVIDER,
        contract: MEMORY_RECALL_V1_CONTRACT,
        eligibility: EvidenceEligibility::TransientOnly,
        reasons: vec![
            EvidenceIneligibility::MissingStableId,
            EvidenceIneligibility::MissingRevision,
            EvidenceIneligibility::MissingRevalidation,
            EvidenceIneligibility::MissingDeletionContract,
            EvidenceIneligibility::MissingScopeProof,
        ],
    }
}

/// Reject a contract this build cannot assess at all. The contract name is
/// never used to grant eligibility.
pub(crate) fn assess_contract(
    contract: &str,
) -> Result<EvidenceAssessment, saaa_personal_state_core::world::runtime_frame::FrameError> {
    match contract {
        MEMORY_RECALL_V1_CONTRACT => Ok(assess_contextstill_v1()),
        _ => Err(
            saaa_personal_state_core::world::runtime_frame::FrameError::UnsupportedEvidenceContract,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::typed_recall::{
        parse_call_tool_result, TypedMemoryType, TypedRecallContractError,
    };
    use serde_json::{json, Value};

    const RULE_FIXTURE: &str =
        include_str!("../../../../tests/fixtures/memory-recall-v1/rule.json");

    fn call_result(text: &str) -> Value {
        json!({"content": [{"type": "text", "text": text}]})
    }

    #[test]
    fn m2_20_current_contract_is_transient_only_with_five_reasons() {
        let assessment = assess_contextstill_v1();
        assert_eq!(assessment.eligibility, EvidenceEligibility::TransientOnly);
        assert_eq!(assessment.reasons.len(), 5);
        assert_eq!(
            assessment.reasons[0],
            EvidenceIneligibility::MissingStableId
        );
        assert_eq!(
            assessment.reasons[4],
            EvidenceIneligibility::MissingScopeProof
        );
    }

    #[test]
    fn m2_20_unknown_contract_is_rejected() {
        assert_eq!(
            assess_contract("some_future_contract").unwrap_err(),
            saaa_personal_state_core::world::runtime_frame::FrameError::UnsupportedEvidenceContract
        );
    }

    #[test]
    fn m2_20_valid_recall_is_still_transient_only() {
        let parsed = parse_call_tool_result(TypedMemoryType::Rule, &call_result(RULE_FIXTURE))
            .expect("valid fixture parses");
        assert!(!parsed.is_empty());
        assert_eq!(
            assess_contextstill_v1().eligibility,
            EvidenceEligibility::TransientOnly
        );
    }

    #[test]
    fn m2_20_unknown_sourceref_is_rejected_by_the_existing_parser() {
        let mut unknown: Value = serde_json::from_str(RULE_FIXTURE).expect("fixture parses");
        unknown["sourceRef"] = json!("forbidden");
        assert_eq!(
            parse_call_tool_result(TypedMemoryType::Rule, &call_result(&unknown.to_string())),
            Err(TypedRecallContractError::InvalidResponse)
        );
    }
}
