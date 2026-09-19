//! Scope and condition matching for correction rules, plus the soft-score correction pass.
//!
//! A rule only applies when every condition it names matches the current scenario. Unknown
//! scenario labels never satisfy an explicit condition, so a vague utterance cannot leak into a
//! project or user scope.

use super::contracts::*;
use super::ranking::{apply_pairwise, sort_corrected, weighted_correction, PairwiseOutcome};
use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq)]
pub struct BaseCandidate {
    pub revision_id: String,
    pub tool_id: String,
    pub base_score: f64,
}

/// Scope match against host-confirmed values only.
pub fn scope_matches(rule: &StoredRule, context: &RequestContext) -> bool {
    if rule.principal_id != context.principal_id {
        return false;
    }
    match rule.scope_kind {
        ScopeKind::User => rule.scope_id == context.principal_id,
        ScopeKind::Conversation => rule.scope_id == context.conversation_id,
        ScopeKind::Project => context
            .project_id
            .as_deref()
            .is_some_and(|project| project == rule.scope_id),
        ScopeKind::Task => context
            .task_id
            .as_deref()
            .is_some_and(|task| task == rule.scope_id),
    }
}

/// Exact condition match. `operation`/`object_type` are mandatory; `phase`/`input_kind` apply
/// only when the rule names them. A rule that names a source constraint cannot be verified in
/// D0–D3, so it never matches.
pub fn condition_matches(rule: &StoredRule, scenario: &Scenario) -> bool {
    if rule.source_constraint.is_some() {
        return false;
    }
    if rule.operation != scenario.operation.as_str() || !scenario.operation.is_known() {
        return false;
    }
    if rule.object_type != scenario.object_type.as_str() || !scenario.object_type.is_known() {
        return false;
    }
    if let Some(phase) = rule.phase.as_deref() {
        if phase != scenario.phase.as_str() || !scenario.phase.is_known() {
            return false;
        }
    }
    if let Some(input_kind) = rule.input_kind.as_deref() {
        if input_kind != scenario.input_kind.as_str() || !scenario.input_kind.is_known() {
            return false;
        }
    }
    true
}

fn condition_signature(rule: &StoredRule) -> String {
    format!(
        "{}|{}|{}|{}",
        rule.operation,
        rule.object_type,
        rule.phase.as_deref().unwrap_or("*"),
        rule.input_kind.as_deref().unwrap_or("*"),
    )
}

/// Keeps one rule per target and condition. Soft avoid/prefer share a group so an opposing
/// correction resolves by narrower scope / newer rule instead of cancelling out. Pairwise keeps
/// its preferred partner in the key.
pub fn dedupe_by_condition<'a>(rules: &'a [StoredRule]) -> Vec<&'a StoredRule> {
    let mut best: HashMap<(String, String, String), &StoredRule> = HashMap::new();
    for rule in rules {
        let group = match rule.action {
            RuleAction::Avoid | RuleAction::Prefer => "soft".to_string(),
            RuleAction::Pairwise => format!(
                "pairwise:{}",
                rule.preferred_tool_id.clone().unwrap_or_default()
            ),
            RuleAction::Forbid => "forbid".to_string(),
        };
        let key = (
            rule.target_tool_id.clone().unwrap_or_default(),
            group,
            condition_signature(rule),
        );
        match best.get(&key) {
            Some(existing)
                if (
                    existing.scope_kind.priority(),
                    existing.created_at,
                    existing.id.as_str(),
                ) >= (
                    rule.scope_kind.priority(),
                    rule.created_at,
                    rule.id.as_str(),
                ) => {}
            _ => {
                best.insert(key, rule);
            }
        }
    }
    best.into_values().collect()
}

