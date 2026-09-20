//! Extraction-output validation and the correction-application transaction.
//!
//! Parsing is strict (fixed keys, byte-exact evidence spans, only host-provided IDs) but a
//! malformed proposal never blocks the turn: the scenario degrades and the proposal is recorded
//! without creating a rule.

use rusqlite::Connection;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::collections::HashSet;

use super::contracts::*;
use super::repository::{self, NewFeedback, NewRule};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExtractionFailure {
    TooLarge,
    InvalidJson,
    UnexpectedShape,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ParsedExtraction {
    pub scenario: Scenario,
    pub accepted: Vec<ExtractedFeedback>,
    pub rejected: Vec<ExtractedFeedback>,
}

impl ParsedExtraction {
    pub fn degraded(intent: &str) -> Self {
        Self {
            scenario: Scenario::degraded(intent),
            accepted: Vec::new(),
            rejected: Vec::new(),
        }
    }
}

const SCENARIO_KEYS: [&str; 5] = ["intent", "operation", "objectType", "phase", "inputKind"];
const FEEDBACK_KEYS: [&str; 8] = [
    "kind",
    "decisionId",
    "rejectedToolId",
    "preferredToolId",
    "scope",
    "duration",
    "evidence",
    "condition",
];

/// Strict parse. `allowed_decisions` and `allowed_tools` are host-built sets: the model may only
/// reference IDs/names the host already gave it.
pub fn parse_extraction(
    raw: &str,
    user_message: &str,
    allowed_decisions: &HashSet<String>,
    allowed_tools: &HashSet<String>,
) -> Result<ParsedExtraction, ExtractionFailure> {
    if raw.len() > EXTRACT_OUTPUT_MAX_BYTES {
        return Err(ExtractionFailure::TooLarge);
    }
    let value: Value = serde_json::from_str(raw).map_err(|_| ExtractionFailure::InvalidJson)?;
    let object = value
        .as_object()
        .ok_or(ExtractionFailure::UnexpectedShape)?;
    if object
        .keys()
        .any(|key| key != "scenario" && key != "feedback")
    {
        return Err(ExtractionFailure::UnexpectedShape);
    }
    let scenario_value = object
        .get("scenario")
        .and_then(Value::as_object)
        .ok_or(ExtractionFailure::UnexpectedShape)?;
    if scenario_value
        .keys()
        .any(|key| !SCENARIO_KEYS.contains(&key.as_str()))
    {
        return Err(ExtractionFailure::UnexpectedShape);
    }
    let scenario = Scenario {
        intent: scenario_value
            .get("intent")
            .and_then(Value::as_str)
            .unwrap_or(user_message)
            .chars()
            .take(SEARCH_INTENT_MAX_BYTES)
            .collect(),
        operation: Operation::parse(
            scenario_value
                .get("operation")
                .and_then(Value::as_str)
                .unwrap_or("unknown"),
        ),
        object_type: ObjectType::parse(
            scenario_value
                .get("objectType")
                .and_then(Value::as_str)
                .unwrap_or("unknown"),
        ),
        phase: Phase::parse(
            scenario_value
                .get("phase")
                .and_then(Value::as_str)
                .unwrap_or("unknown"),
        ),
        input_kind: InputKind::parse(
            scenario_value
                .get("inputKind")
                .and_then(Value::as_str)
                .unwrap_or("unknown"),
        ),
    };

    let mut accepted = Vec::new();
    let mut rejected = Vec::new();
    if let Some(feedback) = object.get("feedback") {
        let items = feedback
            .as_array()
            .ok_or(ExtractionFailure::UnexpectedShape)?;
        for item in items.iter().take(EXTRACT_MAX_FEEDBACK) {
            match parse_feedback(item, user_message, allowed_decisions, allowed_tools) {
                Some(parsed) => accepted.push(parsed),
                None => {
                    if let Some(best_effort) = salvage_feedback(item) {
                        rejected.push(best_effort);
                    }
                }
            }
        }
    }
    Ok(ParsedExtraction {
        scenario,
        accepted,
        rejected,
    })
}

fn parse_feedback(
    value: &Value,
    user_message: &str,
    allowed_decisions: &HashSet<String>,
    allowed_tools: &HashSet<String>,
) -> Option<ExtractedFeedback> {
    let object = value.as_object()?;
    if object
        .keys()
        .any(|key| !FEEDBACK_KEYS.contains(&key.as_str()))
    {
        return None;
    }
    let kind = FeedbackKind::parse(object.get("kind").and_then(Value::as_str)?);
    if kind == FeedbackKind::Unknown {
        return None;
    }
    let decision_id = optional_string(object.get("decisionId"));
    if let Some(decision) = decision_id.as_deref() {
        if !allowed_decisions.contains(decision) {
            return None;
        }
    }
    let rejected_tool_id = optional_string(object.get("rejectedToolId"));
    let preferred_tool_id = optional_string(object.get("preferredToolId"));
    for tool in [rejected_tool_id.as_deref(), preferred_tool_id.as_deref()]
        .into_iter()
        .flatten()
    {
        if !allowed_tools.contains(tool) {
            return None;
        }
    }
    let scope = ScopeKind::parse(object.get("scope").and_then(Value::as_str)?)?;
    let duration = Duration::parse(object.get("duration").and_then(Value::as_str)?)?;
    let evidence = parse_evidence(object.get("evidence")?, user_message)?;
    let condition = parse_condition(object.get("condition"));
    Some(ExtractedFeedback {
        kind,
        decision_id,
        rejected_tool_id,
        preferred_tool_id,
        scope,
        duration,
        evidence,
        condition,
    })
}

fn optional_string(value: Option<&Value>) -> Option<String> {
    match value {
        Some(Value::String(text)) if !text.is_empty() => Some(text.clone()),
        _ => None,
    }
}

fn parse_evidence(value: &Value, user_message: &str) -> Option<Evidence> {
    let object = value.as_object()?;
    let start = object.get("start")?.as_u64()? as usize;
    let end = object.get("end")?.as_u64()? as usize;
    let text = object.get("text")?.as_str()?.to_string();
    if start > end || end > user_message.len() {
        return None;
    }
    if !user_message.is_char_boundary(start) || !user_message.is_char_boundary(end) {
        return None;
    }
    if user_message[start..end] != text {
        return None;
    }
    Some(Evidence { start, end, text })
}

fn parse_condition(value: Option<&Value>) -> FeedbackCondition {
    let Some(object) = value.and_then(Value::as_object) else {
        return FeedbackCondition::default();
    };
    let field = |name: &str| object.get(name).and_then(Value::as_str);
    FeedbackCondition {
        operation: field("operation").map(Operation::parse),
        object_type: field("objectType").map(ObjectType::parse),
        phase: field("phase").map(Phase::parse),
        input_kind: field("inputKind").map(InputKind::parse),
    }
}

/// A structurally complete item that failed host validation. It is stored as rejected so the
/// audit trail exists, but it produces no rule and no epoch change.
fn salvage_feedback(value: &Value) -> Option<ExtractedFeedback> {
    let object = value.as_object()?;
    let kind = FeedbackKind::parse(
        object
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or("unknown"),
    );
    Some(ExtractedFeedback {
        kind,
        decision_id: optional_string(object.get("decisionId")),
        rejected_tool_id: optional_string(object.get("rejectedToolId")),
        preferred_tool_id: optional_string(object.get("preferredToolId")),
        scope: object
            .get("scope")
            .and_then(Value::as_str)
            .and_then(ScopeKind::parse)
            .unwrap_or(ScopeKind::Conversation),
        duration: object
            .get("duration")
            .and_then(Value::as_str)
            .and_then(Duration::parse)
            .unwrap_or(Duration::Unspecified),
        evidence: Evidence {
            start: 0,
            end: 0,
            text: String::new(),
        },
        condition: parse_condition(object.get("condition")),
    })
}

// ---------------------------------------------------------------------------------------------
// Application
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Debug, Default)]
pub struct ApplyOutcome {
    pub feedback_ids: Vec<String>,
    pub rule_ids: Vec<String>,
    pub applied: bool,
    pub duplicate: bool,
    pub ambiguous: bool,
    pub scope_shrunk: bool,
    pub notes: Vec<&'static str>,
}

