//! Role-routing context boundary.
//!
//! A routing step receives the root's initial must-context plus each still-valid amendment once.
//! This is intentionally independent of the legacy broker so existing callers keep their exact
//! selection behaviour until they opt into a role projection.
use super::source::{Candidate, Requirement};
use std::collections::HashSet;

#[derive(Debug, Clone)]
pub(crate) struct RoleProjectionInput {
    pub(crate) root_scope: String,
    pub(crate) initial: Vec<Candidate>,
    pub(crate) amendments: Vec<Candidate>,
    pub(crate) revoked_source_ids: HashSet<String>,
}

pub(crate) fn project(input: RoleProjectionInput) -> Result<Vec<Candidate>, String> {
    let mut seen = HashSet::new();
    let mut projected = Vec::new();
    for candidate in input.initial.into_iter().chain(input.amendments) {
        if input.revoked_source_ids.contains(&candidate.source_id) {
            continue;
        }
        if candidate.requirement != Requirement::Must {
            continue;
        }
        if !candidate.scope_refs.is_empty()
            && !candidate
                .scope_refs
                .iter()
                .any(|scope| scope == &input.root_scope)
        {
            return Err(format!(
                "role_projection_scope_widening:{}",
                candidate.candidate_id
            ));
        }
        if seen.insert(candidate.candidate_id.clone()) {
            projected.push(candidate);
        }
    }
    if projected.is_empty() {
        return Err("role_projection_missing_required_context".into());
    }
    Ok(projected)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn must(id: &str, source: &str, scopes: Vec<&str>) -> Candidate {
        Candidate::untrusted(
            id.into(),
            "personal-state",
            scopes.into_iter().map(str::to_owned).collect(),
            Requirement::Must,
            source.into(),
            1,
            1,
            format!("must:{id}"),
        )
    }

    #[test]
    fn rr_07_amendment_present_once() {
        let amendment = must("amendment", "source-a", vec!["scope-a"]);
        let projected = project(RoleProjectionInput {
            root_scope: "scope-a".into(),
            initial: vec![must("initial", "source-i", vec!["scope-a"])],
            amendments: vec![amendment.clone(), amendment],
            revoked_source_ids: HashSet::new(),
        })
        .expect("projection");
        assert_eq!(projected.len(), 2);
        assert_eq!(projected[1].candidate_id, "amendment");
    }

    #[test]
    fn rr_07_scope_no_widening_and_revocation_are_rejected() {
        let invalid = project(RoleProjectionInput {
            root_scope: "scope-a".into(),
            initial: vec![must("foreign", "source-f", vec!["scope-b"])],
            amendments: vec![],
            revoked_source_ids: HashSet::new(),
        });
        assert!(invalid
            .expect_err("foreign scope")
            .contains("role_projection_scope_widening"));
        let revoked = project(RoleProjectionInput {
            root_scope: "scope-a".into(),
            initial: vec![must("revoked", "source-r", vec!["scope-a"])],
            amendments: vec![],
            revoked_source_ids: HashSet::from(["source-r".into()]),
        });
        assert_eq!(
            revoked.expect_err("revoked source"),
            "role_projection_missing_required_context"
        );
    }
}