/// Applies active rules to the reranked candidate order.
pub fn apply_rules(
    candidates: &[BaseCandidate],
    rules: &[StoredRule],
    scenario: &Scenario,
    context: &RequestContext,
) -> CorrectionOutcome {
    let applicable: Vec<&StoredRule> = dedupe_by_condition(rules)
        .into_iter()
        .filter(|rule| scope_matches(rule, context) && condition_matches(rule, scenario))
        .collect();

    let present_tools: std::collections::BTreeSet<&str> = candidates
        .iter()
        .map(|item| item.tool_id.as_str())
        .collect();

    let mut corrections: HashMap<&str, Vec<(RuleAction, String)>> = HashMap::new();
    let mut forbid: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    let mut edges: Vec<(String, String)> = Vec::new();
    for rule in &applicable {
        match rule.action {
            RuleAction::Avoid | RuleAction::Prefer => {
                if let Some(target) = rule.target_tool_id.as_deref() {
                    if present_tools.contains(target) {
                        corrections
                            .entry(target)
                            .or_default()
                            .push((rule.action, rule.id.clone()));
                    }
                }
            }
            RuleAction::Forbid => {
                if let Some(target) = rule.target_tool_id.as_deref() {
                    forbid.insert(target);
                }
            }
            RuleAction::Pairwise => {
                if let (Some(preferred), Some(rejected)) = (
                    rule.preferred_tool_id.as_deref(),
                    rule.target_tool_id.as_deref(),
                ) {
                    if present_tools.contains(preferred) && present_tools.contains(rejected) {
                        edges.push((preferred.to_string(), rejected.to_string()));
                    }
                }
            }
        }
    }

    let mut corrected: Vec<CorrectedCandidate> = candidates
        .iter()
        .filter(|candidate| !forbid.contains(candidate.tool_id.as_str()))
        .map(|candidate| {
            let mut rule_ids = Vec::new();
            let mut actions = Vec::new();
            if let Some(matched) = corrections.get(candidate.tool_id.as_str()) {
                for (action, rule_id) in matched {
                    actions.push(*action);
                    rule_ids.push(rule_id.clone());
                }
            }
            let correction = weighted_correction(&actions);
            rule_ids.sort();
            CorrectedCandidate {
                revision_id: candidate.revision_id.clone(),
                tool_id: candidate.tool_id.clone(),
                base_score: candidate.base_score,
                correction,
                final_score: candidate.base_score + correction,
                rule_ids,
            }
        })
        .collect();
    sort_corrected(&mut corrected);

    if edges.is_empty() {
        return CorrectionOutcome {
            ordered: corrected,
            ambiguous: false,
        };
    }
    let (ordered, outcome) = apply_pairwise(&corrected, &edges);
    CorrectionOutcome {
        ordered,
        ambiguous: outcome == PairwiseOutcome::Cycle,
    }
}

/// Builds the rule row condition from an extraction proposal, normalizing unknown labels.
pub fn condition_from_proposal(condition: &FeedbackCondition) -> FeedbackCondition {
    FeedbackCondition {
        operation: condition.operation.and_then(normalize_operation),
        object_type: condition.object_type.and_then(normalize_object),
        phase: condition.phase.and_then(normalize_phase),
        input_kind: condition.input_kind.and_then(normalize_input),
    }
}

fn normalize_operation(operation: Operation) -> Option<Operation> {
    operation.is_known().then_some(operation)
}

fn normalize_object(object: ObjectType) -> Option<ObjectType> {
    object.is_known().then_some(object)
}

fn normalize_phase(phase: Phase) -> Option<Phase> {
    phase.is_known().then_some(phase)
}