#[derive(Clone, Debug)]
struct ResolvedScope {
    kind: ScopeKind,
    id: String,
    expires_at: Option<i64>,
    shrunk: bool,
}

fn resolve_scope(
    feedback: &ExtractedFeedback,
    context: &RequestContext,
    now: i64,
) -> ResolvedScope {
    match feedback.duration {
        Duration::Once => {
            if let Some(task) = context.task_id.as_deref() {
                ResolvedScope {
                    kind: ScopeKind::Task,
                    id: task.to_string(),
                    expires_at: None,
                    shrunk: false,
                }
            } else {
                ResolvedScope {
                    kind: ScopeKind::Conversation,
                    id: context.conversation_id.clone(),
                    expires_at: Some(now + 24 * 60 * 60 * 1000),
                    shrunk: false,
                }
            }
        }
        Duration::Unspecified => ResolvedScope {
            kind: ScopeKind::Conversation,
            id: context.conversation_id.clone(),
            expires_at: Some(now + 24 * 60 * 60 * 1000),
            shrunk: false,
        },
        Duration::Persistent => match feedback.scope {
            ScopeKind::Project => match context.project_id.as_deref() {
                Some(project) => ResolvedScope {
                    kind: ScopeKind::Project,
                    id: project.to_string(),
                    expires_at: None,
                    shrunk: false,
                },
                None => ResolvedScope {
                    kind: ScopeKind::Conversation,
                    id: context.conversation_id.clone(),
                    expires_at: None,
                    shrunk: true,
                },
            },
            ScopeKind::User => ResolvedScope {
                kind: ScopeKind::User,
                id: context.principal_id.clone(),
                expires_at: None,
                shrunk: false,
            },
            ScopeKind::Task => match context.task_id.as_deref() {
                Some(task) => ResolvedScope {
                    kind: ScopeKind::Task,
                    id: task.to_string(),
                    expires_at: None,
                    shrunk: false,
                },
                None => ResolvedScope {
                    kind: ScopeKind::Conversation,
                    id: context.conversation_id.clone(),
                    expires_at: None,
                    shrunk: true,
                },
            },
            ScopeKind::Conversation => ResolvedScope {
                kind: ScopeKind::Conversation,
                id: context.conversation_id.clone(),
                expires_at: None,
                shrunk: false,
            },
        },
    }
}

