//! Pure required-context classification.
//!
//! `Must` means that an item is required to make this dispatch safe; it never changes the
//! authority of the item.  All personal-state candidates remain untrusted data.

use super::source::{Candidate, Requirement};
use sha2::{Digest, Sha256};

/// Why an item must survive context reduction.  This is deliberately derived from the current
/// projection rather than persisted as another copy of the memory record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RequiredReason {
    ActiveState,
    UncertainState,
    PendingSource,
    ExecutionContinuation,
}

/// Ephemeral, generation-scoped manifest of the required portion of a composed context. It holds
/// references only; context text remains in the existing source records and provider envelope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RequiredContextSet {
    pub(crate) item_ids: Vec<String>,
    pub(crate) required_bytes: usize,
    pub(crate) digest: String,
}

impl RequiredContextSet {
    pub(crate) fn from_selected(selected: &[Candidate]) -> Self {
        Self::from_references(selected.iter())
    }

    pub(crate) fn from_references<'a>(selected: impl IntoIterator<Item = &'a Candidate>) -> Self {
        let mut items = selected
            .into_iter()
            .filter(|candidate| candidate.requirement == Requirement::Must)
            .map(|candidate| {
                (
                    candidate.candidate_id.clone(),
                    candidate.source_id.clone(),
                    candidate.source_version,
                    candidate.source_digest.clone(),
                    candidate.cost_bytes,
                )
            })
            .collect::<Vec<_>>();
        items.sort_unstable_by(|left, right| left.0.cmp(&right.0));
        let item_ids = items.iter().map(|item| item.0.clone()).collect::<Vec<_>>();
        let required_bytes = items
            .iter()
            .fold(0_usize, |sum, item| sum.saturating_add(item.4));
        let canonical = items
            .iter()
            .map(|item| format!("{}:{}:{}:{}", item.0, item.1, item.2, item.3))
            .collect::<Vec<_>>()
            .join("\n");
        Self {
            item_ids,
            required_bytes,
            digest: format!("{:x}", Sha256::digest(canonical.as_bytes())),
        }
    }
}

impl RequiredReason {
    pub(crate) const fn requirement(self) -> Requirement {
        Requirement::Must
    }
}

/// Classifies an already scope-filtered candidate. Unknown historical material is not promoted.
pub(crate) fn reason(candidate: &Candidate) -> Option<RequiredReason> {
    match candidate.source_kind.as_str() {
        "personal-pending" => Some(RequiredReason::PendingSource),
        "personal-state" => match json_status(&candidate.content).as_deref() {
            Some("Active") | Some("active") => Some(RequiredReason::ActiveState),
            Some("Candidate") | Some("candidate") | Some("Disputed") | Some("disputed") => {
                Some(RequiredReason::UncertainState)
            }
            _ => None,
        },
        "tool-continuation" | "task-continuation" | "delegation-continuation" => {
            Some(RequiredReason::ExecutionContinuation)
        }
        _ => None,
    }
}

pub(crate) fn requirement(candidate: &Candidate) -> Requirement {
    reason(candidate)
        .map(RequiredReason::requirement)
        .unwrap_or(candidate.requirement)
}

fn json_status(content: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(content)
        .ok()?
        .get("status")?
        .as_str()
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::context::source::Candidate;

    fn personal(status: &str) -> Candidate {
        Candidate::untrusted(
            "id".into(),
            "personal-state",
            vec!["task:a".into()],
            Requirement::Should,
            "source".into(),
            1,
            1,
            format!(r#"{{"status":"{status}"}}"#),
        )
    }

    #[test]
    fn current_personal_states_are_required_without_promoting_unrelated_material() {
        for status in ["Active", "Candidate", "Disputed"] {
            assert_eq!(requirement(&personal(status)), Requirement::Must);
        }
        assert_eq!(requirement(&personal("Expired")), Requirement::Should);
        let reference = Candidate::untrusted(
            "ref".into(),
            "reference",
            vec![],
            Requirement::May,
            "ref".into(),
            1,
            1,
            "unrelated".into(),
        );
        assert_eq!(requirement(&reference), Requirement::May);
    }

    #[test]
    fn required_set_is_stable_and_contains_only_must_items() {
        let mut active = personal("Active");
        active.requirement = requirement(&active);
        let optional = Candidate::untrusted(
            "optional".into(),
            "reference",
            vec![],
            Requirement::May,
            "reference".into(),
            1,
            1,
            "optional".into(),
        );
        let first = RequiredContextSet::from_selected(&[optional.clone(), active.clone()]);
        let second = RequiredContextSet::from_selected(&[active, optional]);
        assert_eq!(first.item_ids, vec!["id"]);
        assert_eq!(first.digest, second.digest);
    }
}
