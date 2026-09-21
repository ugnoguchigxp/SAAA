//! Role context projection for role routing.
//!
//! Each step receives only the conditions it needs: the root's initial input condition plus the
//! accepted amendments, in order. The projection never widens the allowed scope, never resurrects
//! a revoked source, and never lets an amendment appear twice. A missing required condition is an
//! error instead of an empty projection that silently drops a constraint.
#![allow(dead_code)]

use std::collections::HashSet;

/// One context condition (the root input or an accepted amendment).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ContextCondition {
    /// Stable source identifier, for example the input message id.
    pub(crate) source_id: String,
    /// Scope key the condition belongs to.
    pub(crate) scope_key: String,
    /// Digest of the condition text; used for lineage, never persisted as raw text.
    pub(crate) digest: String,
    /// Revision at which the amendment was accepted. Root input uses revision 0.
    pub(crate) revision: u32,
    /// The condition is mandatory for this step (Must in RFC 2119 terms).
    pub(crate) must: bool,
    /// The source has been revoked and must not be used.
    pub(crate) revoked: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RoleContext {
    pub(crate) generation: u32,
    pub(crate) conditions: Vec<ContextCondition>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ContextViolation {
    ScopeWidening,
    RevokedRequiredSource,
    DuplicateAmendment,
    EmptyRequiredProjection,
}

/// Projects the ordered Must conditions for a step. `allowed_scopes` is the host-authorized scope
/// set; `generation` is the root revision the projection was produced for.
pub(crate) fn project_role_context(
    root_input: &ContextCondition,
    amendments: &[ContextCondition],
    allowed_scopes: &[String],
    generation: u32,
    require_non_empty: bool,
) -> Result<RoleContext, ContextViolation> {
    let allowed: HashSet<&str> = allowed_scopes.iter().map(String::as_str).collect();
    let mut seen = HashSet::new();
    let mut conditions = Vec::with_capacity(amendments.len() + 1);
    for condition in std::iter::once(root_input).chain(amendments.iter()) {
        if !condition.must {
            continue;
        }
        if !allowed.contains(condition.scope_key.as_str()) {
            return Err(ContextViolation::ScopeWidening);
        }
        if condition.revoked {
            return Err(ContextViolation::RevokedRequiredSource);
        }
        // A revision/source pair may only appear once. The root input uses revision 0, so a later
        // amendment with the same source id is still distinct by revision.
        if !seen.insert((condition.source_id.as_str(), condition.revision)) {
            return Err(ContextViolation::DuplicateAmendment);
        }
        conditions.push(condition.clone());
    }
    if require_non_empty && conditions.is_empty() {
        return Err(ContextViolation::EmptyRequiredProjection);
    }
    conditions.sort_by_key(|condition| condition.revision);
    Ok(RoleContext {
        generation,
        conditions,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn condition(source_id: &str, revision: u32, scope: &str) -> ContextCondition {
        ContextCondition {
            source_id: source_id.into(),
            scope_key: scope.into(),
            digest: format!("digest-{source_id}-{revision}"),
            revision,
            must: true,
            revoked: false,
        }
    }

    fn scopes(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    #[test]
    fn rr_07_amendment_present_once() {
        let root = condition("input-1", 0, "conversation");
        let amendments = vec![
            condition("input-2", 1, "conversation"),
            condition("input-3", 2, "conversation"),
        ];
        let projected =
            project_role_context(&root, &amendments, &scopes(&["conversation"]), 2, true)
                .expect("projection");
        assert_eq!(projected.conditions.len(), 3);
        // Every accepted amendment is present exactly once and ordered by revision.
        assert_eq!(
            projected
                .conditions
                .iter()
                .map(|condition| condition.source_id.as_str())
                .collect::<Vec<_>>(),
            vec!["input-1", "input-2", "input-3"]
        );
        let duplicate = vec![
            condition("input-2", 1, "conversation"),
            condition("input-2", 1, "conversation"),
        ];
        assert_eq!(
            project_role_context(&root, &duplicate, &scopes(&["conversation"]), 1, true),
            Err(ContextViolation::DuplicateAmendment)
        );
    }

    #[test]
    fn rr_07_scope_no_widening() {
        let root = condition("input-1", 0, "conversation");
        let amendment = condition("memory-1", 1, "memory:personal");
        assert_eq!(
            project_role_context(&root, &[amendment], &scopes(&["conversation"]), 1, true),
            Err(ContextViolation::ScopeWidening)
        );
        // A non-must condition outside the authorized scope is simply filtered out.
        let mut optional = condition("memory-1", 1, "memory:personal");
        optional.must = false;
        let projected =
            project_role_context(&root, &[optional], &scopes(&["conversation"]), 1, true)
                .expect("optional filtered");
        assert_eq!(projected.conditions.len(), 1);
    }

    #[test]
    fn rr_07_revoked_source() {
        let root = condition("input-1", 0, "conversation");
        let mut revoked = condition("memory-1", 1, "conversation");
        revoked.revoked = true;
        assert_eq!(
            project_role_context(&root, &[revoked], &scopes(&["conversation"]), 1, true),
            Err(ContextViolation::RevokedRequiredSource)
        );
        // An empty required projection must not hide a missing condition.
        let optional_root = ContextCondition {
            must: false,
            ..condition("input-1", 0, "conversation")
        };
        assert_eq!(
            project_role_context(&optional_root, &[], &scopes(&["conversation"]), 0, true),
            Err(ContextViolation::EmptyRequiredProjection)
        );
    }
}
