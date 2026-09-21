use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RoleRoutingSettings {
    pub(crate) schema_version: u8,
    pub(crate) enabled: bool,
    #[serde(default)]
    pub(crate) actors: Vec<RoutingActor>,
    #[serde(default)]
    pub(crate) roles: RoutingRoles,
    #[serde(default)]
    pub(crate) recipes: Vec<RoutingRecipe>,
    #[serde(default)]
    pub(crate) limits: RoutingLimits,
    #[serde(default)]
    pub(crate) speech: RoutingSpeech,
    #[serde(default)]
    pub(crate) selection: RoutingSelection,
    #[serde(default = "default_premium_approval")]
    pub(crate) premium_approval: String,
    #[serde(default)]
    pub(crate) learning: RoutingLearning,
    #[serde(default)]
    pub(crate) adaptive_improvement: AdaptiveImprovementSettings,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RoutingActor {
    pub(crate) id: String,
    pub(crate) label: String,
    #[serde(default)]
    pub(crate) aliases: Vec<String>,
    pub(crate) transport: String,
    pub(crate) provider_id: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) location: String,
    pub(crate) resource_group: String,
    pub(crate) max_input_bytes: u32,
    #[serde(default)]
    pub(crate) capabilities: Vec<String>,
}
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RoutingRoles {
    pub(crate) frontend: Option<String>,
    pub(crate) reasoner: Option<String>,
    pub(crate) advanced: Option<String>,
    pub(crate) reviewer: Option<String>,
    pub(crate) premium: Option<String>,
    pub(crate) tool_specialist: Option<String>,
}
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RoutingAction {
    Respond,
    Explain,
    Clarify,
    ReconsiderSame,
    ReconsiderOther,
    ReviewOther,
    Revise,
    ProposeUpgrade,
    Finalize,
    Cancel,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RoutingRecipe {
    pub(crate) id: String,
    pub(crate) action: RoutingAction,
    #[serde(default)]
    pub(crate) roles: Vec<String>,
    pub(crate) enabled: bool,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RoutingLimits {
    pub(crate) max_reasoning_steps: u8,
    pub(crate) max_tool_calls: u8,
    pub(crate) root_timeout_ms: u64,
    pub(crate) step_timeout_ms: u64,
    pub(crate) frontend_timeout_ms: u64,
    pub(crate) classification_timeout_ms: u64,
    pub(crate) max_queued_inputs: u8,
    pub(crate) max_review_rounds: u8,
    pub(crate) max_automatic_switches: u8,
    pub(crate) max_estimated_cost_micros: Option<u64>,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RoutingSpeech {
    pub(crate) mode: String,
    pub(crate) ack_delay_ms: u64,
    pub(crate) max_ack_chars: u16,
    pub(crate) progress_min_interval_ms: u64,
    pub(crate) max_progress_per_root: u8,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RoutingSelection {
    pub(crate) mode: String,
    pub(crate) shadow_artifact_id: Option<String>,
    pub(crate) classification_min_confidence: f64,
    pub(crate) weights: RoutingWeights,
    pub(crate) switch_margin: f64,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RoutingWeights {
    pub(crate) quality: f64,
    pub(crate) latency: f64,
    pub(crate) cost: f64,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RoutingLearning {
    pub(crate) enabled: bool,
    pub(crate) local_start: String,
    pub(crate) local_end: String,
    pub(crate) idle_seconds: u32,
    pub(crate) max_run_seconds: u32,
    pub(crate) batch_size: u16,
    pub(crate) allow_local_labeler: bool,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct AdaptiveImprovementSettings {
    pub(crate) enabled: bool,
    pub(crate) provider_recipe: bool,
    pub(crate) tool: bool,
    pub(crate) plan: bool,
    pub(crate) notification: bool,
}
impl Default for RoleRoutingSettings {
    fn default() -> Self {
        Self {
            schema_version: 1,
            enabled: false,
            actors: vec![],
            roles: RoutingRoles::default(),
            recipes: vec![],
            limits: RoutingLimits::default(),
            speech: RoutingSpeech::default(),
            selection: RoutingSelection::default(),
            premium_approval: default_premium_approval(),
            learning: RoutingLearning::default(),
            adaptive_improvement: AdaptiveImprovementSettings::default(),
        }
    }
}
impl Default for RoutingLimits {
    fn default() -> Self {
        Self {
            max_reasoning_steps: 4,
            max_tool_calls: 32,
            root_timeout_ms: 180_000,
            step_timeout_ms: 60_000,
            frontend_timeout_ms: 1_200,
            classification_timeout_ms: 1_500,
            max_queued_inputs: 4,
            max_review_rounds: 1,
            max_automatic_switches: 2,
            max_estimated_cost_micros: None,
        }
    }
}
impl Default for RoutingSpeech {
    fn default() -> Self {
        Self {
            mode: "author_verbatim".into(),
            ack_delay_ms: 250,
            max_ack_chars: 80,
            progress_min_interval_ms: 15_000,
            max_progress_per_root: 2,
        }
    }
}
impl Default for RoutingSelection {
    fn default() -> Self {
        Self {
            mode: "rules".into(),
            shadow_artifact_id: None,
            classification_min_confidence: 0.85,
            weights: RoutingWeights {
                quality: 0.6,
                latency: 0.25,
                cost: 0.15,
            },
            switch_margin: 0.15,
        }
    }
}
impl Default for RoutingLearning {
    fn default() -> Self {
        Self {
            enabled: false,
            local_start: "02:00".into(),
            local_end: "05:00".into(),
            idle_seconds: 300,
            max_run_seconds: 600,
            batch_size: 100,
            allow_local_labeler: false,
        }
    }
}
impl Default for AdaptiveImprovementSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            provider_recipe: false,
            tool: false,
            plan: false,
            notification: false,
        }
    }
}
fn default_premium_approval() -> String {
    "per_request".into()
}

pub(crate) fn validate_settings(v: &RoleRoutingSettings) -> Result<(), String> {
    if v.schema_version != 1 {
        return Err("Unsupported role routing schema version".into());
    }
    valid_limits(&v.limits)?;
    valid_selection(&v.selection)?;
    valid_learning(&v.learning)?;
    if v.speech.mode != "author_verbatim"
        || v.speech.max_ack_chars > 80
        || v.speech.ack_delay_ms > 10_000
        || v.speech.progress_min_interval_ms < 1_000
        || !matches!(v.premium_approval.as_str(), "per_request" | "never")
    {
        return Err("Invalid role routing speech or approval settings".into());
    }
    if !v.enabled {
        return Ok(());
    }
    if v.actors.is_empty() || v.actors.len() > 16 || v.recipes.is_empty() || v.recipes.len() > 32 {
        return Err("Role routing actors or recipes are outside supported limits".into());
    }
    let mut ids = HashSet::new();
    let mut aliases = HashSet::new();
    for actor in &v.actors {
        if !valid_id(&actor.id)
            || actor.label.is_empty()
            || actor.label.chars().count() > 80
            || actor.aliases.len() > 8
            || !valid_id(&actor.resource_group)
            || !(1..=65_536).contains(&actor.max_input_bytes)
            || !ids.insert(actor.id.as_str())
        {
            return Err("Invalid role routing actor".into());
        }
        if !aliases.insert(actor.id.to_lowercase()) {
            return Err("Duplicate or invalid role routing alias".into());
        }
        if !matches!(actor.location.as_str(), "local" | "cloud")
            || !matches!(actor.transport.as_str(), "provider" | "codex_sdk")
            || (actor.transport == "provider"
                && (!actor.provider_id.as_deref().is_some_and(valid_id) || actor.model.is_some()))
            || (actor.transport == "codex_sdk"
                && (actor.provider_id.is_some()
                    || !actor.model.as_deref().is_some_and(valid_model)))
        {
            return Err("Invalid role routing actor transport".into());
        }
        for alias in &actor.aliases {
            let key = alias.trim().to_lowercase();
            if key.is_empty() || key.chars().count() > 40 || !aliases.insert(key) {
                return Err("Duplicate or invalid role routing alias".into());
            }
        }
    }
    let roles = HashMap::from([
        ("frontend", &v.roles.frontend),
        ("reasoner", &v.roles.reasoner),
        ("advanced", &v.roles.advanced),
        ("reviewer", &v.roles.reviewer),
        ("premium", &v.roles.premium),
        ("tool_specialist", &v.roles.tool_specialist),
    ]);
    if !v
        .roles
        .reasoner
        .as_deref()
        .is_some_and(|id| ids.contains(id))
    {
        return Err("Role routing requires a configured reasoner".into());
    }
    if roles
        .values()
        .copied()
        .flatten()
        .any(|id| !ids.contains(id.as_str()))
    {
        return Err("Role routing role references an unknown actor".into());
    }
    let mut recipe_ids = HashSet::new();
    for recipe in &v.recipes {
        if !valid_id(&recipe.id)
            || !recipe_ids.insert(recipe.id.as_str())
            || recipe.roles.is_empty()
            || recipe.roles.len() > 3
            || recipe
                .roles
                .iter()
                .any(|role| !roles.contains_key(role.as_str()) || roles[role.as_str()].is_none())
            || (recipe.action == RoutingAction::ReviewOther
                && (recipe.roles.len() < 2
                    || recipe
                        .roles
                        .iter()
                        .filter_map(|role| roles[role.as_str()].as_deref())
                        .collect::<HashSet<_>>()
                        .len()
                        != recipe.roles.len()))
        {
            return Err("Invalid role routing recipe".into());
        }
    }
    Ok(())
}
fn valid_limits(v: &RoutingLimits) -> Result<(), String> {
    if !(1..=8).contains(&v.max_reasoning_steps)
        || v.max_tool_calls > 32
        || !(1_000..=600_000).contains(&v.root_timeout_ms)
        || !(1_000..=v.root_timeout_ms).contains(&v.step_timeout_ms)
        || !(100..=3_000).contains(&v.frontend_timeout_ms)
        || !(100..=3_000).contains(&v.classification_timeout_ms)
        || !(1..=8).contains(&v.max_queued_inputs)
        || v.max_review_rounds > 2
        || v.max_automatic_switches > 4
    {
        return Err("Invalid role routing limits".into());
    }
    Ok(())
}
fn valid_selection(v: &RoutingSelection) -> Result<(), String> {
    let sum = v.weights.quality + v.weights.latency + v.weights.cost;
    if !matches!(v.mode.as_str(), "rules" | "shadow")
        || !v.classification_min_confidence.is_finite()
        || !(0.0..=1.0).contains(&v.classification_min_confidence)
        || !v.switch_margin.is_finite()
        || v.switch_margin < 0.0
        || [v.weights.quality, v.weights.latency, v.weights.cost]
            .iter()
            .any(|n| !n.is_finite() || *n < 0.0)
        || (sum - 1.0).abs() > 0.000_001
    {
        return Err("Invalid role routing selection settings".into());
    }
    Ok(())
}
fn valid_learning(v: &RoutingLearning) -> Result<(), String> {
    if !valid_time(&v.local_start)
        || !valid_time(&v.local_end)
        || v.idle_seconds > 86_400
        || !(1..=3_600).contains(&v.max_run_seconds)
        || !(1..=500).contains(&v.batch_size)
    {
        return Err("Invalid role routing learning settings".into());
    }
    Ok(())
}
fn valid_time(v: &str) -> bool {
    v.len() == 5
        && v.as_bytes()[2] == b':'
        && v[0..2].parse::<u8>().is_ok_and(|h| h < 24)
        && v[3..5].parse::<u8>().is_ok_and(|m| m < 60)
}
fn valid_id(v: &str) -> bool {
    !v.is_empty()
        && v.len() <= 80
        && v.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}
fn valid_model(v: &str) -> bool {
    !v.is_empty() && v.len() <= 160 && !v.chars().any(char::is_control)
}

/// A payload limit is always measured in UTF-8 bytes, never characters. Callers use this for
/// user content, tool arguments, and sidecar frames so a multi-byte string cannot slip past a
/// byte-oriented sidecar or SQLite bound.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn validate_payload_bytes(value: &str, limit: usize, label: &str) -> Result<(), String> {
    if value.len() > limit {
        return Err(format!("Role-routing {label} exceeds {limit} UTF-8 bytes"));
    }
    Ok(())
}

/// Role-routing identifiers reuse the application identifier contract (ASCII, 160 bytes max)
/// so ledger IDs cannot smuggle separators or multi-byte content into SQLite keys.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn validate_routing_id(value: &str, label: &str) -> Result<(), String> {
    crate::validate_identifier(value, label)
}
#[cfg(test)]
mod tests {
    use super::*;

    fn provider_actor(id: &str) -> RoutingActor {
        RoutingActor {
            id: id.to_string(),
            label: id.to_string(),
            aliases: Vec::new(),
            transport: "provider".to_string(),
            provider_id: Some("qwen".to_string()),
            model: None,
            location: "local".to_string(),
            resource_group: "gpu".to_string(),
            max_input_bytes: 1_024,
            capabilities: vec!["reason".to_string()],
        }
    }

    #[test]
    fn disabled_default_is_valid() {
        assert!(validate_settings(&RoleRoutingSettings::default()).is_ok());
    }

    #[test]
    fn rr_01_unknown_field() {
        let baseline = serde_json::to_value(RoleRoutingSettings::default()).expect("serialize");
        let mut object = baseline.as_object().cloned().expect("object");
        object.insert("unexpectedField".into(), serde_json::json!(true));
        let encoded = serde_json::Value::Object(object).to_string();
        assert!(serde_json::from_str::<RoleRoutingSettings>(&encoded).is_err());
        // A nested unknown field is rejected the same way.
        let mut nested = serde_json::to_value(RoleRoutingSettings::default()).expect("serialize");
        nested["limits"]["ghost"] = serde_json::json!(1);
        assert!(serde_json::from_value::<RoleRoutingSettings>(nested).is_err());
    }

    #[test]
    fn rr_01_utf8_limit() {
        // "あ" is 3 UTF-8 bytes / 1 char: a char-based check would wrongly accept the second.
        assert!(validate_payload_bytes("あ", 3, "content").is_ok());
        assert!(validate_payload_bytes("ああ", 3, "content").is_err());
        assert!(validate_payload_bytes("abc", 3, "content").is_ok());
    }

    #[test]
    fn rr_01_invalid_id() {
        assert!(validate_routing_id("root-1_ok", "root id").is_ok());
        for invalid in ["", "has space", "slash/sep", "日本語", "dot.dot"] {
            assert!(
                validate_routing_id(invalid, "root id").is_err(),
                "expected {invalid:?} to be rejected"
            );
        }
        let too_long = "a".repeat(161);
        assert!(validate_routing_id(&too_long, "root id").is_err());
    }

    #[test]
    fn cross_review_requires_distinct_actors() {
        let mut settings = RoleRoutingSettings::default();
        settings.enabled = true;
        settings.actors = vec![provider_actor("qwen")];
        settings.roles.reasoner = Some("qwen".to_string());
        settings.roles.reviewer = Some("qwen".to_string());
        settings.recipes = vec![RoutingRecipe {
            id: "review".to_string(),
            action: RoutingAction::ReviewOther,
            roles: vec!["reviewer".to_string(), "reasoner".to_string()],
            enabled: true,
        }];
        assert!(validate_settings(&settings).is_err());
    }
}