/// Normalizes the condition, borrowing the scenario's operation/object when the proposal left
/// them out. Returns `None` when operation or object is still unknown: a permanent rule must
/// never be created from an unbound condition.
fn resolved_condition(
    feedback: &ExtractedFeedback,
    scenario: &Scenario,
) -> Option<FeedbackCondition> {
    let operation = feedback
        .condition
        .operation
        .filter(|value| value.is_known())
        .or_else(|| scenario.operation.is_known().then_some(scenario.operation))?;
    let object_type = feedback
        .condition
        .object_type
        .filter(|value| value.is_known())
        .or_else(|| {
            scenario
                .object_type
                .is_known()
                .then_some(scenario.object_type)
        })?;
    Some(FeedbackCondition {
        operation: Some(operation),
        object_type: Some(object_type),
        phase: feedback.condition.phase.filter(|value| value.is_known()),
        input_kind: feedback
            .condition
            .input_kind
            .filter(|value| value.is_known()),
    })
}

fn signature(
    message_id: &str,
    decision_id: Option<&str>,
    kind: FeedbackKind,
    rejected: Option<&str>,
    preferred: Option<&str>,
    condition: &FeedbackCondition,
) -> String {
    let mut object = Map::new();
    object.insert("message".into(), Value::String(message_id.to_string()));
    object.insert(
        "decision".into(),
        Value::String(decision_id.unwrap_or_default().to_string()),
    );
    object.insert("kind".into(), Value::String(kind.as_str().to_string()));
    object.insert(
        "target".into(),
        Value::String(rejected.unwrap_or_default().to_string()),
    );
    object.insert(
        "preferred".into(),
        Value::String(preferred.unwrap_or_default().to_string()),
    );
    object.insert(
        "operation".into(),
        Value::String(
            condition
                .operation
                .map(Operation::as_str)
                .unwrap_or("unknown")
                .to_string(),
        ),
    );
    object.insert(
        "object".into(),
        Value::String(
            condition
                .object_type
                .map(ObjectType::as_str)
                .unwrap_or("unknown")
                .to_string(),
        ),
    );
    object.insert(
        "phase".into(),
        Value::String(
            condition
                .phase
                .map(Phase::as_str)
                .unwrap_or("*")
                .to_string(),
        ),
    );
    object.insert(
        "input".into(),
        Value::String(
            condition
                .input_kind
                .map(InputKind::as_str)
                .unwrap_or("*")
                .to_string(),
        ),
    );
    let canonical = Value::Object(object).to_string();
    let digest = Sha256::digest(canonical.as_bytes());
    let mut output = String::with_capacity(64);
    for byte in digest {
        output.push_str(&format!("{byte:02x}"));
    }
    output
}