fn normalize_input(input: InputKind) -> Option<InputKind> {
    input.is_known().then_some(input)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base(tool: &str, score: f64) -> BaseCandidate {
        BaseCandidate {
            revision_id: format!("{tool}-rev"),
            tool_id: tool.to_string(),
            base_score: score,
        }
    }

    fn rule(
        id: &str,
        target: &str,
        action: RuleAction,
        scope: ScopeKind,
        scope_id: &str,
    ) -> StoredRule {
        StoredRule {
            id: id.to_string(),
            principal_id: "P1".to_string(),
            scope_kind: scope,
            scope_id: scope_id.to_string(),
            operation: "search".to_string(),
            object_type: "decision_record".to_string(),
            phase: None,
            input_kind: None,
            source_constraint: None,
            target_tool_id: Some(target.to_string()),
            target_revision_id: None,
            preferred_tool_id: None,
            action,
            strength: RULE_STRENGTH,
            state: RuleState::Active,
            created_at: 1,
        }
    }

    fn scenario(operation: Operation, object: ObjectType) -> Scenario {
        Scenario {
            intent: "intent".into(),
            operation,
            object_type: object,
            phase: Phase::Unknown,
            input_kind: InputKind::Unknown,
        }
    }

    #[test]
    fn matching_scope_and_condition_changes_order() {
        let context = RequestContext::new("P1", "C1").with_project(Some("A".into()));
        let candidates = vec![base("web", 1.0), base("minutes", 0.5)];
        let rules = vec![
            rule("r1", "minutes", RuleAction::Prefer, ScopeKind::Project, "A"),
            rule("r2", "web", RuleAction::Avoid, ScopeKind::Project, "A"),
        ];
        let outcome = apply_rules(
            &candidates,
            &rules,
            &scenario(Operation::Search, ObjectType::DecisionRecord),
            &context,
        );
        let order: Vec<&str> = outcome
            .ordered
            .iter()
            .map(|item| item.tool_id.as_str())
            .collect();
        assert_eq!(order, vec!["minutes", "web"]);
    }

    #[test]
    fn different_object_does_not_apply_project_rule() {
        let context = RequestContext::new("P1", "C1").with_project(Some("A".into()));
        let candidates = vec![base("web", 1.0), base("minutes", 0.5)];
        let rules = vec![
            rule("r1", "minutes", RuleAction::Prefer, ScopeKind::Project, "A"),
            rule("r2", "web", RuleAction::Avoid, ScopeKind::Project, "A"),
        ];
        let outcome = apply_rules(
            &candidates,
            &rules,
            &scenario(Operation::Search, ObjectType::CurrentInformation),
            &context,
        );
        let order: Vec<&str> = outcome
            .ordered
            .iter()
            .map(|item| item.tool_id.as_str())
            .collect();
        assert_eq!(order, vec!["web", "minutes"]);
        assert!(outcome.ordered.iter().all(|item| item.correction == 0.0));
    }

    #[test]
    fn unknown_object_cannot_receive_a_rule() {
        let context = RequestContext::new("P1", "C1").with_project(Some("A".into()));
        let candidates = vec![base("web", 1.0), base("minutes", 0.5)];
        let rules = vec![rule(
            "r1",
            "minutes",
            RuleAction::Prefer,
            ScopeKind::Project,
            "A",
        )];
        let outcome = apply_rules(
            &candidates,
            &rules,
            &scenario(Operation::Search, ObjectType::Unknown),
            &context,
        );
        assert!(outcome.ordered.iter().all(|item| item.correction == 0.0));
    }

    #[test]
    fn other_project_and_other_principal_do_not_receive_the_rule() {
        let candidates = vec![base("web", 1.0), base("minutes", 0.5)];
        let rules = vec![rule(
            "r1",
            "minutes",
            RuleAction::Prefer,
            ScopeKind::Project,
            "A",
        )];
        for context in [
            RequestContext::new("P1", "C1").with_project(Some("B".into())),
            RequestContext::new("P2", "C1").with_project(Some("A".into())),
        ] {
            let outcome = apply_rules(
                &candidates,
                &rules,
                &scenario(Operation::Search, ObjectType::DecisionRecord),
                &context,
            );
            assert!(outcome.ordered.iter().all(|item| item.correction == 0.0));
        }
    }

    #[test]
    fn duplicate_rules_do_not_double_count() {
        let context = RequestContext::new("P1", "C1").with_project(Some("A".into()));
        let candidates = vec![base("minutes", 0.5)];
        let rules = vec![
            rule("r1", "minutes", RuleAction::Prefer, ScopeKind::Project, "A"),
            rule("r2", "minutes", RuleAction::Prefer, ScopeKind::Project, "A"),
        ];
        let outcome = apply_rules(
            &candidates,
            &rules,
            &scenario(Operation::Search, ObjectType::DecisionRecord),
            &context,
        );
        assert!((outcome.ordered[0].correction - 0.25).abs() < 1e-12);
    }

    #[test]
    fn narrower_scope_wins_over_broader_scope() {
        let context = RequestContext::new("P1", "C1").with_project(Some("A".into()));
        let candidates = vec![base("minutes", 0.5)];
        let rules = vec![
            rule("user", "minutes", RuleAction::Avoid, ScopeKind::User, "P1"),
            rule(
                "project",
                "minutes",
                RuleAction::Prefer,
                ScopeKind::Project,
                "A",
            ),
        ];
        let outcome = apply_rules(
            &candidates,
            &rules,
            &scenario(Operation::Search, ObjectType::DecisionRecord),
            &context,
        );
        assert!((outcome.ordered[0].correction - 0.25).abs() < 1e-12);
    }
}
