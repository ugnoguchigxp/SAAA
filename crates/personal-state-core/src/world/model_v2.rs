//! World Model v2 wire types (D06). No IO, clock or ID generation.
//!
//! v1 types in `model.rs` are frozen: this module only adds the version-2
//! payloads, enums and structural checks. Scope/dependency semantics live in
//! `validation_v2.rs`.

use super::model::{
    Basis, Condition, EffectInput, EntityTag, EvidenceStance, FocusReason, FocusTag, RelationTag,
};
use crate::SourceKey;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const WORLD_V2_SCHEMA_VERSION: i64 = 2;
pub const WORLD_V2_SEMANTIC_KEY_PREFIX: &str = "wm2:";
pub const MAX_STATE_BYTES: usize = 2_000;
pub const MAX_ASSESSMENT_REFS: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityKindV2 {
    Project,
    Concept,
    Metric,
    Goal,
    Actor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationTypeV2 {
    RelatedTo,
    PartOf,
    DependsOn,
    ImportantFor,
    Increases,
    Decreases,
    Causes,
    Enables,
    Inhibits,
    HasGoal,
    ServesGoal,
    CorrelatesWith,
}

impl RelationTypeV2 {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::RelatedTo => "related_to",
            Self::PartOf => "part_of",
            Self::DependsOn => "depends_on",
            Self::ImportantFor => "important_for",
            Self::Increases => "increases",
            Self::Decreases => "decreases",
            Self::Causes => "causes",
            Self::Enables => "enables",
            Self::Inhibits => "inhibits",
            Self::HasGoal => "has_goal",
            Self::ServesGoal => "serves_goal",
            Self::CorrelatesWith => "correlates_with",
        }
    }

    /// Relations whose sign can participate in causal composition.
    pub fn is_causal(&self) -> bool {
        matches!(self, Self::Increases | Self::Decreases)
    }

    /// Relations with a signed effect direction (increase/decrease).
    pub fn is_signed_effect(&self) -> bool {
        matches!(
            self,
            Self::Increases | Self::Decreases | Self::Causes | Self::Enables | Self::Inhibits
        )
    }

    pub fn is_auxiliary(&self) -> bool {
        matches!(self, Self::RelatedTo | Self::PartOf | Self::ImportantFor)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetDirection {
    LowerIsBetter,
    HigherIsBetter,
}

impl TargetDirection {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::LowerIsBetter => "lower_is_better",
            Self::HigherIsBetter => "higher_is_better",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CorrelationSign {
    Positive,
    Negative,
}

impl CorrelationSign {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Positive => "positive",
            Self::Negative => "negative",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Epistemic {
    Observation,
    Hypothesis,
    Supported,
    Disputed,
}

impl Epistemic {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Observation => "observation",
            Self::Hypothesis => "hypothesis",
            Self::Supported => "supported",
            Self::Disputed => "disputed",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfidenceMethod {
    ManualV1,
    CounterevidenceV1,
    /// Return-only: path ranking uses this but it is never storable.
    PathRankV1,
}

impl ConfidenceMethod {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ManualV1 => "manual_v1",
            Self::CounterevidenceV1 => "counterevidence_v1",
            Self::PathRankV1 => "path_rank_v1",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MechanismState {
    Unassessed,
    Missing,
    Described,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectDirection {
    Increase,
    Decrease,
    Unchanged,
    Unknown,
    /// Path return only. Never a stored OutcomeUpdate direction.
    Mixed,
}

impl EffectDirection {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Increase => "increase",
            Self::Decrease => "decrease",
            Self::Unchanged => "unchanged",
            Self::Unknown => "unknown",
            Self::Mixed => "mixed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Confidence {
    pub value: u16,
    pub method: ConfidenceMethod,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CorrelationStrength {
    pub magnitude: u16,
    pub method: String,
    pub population: String,
    pub period_start_ms: i64,
    pub period_end_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutcomeUpdate {
    pub prior_assertion_id: String,
    pub outcome_source: SourceKey,
    pub prediction_source: SourceKey,
    pub comparison_id: String,
    pub expected: EffectDirection,
    pub actual: EffectDirection,
    pub predicted_at_ms: i64,
    pub observed_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EntityPayloadV2 {
    #[serde(rename = "type")]
    pub payload_type: EntityTag,
    pub schema_version: i64,
    pub entity_id: String,
    pub entity_kind: EntityKindV2,
    pub name: String,
    pub aliases: Vec<String>,
    pub objective_assertion_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelationPayloadV2 {
    #[serde(rename = "type")]
    pub payload_type: RelationTag,
    pub schema_version: i64,
    pub from_entity_id: String,
    pub to_entity_id: String,
    pub relation_type: RelationTypeV2,
    pub effect_input: Option<EffectInput>,
    pub conditions: Vec<Condition>,
    pub comparison_id: Option<String>,
    pub basis: Basis,
    pub evidence_stances: Vec<EvidenceStance>,
    pub target_direction: Option<TargetDirection>,
    pub correlation_sign: Option<CorrelationSign>,
    pub epistemic: Epistemic,
    pub confidence: Option<Confidence>,
    pub strength: Option<CorrelationStrength>,
    pub assessment_refs: Vec<SourceKey>,
    pub mechanism: MechanismState,
    pub outcome_update: Option<OutcomeUpdate>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FocusPayloadV2 {
    #[serde(rename = "type")]
    pub payload_type: FocusTag,
    pub schema_version: i64,
    pub entity_id: String,
    pub reason: FocusReason,
    pub objective_assertion_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(clippy::large_enum_variant)]
pub enum WorldPayloadV2 {
    Entity(EntityPayloadV2),
    Relation(RelationPayloadV2),
    Focus(FocusPayloadV2),
}

impl WorldPayloadV2 {
    pub fn decode(kind_name: &str, value: &serde_json::Value) -> Result<Self, String> {
        match kind_name {
            "world_entity" => {
                let p: EntityPayloadV2 =
                    serde_json::from_value(value.clone()).map_err(|_| "world-invalid-payload")?;
                if p.schema_version != WORLD_V2_SCHEMA_VERSION
                    || p.payload_type != EntityTag::Entity
                {
                    return Err("world-invalid-payload".into());
                }
                Ok(Self::Entity(p))
            }
            "world_relation" => {
                let p: RelationPayloadV2 =
                    serde_json::from_value(value.clone()).map_err(|_| "world-invalid-payload")?;
                if p.schema_version != WORLD_V2_SCHEMA_VERSION
                    || p.payload_type != RelationTag::Relation
                {
                    return Err("world-invalid-payload".into());
                }
                Ok(Self::Relation(p))
            }
            "world_focus" => {
                let p: FocusPayloadV2 =
                    serde_json::from_value(value.clone()).map_err(|_| "world-invalid-payload")?;
                if p.schema_version != WORLD_V2_SCHEMA_VERSION || p.payload_type != FocusTag::Focus
                {
                    return Err("world-invalid-payload".into());
                }
                Ok(Self::Focus(p))
            }
            _ => Err("world-invalid-payload".into()),
        }
    }

    /// Canonical JSON bytes used for the `wm2-payload-` reference and the
    /// state-byte limit. Field order follows the struct definition; optional
    /// fields serialize as `null`.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, String> {
        match self {
            Self::Entity(p) => serde_json::to_vec(p),
            Self::Relation(p) => serde_json::to_vec(p),
            Self::Focus(p) => serde_json::to_vec(p),
        }
        .map_err(|_| "world-invalid-payload".into())
    }

    pub fn byte_len(&self) -> Result<usize, String> {
        self.canonical_bytes().map(|bytes| bytes.len())
    }
}

fn check_id(value: &str) -> Result<(), String> {
    if value.is_empty() || value.len() > 160 {
        return Err("world-invalid-payload".into());
    }
    Ok(())
}

fn check_medium_text(value: &str) -> Result<(), String> {
    if value.is_empty() || value.len() > 96 {
        return Err("world-invalid-payload".into());
    }
    Ok(())
}

pub fn check_entity_v2_struct(p: &EntityPayloadV2) -> Result<(), String> {
    if p.schema_version != WORLD_V2_SCHEMA_VERSION {
        return Err("world-invalid-payload".into());
    }
    check_id(&p.entity_id)?;
    let trimmed = p.name.trim();
    if trimmed.is_empty() || trimmed.len() > 160 {
        return Err("world-invalid-payload".into());
    }
    if p.aliases.len() > super::model::MAX_ALIASES {
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
    let name_canon = super::identity::normalize_name(&p.name);
    if name_canon.is_empty() || seen.contains(&name_canon) {
        return Err("world-invalid-payload".into());
    }
    match p.entity_kind {
        EntityKindV2::Goal => {
            let objective = p
                .objective_assertion_id
                .as_deref()
                .ok_or("world-invalid-payload")?;
            check_id(objective)?;
        }
        _ => {
            if p.objective_assertion_id.is_some() {
                return Err("world-invalid-payload".into());
            }
        }
    }
    Ok(())
}

pub fn check_relation_v2_struct(p: &RelationPayloadV2) -> Result<Vec<(String, String)>, String> {
    if p.schema_version != WORLD_V2_SCHEMA_VERSION {
        return Err("world-invalid-payload".into());
    }
    check_id(&p.from_entity_id)?;
    check_id(&p.to_entity_id)?;
    if p.from_entity_id == p.to_entity_id {
        return Err("world-invalid-payload".into());
    }
    let sorted = super::model::check_conditions_struct(&p.conditions)?;
    if p.evidence_stances.is_empty()
        || p.evidence_stances.len() > super::model::MAX_EVIDENCE_STANCES
    {
        return Err("world-invalid-payload".into());
    }
    if let Some(comparison) = &p.comparison_id {
        check_medium_text(comparison)?;
    }
    if p.assessment_refs.len() > MAX_ASSESSMENT_REFS {
        return Err("world-limit".into());
    }
    let mut refs = BTreeSet::new();
    for source in &p.assessment_refs {
        if !refs.insert(source) {
            return Err("world-invalid-payload".into());
        }
    }
    if let Some(confidence) = &p.confidence {
        if confidence.value > 1000 {
            return Err("world-invalid-payload".into());
        }
        if confidence.method == ConfidenceMethod::PathRankV1 {
            return Err("world-invalid-payload".into());
        }
        if p.assessment_refs.is_empty() {
            return Err("world-invalid-payload".into());
        }
    }
    if let Some(strength) = &p.strength {
        if strength.magnitude > 1000 {
            return Err("world-invalid-payload".into());
        }
        check_medium_text(&strength.method)?;
        check_medium_text(&strength.population)?;
        if strength.period_start_ms >= strength.period_end_ms {
            return Err("world-invalid-payload".into());
        }
        if p.assessment_refs.is_empty() {
            return Err("world-invalid-payload".into());
        }
    }
    if matches!(
        p.mechanism,
        MechanismState::Missing | MechanismState::Described
    ) && p.assessment_refs.is_empty()
    {
        return Err("world-invalid-payload".into());
    }
    if p.epistemic == Epistemic::Supported {
        // Reserved value: decodable but never newly storable (C3).
        return Err("world-invalid-payload".into());
    }
    if let Some(update) = &p.outcome_update {
        check_medium_text(&update.comparison_id)?;
        if matches!(
            update.expected,
            EffectDirection::Mixed | EffectDirection::Unknown
        ) || matches!(update.actual, EffectDirection::Mixed)
        {
            return Err("world-invalid-payload".into());
        }
        if update.predicted_at_ms > update.observed_at_ms {
            return Err("world-invalid-payload".into());
        }
    }
    // Fixed per-type optional-attribute contract (C3 / plan section 5.3 for
    // relation_type itself is checked in validation_v2 where endpoints are known).
    if p.target_direction.is_some() && !matches!(p.relation_type, RelationTypeV2::ServesGoal) {
        return Err("world-invalid-payload".into());
    }
    let needs_effect = matches!(
        p.relation_type,
        RelationTypeV2::Increases
            | RelationTypeV2::Decreases
            | RelationTypeV2::Causes
            | RelationTypeV2::Enables
            | RelationTypeV2::Inhibits
    );
    if needs_effect != p.effect_input.is_some() {
        return Err("world-invalid-payload".into());
    }
    if p.strength.is_some() && !matches!(p.relation_type, RelationTypeV2::CorrelatesWith) {
        return Err("world-invalid-payload".into());
    }
    if p.correlation_sign.is_some() && !matches!(p.relation_type, RelationTypeV2::CorrelatesWith) {
        return Err("world-invalid-payload".into());
    }
    if matches!(p.relation_type, RelationTypeV2::CorrelatesWith) && p.correlation_sign.is_none() {
        return Err("world-invalid-payload".into());
    }
    if p.comparison_id.is_some()
        && !matches!(
            p.relation_type,
            RelationTypeV2::Increases | RelationTypeV2::Decreases
        )
    {
        return Err("world-invalid-payload".into());
    }
    Ok(sorted)
}

pub fn check_focus_v2_struct(p: &FocusPayloadV2) -> Result<(), String> {
    if p.schema_version != WORLD_V2_SCHEMA_VERSION {
        return Err("world-invalid-payload".into());
    }
    check_id(&p.entity_id)?;
    match p.reason {
        FocusReason::CurrentWork => {
            let objective = p
                .objective_assertion_id
                .as_deref()
                .ok_or("world-invalid-payload")?;
            check_id(objective)?;
        }
        FocusReason::ExplicitInterest => {
            if p.objective_assertion_id.is_some() {
                return Err("world-invalid-payload".into());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn d06_v2_relation_roundtrip_and_version_rejection() {
        let payload = RelationPayloadV2 {
            payload_type: RelationTag::Relation,
            schema_version: 2,
            from_entity_id: "a".into(),
            to_entity_id: "b".into(),
            relation_type: RelationTypeV2::CorrelatesWith,
            effect_input: None,
            conditions: Vec::new(),
            comparison_id: None,
            basis: Basis::ModelHypothesis,
            evidence_stances: vec![EvidenceStance {
                source: SourceKey {
                    id: "s1".into(),
                    version: 1,
                    start: 0,
                    end: 1,
                },
                stance: super::super::model::Stance::Context,
            }],
            target_direction: None,
            correlation_sign: Some(CorrelationSign::Negative),
            epistemic: Epistemic::Hypothesis,
            confidence: None,
            strength: None,
            assessment_refs: Vec::new(),
            mechanism: MechanismState::Unassessed,
            outcome_update: None,
        };
        let value = serde_json::to_value(&payload).unwrap();
        let decoded = WorldPayloadV2::decode("world_relation", &value).unwrap();
        assert_eq!(decoded, WorldPayloadV2::Relation(payload));

        let mut wrong = value.clone();
        wrong["schema_version"] = serde_json::json!(3);
        assert!(WorldPayloadV2::decode("world_relation", &wrong).is_err());

        let mut unknown = value.clone();
        unknown["extra"] = serde_json::json!(true);
        assert!(WorldPayloadV2::decode("world_relation", &unknown).is_err());
    }

    #[test]
    fn d06_goal_entity_requires_objective() {
        let mut payload = EntityPayloadV2 {
            payload_type: EntityTag::Entity,
            schema_version: 2,
            entity_id: "g1".into(),
            entity_kind: EntityKindV2::Goal,
            name: "Goal".into(),
            aliases: Vec::new(),
            objective_assertion_id: None,
        };
        assert!(check_entity_v2_struct(&payload).is_err());
        payload.objective_assertion_id = Some("o1".into());
        assert!(check_entity_v2_struct(&payload).is_ok());
    }

    #[test]
    fn d06_confidence_range_is_checked() {
        let mut payload = RelationPayloadV2 {
            payload_type: RelationTag::Relation,
            schema_version: 2,
            from_entity_id: "a".into(),
            to_entity_id: "b".into(),
            relation_type: RelationTypeV2::Increases,
            effect_input: Some(EffectInput::Intervention),
            conditions: Vec::new(),
            comparison_id: None,
            basis: Basis::ModelHypothesis,
            evidence_stances: vec![EvidenceStance {
                source: SourceKey {
                    id: "s1".into(),
                    version: 1,
                    start: 0,
                    end: 1,
                },
                stance: super::super::model::Stance::Context,
            }],
            target_direction: None,
            correlation_sign: None,
            epistemic: Epistemic::Hypothesis,
            confidence: Some(Confidence {
                value: 1001,
                method: ConfidenceMethod::ManualV1,
            }),
            strength: None,
            assessment_refs: vec![SourceKey {
                id: "s1".into(),
                version: 1,
                start: 0,
                end: 1,
            }],
            mechanism: MechanismState::Unassessed,
            outcome_update: None,
        };
        assert!(check_relation_v2_struct(&payload).is_err());
        payload.confidence.as_mut().unwrap().value = 1000;
        assert!(check_relation_v2_struct(&payload).is_ok());
    }
}

#[cfg(test)]
mod optional_field_tests {
    use super::*;

    #[test]
    fn d06_missing_optional_fields_decode_as_null() {
        // `effect_input`, `comparison_id`, `target_direction`, `correlation_sign`,
        // `confidence`, `strength`, `outcome_update` omitted: all become null.
        let value = serde_json::json!({
            "type": "relation",
            "schema_version": 2,
            "from_entity_id": "a",
            "to_entity_id": "b",
            "relation_type": "related_to",
            "conditions": [],
            "basis": "model_hypothesis",
            "evidence_stances": [{"source": {"id": "s1", "version": 1, "start": 0, "end": 1}, "stance": "context"}],
            "epistemic": "hypothesis",
            "assessment_refs": [],
            "mechanism": "unassessed"
        });
        let decoded = WorldPayloadV2::decode("world_relation", &value).unwrap();
        let WorldPayloadV2::Relation(relation) = decoded else {
            panic!("relation")
        };
        assert!(relation.effect_input.is_none());
        assert!(relation.comparison_id.is_none());
        assert!(relation.target_direction.is_none());
        assert!(relation.correlation_sign.is_none());
        assert!(relation.confidence.is_none());
        assert!(relation.strength.is_none());
        assert!(relation.outcome_update.is_none());
        // Canonical output must still emit the keys as null.
        let canonical = serde_json::to_value(&relation).unwrap();
        assert!(canonical.get("effect_input").is_some());
        assert!(canonical["effect_input"].is_null());
    }
}