/// Applies one extraction inside the caller's writer transaction. Any repository error aborts the
/// transaction so a partial correction is never committed.
pub fn apply_extraction(
    connection: &Connection,
    context: &RequestContext,
    decision_id: Option<&str>,
    parsed: &ParsedExtraction,
    now: i64,
) -> ToolSelectionResult<ApplyOutcome> {
    let message_id = context
        .input_message_id
        .as_deref()
        .ok_or_else(ToolSelectionError::invalid)?;
    let mut outcome = ApplyOutcome::default();

    for (feedback, is_rejected) in parsed
        .accepted
        .iter()
        .map(|item| (item, false))
        .chain(parsed.rejected.iter().map(|item| (item, true)))
    {
        // Items that failed host validation are stored as rejected. Their referenced decision ID
        // may be unknown, so it is never used as a foreign key.
        let row_decision_id = if is_rejected {
            None
        } else {
            feedback.decision_id.as_deref().or(decision_id)
        };
        let condition = resolved_condition(feedback, &parsed.scenario);
        let key = signature(
            message_id,
            feedback.decision_id.as_deref().or(decision_id),
            feedback.kind,
            feedback.rejected_tool_id.as_deref(),
            feedback.preferred_tool_id.as_deref(),
            &condition.clone().unwrap_or_default(),
        );
        let feedback_id = crate::new_id("tsfb");
        let evidence_json = serde_json::json!({
            "start": feedback.evidence.start,
            "end": feedback.evidence.end,
            "text": feedback.evidence.text,
        });
        let proposal_json = serde_json::json!({
            "kind": feedback.kind.as_str(),
            "rejectedToolId": feedback.rejected_tool_id,
            "preferredToolId": feedback.preferred_tool_id,
            "scope": feedback.scope.as_str(),
            "duration": feedback.duration.as_str(),
        });
        let stored = repository::insert_feedback_if_absent(
            connection,
            &NewFeedback {
                id: &feedback_id,
                principal_id: &context.principal_id,
                message_id,
                decision_id: row_decision_id,
                kind: feedback.kind.as_str(),
                evidence_json: &evidence_json,
                proposal_json: &proposal_json,
                status: "pending",
                idempotency_key: &key,
                created_at: now,
            },
        )
        .map_err(|_| ToolSelectionError::storage())?;
        if !stored {
            outcome.duplicate = true;
            continue;
        }
        outcome.feedback_ids.push(feedback_id.clone());

        // Rejected proposals never become rules.
        if is_rejected {
            repository::update_feedback_status(connection, &feedback_id, "rejected")
                .map_err(|_| ToolSelectionError::storage())?;
            continue;
        }

        match feedback.kind {
            FeedbackKind::Ambiguous => {
                repository::update_feedback_status(connection, &feedback_id, "ambiguous")
                    .map_err(|_| ToolSelectionError::storage())?;
                outcome.ambiguous = true;
                outcome
                    .notes
                    .push("The correction is ambiguous; the previous instruction was kept.");
            }
            FeedbackKind::Revoke => {
                if feedback.rejected_tool_id.is_none() && feedback.preferred_tool_id.is_none() {
                    repository::update_feedback_status(connection, &feedback_id, "ambiguous")
                        .map_err(|_| ToolSelectionError::storage())?;
                    outcome.ambiguous = true;
                    outcome.notes.push("Which instruction should be withdrawn?");
                    continue;
                }
                // A named but unresolvable tool must not revoke every correction in scope.
                let target = feedback
                    .rejected_tool_id
                    .as_deref()
                    .or(feedback.preferred_tool_id.as_deref())
                    .and_then(|value| {
                        super::resolve::resolve_tool_id(
                            connection,
                            value,
                            context,
                            feedback.decision_id.as_deref().or(decision_id),
                        )
                    });
                let Some(target) = target else {
                    repository::update_feedback_status(connection, &feedback_id, "ambiguous")
                        .map_err(|_| ToolSelectionError::storage())?;
                    outcome.ambiguous = true;
                    outcome
                        .notes
                        .push("The instruction to withdraw could not be resolved.");
                    continue;
                };
                let scope = resolve_scope(feedback, context, now);
                repository::revoke_matching_soft_rules(
                    connection,
                    &context.principal_id,
                    scope.kind.as_str(),
                    &scope.id,
                    Some(target.as_str()),
                )
                .map_err(|_| ToolSelectionError::storage())?;
                repository::update_feedback_status(connection, &feedback_id, "applied")
                    .map_err(|_| ToolSelectionError::storage())?;
                outcome.applied = true;
            }
            FeedbackKind::ExplicitPositive => {
                if let Some(decision) = feedback.decision_id.as_deref().or(decision_id) {
                    repository::set_satisfaction_for_decision(
                        connection,
                        decision,
                        "explicit_positive",
                    )
                    .map_err(|_| ToolSelectionError::storage())?;
                }
                repository::update_feedback_status(connection, &feedback_id, "applied")
                    .map_err(|_| ToolSelectionError::storage())?;
            }
            FeedbackKind::Arguments
            | FeedbackKind::OutputQuality
            | FeedbackKind::SourceScope
            | FeedbackKind::TemporaryConstraint => {
                // These never lower the whole-tool ranking.
                repository::update_feedback_status(connection, &feedback_id, "applied")
                    .map_err(|_| ToolSelectionError::storage())?;
            }
            FeedbackKind::ToolChoice => {
                let Some(condition) = condition else {
                    repository::update_feedback_status(connection, &feedback_id, "ambiguous")
                        .map_err(|_| ToolSelectionError::storage())?;
                    outcome.ambiguous = true;
                    outcome
                        .notes
                        .push("No persistent rule was saved for an unbound condition.");
                    continue;
                };
                let scope = resolve_scope(feedback, context, now);
                if scope.shrunk {
                    outcome.scope_shrunk = true;
                    outcome.notes.push(
                        "Saved for this conversation because no project/task ID was confirmed.",
                    );
                }
                let rejected = feedback.rejected_tool_id.as_deref().and_then(|value| {
                    super::resolve::resolve_tool_id(
                        connection,
                        value,
                        context,
                        feedback.decision_id.as_deref().or(decision_id),
                    )
                });
                let preferred = feedback.preferred_tool_id.as_deref().and_then(|value| {
                    super::resolve::resolve_tool_id(
                        connection,
                        value,
                        context,
                        feedback.decision_id.as_deref().or(decision_id),
                    )
                });
                let mut created = false;
                if let Some(tool_id) = rejected.as_deref() {
                    created |= insert_soft_rule(
                        connection,
                        context,
                        &feedback_id,
                        &scope,
                        &condition,
                        tool_id,
                        RuleAction::Avoid,
                        now,
                        &mut outcome,
                    )?;
                }
                if let Some(tool_id) = preferred.as_deref() {
                    created |= insert_soft_rule(
                        connection,
                        context,
                        &feedback_id,
                        &scope,
                        &condition,
                        tool_id,
                        RuleAction::Prefer,
                        now,
                        &mut outcome,
                    )?;
                }
                if !created {
                    repository::update_feedback_status(connection, &feedback_id, "ambiguous")
                        .map_err(|_| ToolSelectionError::storage())?;
                    outcome.ambiguous = true;
                    outcome
                        .notes
                        .push("The correction did not name a usable tool.");
                } else {
                    repository::update_feedback_status(connection, &feedback_id, "applied")
                        .map_err(|_| ToolSelectionError::storage())?;
                }
            }
            FeedbackKind::Unknown => {
                repository::update_feedback_status(connection, &feedback_id, "ambiguous")
                    .map_err(|_| ToolSelectionError::storage())?;
                outcome.ambiguous = true;
            }
        }
    }

    if outcome.applied {
        repository::bump_epochs(connection, false, false, true)
            .map_err(|_| ToolSelectionError::storage())?;
    }
    Ok(outcome)
}

