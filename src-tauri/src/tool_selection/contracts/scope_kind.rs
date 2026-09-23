use super::selection_mode::{
    FeedbackKind, InputKind, ObjectType, Operation, Phase, ToolSelectionError,
    ToolSelectionErrorCode, ToolSelectionResult, RULE_CORRECTION_CLAMP, RULE_STRENGTH,
};
use super::*;
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ScopeKind {
    User,
    Project,
    Task,
    Conversation,
}
impl ScopeKind {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "user" => Some(Self::User),
            "project" => Some(Self::Project),
            "task" => Some(Self::Task),
            "conversation" => Some(Self::Conversation),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Project => "project",
            Self::Task => "task",
            Self::Conversation => "conversation",
        }
    }

    /// Narrower scope wins. task > conversation > project > user, per the guide.
    pub fn priority(self) -> u8 {
        match self {
            Self::Task => 4,
            Self::Conversation => 3,
            Self::Project => 2,
            Self::User => 1,
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Duration {
    Once,
    Persistent,
    Unspecified,
}
impl Duration {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "once" => Some(Self::Once),
            "persistent" => Some(Self::Persistent),
            "unspecified" => Some(Self::Unspecified),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Once => "once",
            Self::Persistent => "persistent",
            Self::Unspecified => "unspecified",
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleAction {
    Avoid,
    Prefer,
    Pairwise,
    Forbid,
}
impl RuleAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Avoid => "avoid",
            Self::Prefer => "prefer",
            Self::Pairwise => "pairwise",
            Self::Forbid => "forbid",
        }
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Scenario {
    pub intent: String,
    pub operation: Operation,
    pub object_type: ObjectType,
    pub phase: Phase,
    pub input_kind: InputKind,
}
impl Scenario {
    pub fn degraded(intent: &str) -> Self {
        Self {
            intent: intent.to_string(),
            operation: Operation::Unknown,
            object_type: ObjectType::Unknown,
            phase: Phase::Unknown,
            input_kind: InputKind::Unknown,
        }
    }

    /// Stable semantic signature used in logs and tests. Intent is intentionally excluded from
    /// rule matching but included here because it identifies the decision scenario.
    pub fn canonical(&self) -> String {
        format!(
            "{}|{}|{}|{}|{}",
            self.intent,
            self.operation.as_str(),
            self.object_type.as_str(),
            self.phase.as_str(),
            self.input_kind.as_str()
        )
    }
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FeedbackCondition {
    pub operation: Option<Operation>,
    pub object_type: Option<ObjectType>,
    pub phase: Option<Phase>,
    pub input_kind: Option<InputKind>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Evidence {
    pub start: usize,
    pub end: usize,
    pub text: String,
}
#[derive(Clone, Debug, PartialEq)]
pub struct ExtractedFeedback {
    pub kind: FeedbackKind,
    pub decision_id: Option<String>,
    pub rejected_tool_id: Option<String>,
    pub preferred_tool_id: Option<String>,
    pub scope: ScopeKind,
    pub duration: Duration,
    pub evidence: Evidence,
    pub condition: FeedbackCondition,
}
/// One ranked candidate stored with a decision.
#[derive(Clone, Debug, PartialEq)]
pub struct CandidateRecord {
    pub revision_id: String,
    pub tool_id: String,
    pub lex_rank: Option<i64>,
    pub vec_rank: Option<i64>,
    pub raw_score: Option<f64>,
    pub base_score: f64,
    pub final_score: f64,
    pub rule_ids: Vec<String>,
    pub final_rank: i64,
}
#[derive(Clone, Debug, PartialEq)]
pub struct DecisionRecord {
    pub id: String,
    pub principal_id: String,
    pub conversation_id: String,
    pub run_id: Option<String>,
    pub message_id: Option<String>,
    pub scenario: Scenario,
    pub catalog_epoch: i64,
    pub acl_epoch: i64,
    pub rule_epoch: i64,
    pub model_hash: Option<String>,
    pub status: DecisionStatus,
    pub created_at: i64,
    pub candidates: Vec<CandidateRecord>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DecisionStatus {
    Ok,
    NoMatch,
    Degraded,
}
impl DecisionStatus {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "ok" => Some(Self::Ok),
            "no_match" => Some(Self::NoMatch),
            "degraded" => Some(Self::Degraded),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::NoMatch => "no_match",
            Self::Degraded => "degraded",
        }
    }
}
/// Active rule loaded from the store. `active_rule` owns the matched condition/action.
#[derive(Clone, Debug, PartialEq)]
pub struct StoredRule {
    pub id: String,
    pub principal_id: String,
    pub scope_kind: ScopeKind,
    pub scope_id: String,
    pub operation: String,
    pub object_type: String,
    pub phase: Option<String>,
    pub input_kind: Option<String>,
    pub source_constraint: Option<String>,
    pub target_tool_id: Option<String>,
    pub target_revision_id: Option<String>,
    pub preferred_tool_id: Option<String>,
    pub action: RuleAction,
    pub strength: f64,
    pub state: RuleState,
    pub created_at: i64,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuleState {
    Active,
    Revoked,
    Superseded,
}
impl RuleState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Revoked => "revoked",
            Self::Superseded => "superseded",
        }
    }
}
/// Result of applying active rules to a reranked candidate order.
#[derive(Clone, Debug, PartialEq)]
pub struct CorrectedCandidate {
    pub revision_id: String,
    pub tool_id: String,
    pub base_score: f64,
    pub correction: f64,
    pub final_score: f64,
    pub rule_ids: Vec<String>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct CorrectionOutcome {
    pub ordered: Vec<CorrectedCandidate>,
    pub ambiguous: bool,
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unset_configuration_keeps_direct_mode() {
        let config = ToolSelectionConfig::from_path(None);
        assert_eq!(config.mode, SelectionMode::Direct);
        assert!(!config.discovery_enabled());
    }

    #[test]
    fn invalid_configuration_disables_discovery_instead_of_offering_everything() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("config.json");
        std::fs::write(&path, b"{\"formatVersion\":9,\"mode\":\"discovery\"}").expect("write");
        let config = ToolSelectionConfig::from_path(Some(&path));
        assert_eq!(config.mode, SelectionMode::Disabled);
        assert!(config.diagnostic.is_some());
    }

    #[test]
    fn discovery_requires_both_paths() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("config.json");
        std::fs::write(
            &path,
            br#"{"formatVersion":1,"mode":"discovery","pythonPath":"/venv/bin/python"}"#,
        )
        .expect("write");
        let config = ToolSelectionConfig::from_path(Some(&path));
        assert_eq!(config.mode, SelectionMode::Disabled);
    }

    #[test]
    fn mock_configuration_is_an_explicit_model_free_developer_lane() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("config.json");
        std::fs::write(&path, br#"{"formatVersion":1,"mode":"mock"}"#).expect("write");
        let config = ToolSelectionConfig::from_path(Some(&path));
        assert_eq!(config.mode, SelectionMode::Mock);
        assert!(!config.discovery_enabled());
        assert!(config.python_path.is_none());
        assert!(config.model_manifest_path.is_none());
    }

    #[test]
    fn mock_fixture_runtime_requires_an_explicit_isolated_smoke_environment() {
        use std::ffi::OsStr;

        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("config.json");
        std::fs::write(&path, br#"{"formatVersion":1,"mode":"mock"}"#).expect("write");
        let mock = ToolSelectionConfig::from_path(Some(&path));

        let disabled = restrict_mock_to_fixture_environment(mock.clone(), None, None, None);
        assert_eq!(disabled.mode, SelectionMode::Disabled);
        assert!(disabled.diagnostic.is_some());
        let relative = restrict_mock_to_fixture_environment(
            mock.clone(),
            Some(OsStr::new("1")),
            Some(OsStr::new("fixture")),
            Some(OsStr::new("relative")),
        );
        assert_eq!(relative.mode, SelectionMode::Disabled);
        let isolated = restrict_mock_to_fixture_environment(
            mock,
            Some(OsStr::new("1")),
            Some(OsStr::new("fixture")),
            Some(OsStr::new("/tmp/fixture")),
        );
        assert_eq!(isolated.mode, SelectionMode::Mock);
    }

    #[test]
    fn unknown_semantic_labels_normalize_to_unknown() {
        assert_eq!(Operation::parse("not-a-real-op"), Operation::Unknown);
        assert_eq!(ObjectType::parse("nope"), ObjectType::Unknown);
        assert!(!Operation::parse("nope").is_known());
    }

    #[test]
    fn task_scope_is_narrower_than_user_scope() {
        assert!(ScopeKind::Task.priority() > ScopeKind::Conversation.priority());
        assert!(ScopeKind::Conversation.priority() > ScopeKind::Project.priority());
        assert!(ScopeKind::Project.priority() > ScopeKind::User.priority());
    }
}
