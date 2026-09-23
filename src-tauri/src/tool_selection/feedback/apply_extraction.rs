use super::*;
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
                        super::super::resolve::resolve_tool_id(
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
                    super::super::resolve::resolve_tool_id(
                        connection,
                        value,
                        context,
                        feedback.decision_id.as_deref().or(decision_id),
                    )
                });
                let preferred = feedback.preferred_tool_id.as_deref().and_then(|value| {
                    super::super::resolve::resolve_tool_id(
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
pub(super) fn insert_soft_rule(
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
    // A rule that names a remote MCP tool is bound to the endpoint it was learned on. L-Lang rules
    // carry no binding and are never filtered by endpoint.
    if let Some(endpoint_hash) = repository::source_binding_hash(connection, tool_id)
        .map_err(|_| ToolSelectionError::storage())?
    {
        repository::insert_rule_source_binding(connection, &rule_id, tool_id, &endpoint_hash)
            .map_err(|_| ToolSelectionError::storage())?;
    }
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