#[allow(clippy::too_many_arguments)]
fn insert_soft_rule(
    connection: &Connection,
    context: &RequestContext,
    feedback_id: &str,
    scope: &ResolvedScope,
    condition: &FeedbackCondition,
    tool_id: &str,
    action: RuleAction,
    now: i64,
    outcome: &mut ApplyOutcome,
) -> ToolSelectionResult<bool> {
    let rule_id = crate::new_id("tsrule");
    repository::insert_rule(
        connection,
        &NewRule {
            id: &rule_id,
            feedback_id,
            principal_id: &context.principal_id,
            scope_kind: scope.kind.as_str(),
            scope_id: &scope.id,
            operation: condition
                .operation
                .map(Operation::as_str)
                .unwrap_or("unknown"),
            object_type: condition
                .object_type
                .map(ObjectType::as_str)
                .unwrap_or("unknown"),
            phase: condition.phase.map(Phase::as_str),
            input_kind: condition.input_kind.map(InputKind::as_str),
            source_constraint: None,
            target_tool_id: Some(tool_id),
            target_revision_id: None,
            preferred_tool_id: None,
            action: action.as_str(),
            strength: RULE_STRENGTH,
            expires_at: scope.expires_at,
            state: "active",
            created_at: now,
        },
    )
    .map_err(|_| ToolSelectionError::storage())?;
    repository::supersede_soft_rules(
        connection,
        tool_id,
        action.as_str(),
        condition,
        scope.kind.as_str(),
        &scope.id,
        &rule_id,
    )
    .map_err(|_| ToolSelectionError::storage())?;
    outcome.rule_ids.push(rule_id);
    outcome.applied = true;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allowed() -> (HashSet<String>, HashSet<String>) {
        (
            ["d1".to_string()].into_iter().collect(),
            ["web".to_string(), "minutes".to_string()]
                .into_iter()
                .collect(),
        )
    }

    #[test]
    fn valid_output_parses() {
        let (decisions, tools) = allowed();
        let raw = r#"{"scenario":{"intent":"案件の過去の判断を確認","operation":"search","objectType":"decision_record","phase":"discover","inputKind":"text"},"feedback":[{"kind":"tool_choice","decisionId":"d1","rejectedToolId":"web","preferredToolId":"minutes","scope":"project","duration":"persistent","evidence":{"start":0,"end":6,"text":"この"},"condition":{"operation":"search","objectType":"decision_record","phase":null,"inputKind":null}}]}"#;
        let parsed = parse_extraction(raw, "この案件", &decisions, &tools).expect("parse");
        assert_eq!(parsed.accepted.len(), 1);
        assert_eq!(parsed.scenario.object_type, ObjectType::DecisionRecord);
    }

    #[test]
    fn unknown_decision_id_is_rejected() {
        let (decisions, tools) = allowed();
        let raw = r#"{"scenario":{"intent":"x","operation":"search","objectType":"decision_record","phase":"discover","inputKind":"text"},"feedback":[{"kind":"tool_choice","decisionId":"other","rejectedToolId":"web","preferredToolId":"minutes","scope":"project","duration":"persistent","evidence":{"start":0,"end":1,"text":"あ"},"condition":{"operation":"search","objectType":"decision_record","phase":null,"inputKind":null}}]}"#;
        let parsed = parse_extraction(raw, "あ", &decisions, &tools).expect("parse");
        assert!(parsed.accepted.is_empty());
    }

    #[test]
    fn byte_boundary_error_is_rejected() {
        let (decisions, tools) = allowed();
        let message = "日本語";
        let raw = r#"{"scenario":{"intent":"x","operation":"search","objectType":"decision_record","phase":"discover","inputKind":"text"},"feedback":[{"kind":"tool_choice","decisionId":"d1","rejectedToolId":"web","preferredToolId":"minutes","scope":"project","duration":"persistent","evidence":{"start":1,"end":2,"text":"本"},"condition":{"operation":"search","objectType":"decision_record","phase":null,"inputKind":null}}]}"#;
        let parsed = parse_extraction(raw, message, &decisions, &tools).expect("parse");
        assert!(parsed.accepted.is_empty());
    }

    #[test]
    fn extra_keys_are_rejected() {
        let (decisions, tools) = allowed();
        let raw = r#"{"scenario":{"intent":"x","operation":"search","objectType":"decision_record","phase":"discover","inputKind":"text","extra":1}}"#;
        assert_eq!(
            parse_extraction(raw, "x", &decisions, &tools),
            Err(ExtractionFailure::UnexpectedShape)
        );
    }

    #[test]
    fn unknown_enum_normalizes_to_unknown() {
        let (decisions, tools) = allowed();
        let raw = r#"{"scenario":{"intent":"x","operation":"teleport","objectType":"alien","phase":"???","inputKind":"???"}}"#;
        let parsed = parse_extraction(raw, "x", &decisions, &tools).expect("parse");
        assert_eq!(parsed.scenario.operation, Operation::Unknown);
        assert_eq!(parsed.scenario.object_type, ObjectType::Unknown);
    }

    #[test]
    fn signature_is_stable_for_the_same_event() {
        let condition = FeedbackCondition {
            operation: Some(Operation::Search),
            object_type: Some(ObjectType::DecisionRecord),
            phase: None,
            input_kind: None,
        };
        let first = signature(
            "m",
            Some("d"),
            FeedbackKind::ToolChoice,
            Some("a"),
            Some("b"),
            &condition,
        );
        let second = signature(
            "m",
            Some("d"),
            FeedbackKind::ToolChoice,
            Some("a"),
            Some("b"),
            &condition,
        );
        assert_eq!(first, second);
    }
}
