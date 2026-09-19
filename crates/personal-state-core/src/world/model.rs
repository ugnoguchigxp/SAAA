//! World Model value types (WM-01). No IO, clock, or ID generation.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const WORLD_SCHEMA_VERSION: i64 = 1;
pub const WORLD_SEMANTIC_KEY_PREFIX: &str = "wm1:";
pub const MAX_ALIASES: usize = 4;
pub const MAX_CONDITIONS: usize = 4;
pub const MAX_EVIDENCE_STANCES: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityKind {
    Project,
    Concept,
    Metric,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationType {
    RelatedTo,
    PartOf,
    DependsOn,
    ImportantFor,
    Increases,
    Decreases,
}

impl RelationType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::RelatedTo => "related_to",
            Self::PartOf => "part_of",
            Self::DependsOn => "depends_on",
            Self::ImportantFor => "important_for",
            Self::Increases => "increases",
            Self::Decreases => "decreases",
        }
    }

    pub fn is_causal(&self) -> bool {
        matches!(self, Self::Increases | Self::Decreases)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectInput {
    QuantityIncrease,
    Intervention,
}

impl EffectInput {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::QuantityIncrease => "quantity_increase",
            Self::Intervention => "intervention",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Basis {
    UserStatement,
    ModelHypothesis,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stance {
    Supports,
    Challenges,
    Context,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FocusReason {
    CurrentWork,
    ExplicitInterest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Condition {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceStance {
    pub source: crate::SourceKey,
    pub stance: Stance,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EntityPayload {
    #[serde(rename = "type")]
    pub payload_type: EntityTag,
    pub schema_version: i64,
    pub entity_id: String,
    pub entity_kind: EntityKind,
    pub name: String,
    pub aliases: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityTag {
    Entity,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelationPayload {
    #[serde(rename = "type")]
    pub payload_type: RelationTag,
    pub schema_version: i64,
    pub from_entity_id: String,
    pub to_entity_id: String,
    pub relation_type: RelationType,
    pub effect_input: Option<EffectInput>,
    pub conditions: Vec<Condition>,
    pub basis: Basis,
    pub evidence_stances: Vec<EvidenceStance>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationTag {
    Relation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FocusPayload {
    #[serde(rename = "type")]
    pub payload_type: FocusTag,
    pub schema_version: i64,
    pub entity_id: String,
    pub reason: FocusReason,
    pub objective_assertion_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FocusTag {
    Focus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorldPayload {
    Entity(EntityPayload),
    Relation(RelationPayload),
    Focus(FocusPayload),
}

impl WorldPayload {
    /// WM-01 takes the JSON kind name ("world_entity" etc). WM-02 maps
    /// `crate::Kind` variants onto these names without changing behavior.
    pub fn decode(kind_name: &str, value: &serde_json::Value) -> Result<Self, String> {
        match kind_name {
            "world_entity" => {
                let p: EntityPayload =
                    serde_json::from_value(value.clone()).map_err(|_| "world-invalid-payload")?;
                if p.schema_version != WORLD_SCHEMA_VERSION || p.payload_type != EntityTag::Entity {
                    return Err("world-invalid-payload".into());
                }
                Ok(Self::Entity(p))
            }
            "world_relation" => {
                let p: RelationPayload =
                    serde_json::from_value(value.clone()).map_err(|_| "world-invalid-payload")?;
                if p.schema_version != WORLD_SCHEMA_VERSION
                    || p.payload_type != RelationTag::Relation
                {
                    return Err("world-invalid-payload".into());
                }
                Ok(Self::Relation(p))
            }
            "world_focus" => {
                let p: FocusPayload =
                    serde_json::from_value(value.clone()).map_err(|_| "world-invalid-payload")?;
                if p.schema_version != WORLD_SCHEMA_VERSION || p.payload_type != FocusTag::Focus {
                    return Err("world-invalid-payload".into());
                }
                Ok(Self::Focus(p))
            }
            _ => Err("world-invalid-payload".into()),
        }
    }

    pub fn encoded_len(value: &serde_json::Value) -> Result<usize, String> {
        let bytes = serde_json::to_vec(value).map_err(|_| "world-invalid-payload")?;
        Ok(bytes.len())
    }
}

/// Structural checks only (lengths, counts, ordering). Scope/dependency checks
/// live in `validation.rs`.
pub fn check_entity_struct(p: &EntityPayload) -> Result<(), String> {
    if p.entity_id.is_empty() || p.entity_id.len() > 160 {
        return Err("world-invalid-payload".into());
    }
    let trimmed = p.name.trim();
    if trimmed.is_empty() || trimmed.len() > 160 {
        return Err("world-invalid-payload".into());
    }
    if p.aliases.len() > MAX_ALIASES {
        return Err("world-limit".into());
    }
    let mut seen = BTreeSet::new();
    for alias in &p.aliases {
        let t = alias.trim();
        if t.is_empty() || t.len() > 160 {
            return Err("world-invalid-payload".into());
        }
        let canon = super::identity::normalize_name(alias);
        if canon.is_empty() || !seen.insert(canon) {
            return Err("world-invalid-payload".into());
        }
    }
    // Name must not duplicate an alias after normalization.
    let name_canon = super::identity::normalize_name(&p.name);
    if name_canon.is_empty() || seen.contains(&name_canon) {
        return Err("world-invalid-payload".into());
    }
    Ok(())
}

pub fn check_conditions_struct(conditions: &[Condition]) -> Result<Vec<(String, String)>, String> {
    if conditions.len() > MAX_CONDITIONS {
        return Err("world-limit".into());
    }
    let mut pairs: Vec<(String, String)> = Vec::new();
    let mut keys = BTreeSet::new();
    for c in conditions {
        if c.key.is_empty()
            || c.key.len() > 32
            || !c
                .key
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
        {
            return Err("world-invalid-payload".into());
        }
        if c.value.trim().is_empty() || c.value.len() > 96 {
            return Err("world-invalid-payload".into());
        }
        if !keys.insert(c.key.clone()) {
            return Err("world-invalid-payload".into());
        }
        pairs.push((c.key.clone(), c.value.clone()));
    }
    pairs.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(pairs)
}

pub fn check_relation_struct(p: &RelationPayload) -> Result<Vec<(String, String)>, String> {
    if p.from_entity_id.is_empty() || p.to_entity_id.is_empty() {
        return Err("world-invalid-payload".into());
    }
    if p.from_entity_id == p.to_entity_id {
        return Err("world-invalid-payload".into());
    }
    let sorted = check_conditions_struct(&p.conditions)?;
    if p.evidence_stances.is_empty() || p.evidence_stances.len() > MAX_EVIDENCE_STANCES {
        return Err("world-invalid-payload".into());
    }
    // Causal relations require an effect input; others require null.
    let causal = p.relation_type.is_causal();
    if causal && p.effect_input.is_none() {
        return Err("world-invalid-payload".into());
    }
    if !causal && p.effect_input.is_some() {
        return Err("world-invalid-payload".into());
    }
    Ok(sorted)
}

pub fn check_focus_struct(p: &FocusPayload) -> Result<(), String> {
    if p.entity_id.is_empty() {
        return Err("world-invalid-payload".into());
    }
    match p.reason {
        FocusReason::CurrentWork => {
            if p.objective_assertion_id
                .as_deref()
                .is_none_or(|s| s.is_empty())
            {
                return Err("world-invalid-payload".into());
            }
        }
        FocusReason::ExplicitInterest => {
            if p.objective_assertion_id.is_some() {
                return Err("world-invalid-payload".into());
            }
        }
    }
    Ok(())
}

/// WorldSlice and its nested structures (WM-13). Diagnostic data only; the
/// initial release never feeds this to a Provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SliceNode {
    pub entity_id: String,
    pub name: String,
    pub entity_kind: EntityKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SliceRelation {
    pub assertion_id: String,
    pub semantic_key: String,
    pub from_entity_id: String,
    pub to_entity_id: String,
    pub relation_type: String,
    pub status: String,
    pub basis: String,
    pub conditions: Vec<Vec<String>>,
    pub evidence: Vec<SliceEvidence>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SliceEvidence {
    pub source_id: String,
    pub version: u64,
    pub stance: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SliceFocus {
    pub entity_id: String,
    pub reason: String,
    pub objective_assertion_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SliceStep {
    pub assertion_id: String,
    pub relation_type: String,
    pub traversed_reverse: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SlicePath {
    pub nodes: Vec<String>,
    pub steps: Vec<SliceStep>,
    pub conditions_unverified: bool,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResearchGap {
    pub key: String,
    pub reason: String,
    pub relation_assertion_id: String,
    pub focus_entity_id: String,
    pub question: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorldSlice {
    pub revision: u64,
    pub as_of_ms: i64,
    pub nodes: Vec<SliceNode>,
    pub relations: Vec<SliceRelation>,
    pub focus: Vec<SliceFocus>,
    pub relevance_paths: Vec<SlicePath>,
    pub causal_paths: Vec<SlicePath>,
    pub research_gaps: Vec<ResearchGap>,
    pub notices: Vec<String>,
    pub truncated: bool,
}

impl WorldSlice {
    pub fn empty(revision: u64, as_of_ms: i64) -> Self {
        Self {
            revision,
            as_of_ms,
            nodes: Vec::new(),
            relations: Vec::new(),
            focus: Vec::new(),
            relevance_paths: Vec::new(),
            causal_paths: Vec::new(),
            research_gaps: Vec::new(),
            notices: Vec::new(),
            truncated: false,
        }
    }

    pub fn encoded_len(&self) -> Result<usize, String> {
        serde_json::to_vec(self)
            .map(|bytes| bytes.len())
            .map_err(|_| "world-invalid-payload".into())
    }
}
