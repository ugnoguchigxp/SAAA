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
const SCENARIO_KEYS: [&str;
5] = ["intent", "operation", "objectType", "phase", "inputKind"];
const FEEDBACK_KEYS: [&str;
8] = [
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
