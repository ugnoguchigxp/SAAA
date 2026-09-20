//! Versioned decoding and a common read-only view (D08).
//!
//! v1 wire JSON stays exactly as decoded by `model.rs`; v2 is decoded by
//! `model_v2.rs`. Both are normalized into a v2-shaped view for identity and
//! explanation, while preserving the original `schema_version` and
//! `semantic_key`. Normalizing never re-saves a v1 payload as v2.

use super::model::{
    self, EntityPayload, FocusPayload, RelationPayload, WorldPayload, WORLD_SCHEMA_VERSION,
};
use super::model_v2::{
    self, EntityPayloadV2, FocusPayloadV2, RelationPayloadV2, RelationTypeV2, WorldPayloadV2,
    WORLD_V2_SCHEMA_VERSION,
};
use super::validation::WorldError;
use sha2::{Digest, Sha256};

pub const INVALID_PAYLOAD: &str = "world-invalid-payload";
pub const WORLD_V2_PAYLOAD_REF_PREFIX: &str = "wm2-payload-";

/// Content-addressed reference for a v2 payload (C5). The canonical bytes are
/// the typed DTO's field order with `Option` rendered as `null`.
pub fn payload_ref_v2(bytes: &[u8]) -> String {
    format!("{}{:x}", WORLD_V2_PAYLOAD_REF_PREFIX, Sha256::digest(bytes))
}

/// Reject a v2 assertion whose `payload_ref` is not the content hash of its
/// payload. v1 references keep their existing rule.
pub fn verify_payload_ref(payload_ref: &str, payload: &VersionedWorldPayload) -> bool {
    match payload {
        VersionedWorldPayload::V1(_) => true,
        VersionedWorldPayload::V2(payload) => payload
            .canonical_bytes()
            .is_ok_and(|bytes| payload_ref == payload_ref_v2(&bytes)),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(clippy::large_enum_variant)]
pub enum VersionedWorldPayload {
    V1(WorldPayload),
    V2(WorldPayloadV2),
}

impl VersionedWorldPayload {
    pub fn schema_version(&self) -> i64 {
        match self {
            Self::V1(_) => WORLD_SCHEMA_VERSION,
            Self::V2(_) => WORLD_V2_SCHEMA_VERSION,
        }
    }

    /// Normalize to a v2-shaped view. v1 relations read as `hypothesis` with no
    /// confidence/strength/comparison/mechanism so they are never mistaken for
    /// evaluated data.
    pub fn view(&self, semantic_key: &str) -> WorldView {
        match self {
            Self::V1(payload) => match payload {
                WorldPayload::Entity(p) => WorldView::Entity(WorldEntityView {
                    schema_version: WORLD_SCHEMA_VERSION,
                    semantic_key: semantic_key.to_string(),
                    payload: entity_to_v2(p),
                }),
                WorldPayload::Relation(p) => WorldView::Relation(WorldRelationView {
                    schema_version: WORLD_SCHEMA_VERSION,
                    semantic_key: semantic_key.to_string(),
                    payload: relation_to_v2(p),
                }),
                WorldPayload::Focus(p) => WorldView::Focus(WorldFocusView {
                    schema_version: WORLD_SCHEMA_VERSION,
                    semantic_key: semantic_key.to_string(),
                    payload: focus_to_v2(p),
                }),
            },
            Self::V2(payload) => match payload {
                WorldPayloadV2::Entity(p) => WorldView::Entity(WorldEntityView {
                    schema_version: WORLD_V2_SCHEMA_VERSION,
                    semantic_key: semantic_key.to_string(),
                    payload: p.clone(),
                }),
                WorldPayloadV2::Relation(p) => WorldView::Relation(WorldRelationView {
                    schema_version: WORLD_V2_SCHEMA_VERSION,
                    semantic_key: semantic_key.to_string(),
                    payload: p.clone(),
                }),
                WorldPayloadV2::Focus(p) => WorldView::Focus(WorldFocusView {
                    schema_version: WORLD_V2_SCHEMA_VERSION,
                    semantic_key: semantic_key.to_string(),
                    payload: p.clone(),
                }),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(clippy::large_enum_variant)]
pub enum WorldView {
    Entity(WorldEntityView),
    Relation(WorldRelationView),
    Focus(WorldFocusView),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorldEntityView {
    pub schema_version: i64,
    pub semantic_key: String,
    pub payload: EntityPayloadV2,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorldRelationView {
    pub schema_version: i64,
    pub semantic_key: String,
    pub payload: RelationPayloadV2,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorldFocusView {
    pub schema_version: i64,
    pub semantic_key: String,
    pub payload: FocusPayloadV2,
}

fn entity_to_v2(p: &EntityPayload) -> EntityPayloadV2 {
    EntityPayloadV2 {
        payload_type: p.payload_type,
        schema_version: WORLD_V2_SCHEMA_VERSION,
        entity_id: p.entity_id.clone(),
        entity_kind: match p.entity_kind {
            model::EntityKind::Project => model_v2::EntityKindV2::Project,
            model::EntityKind::Concept => model_v2::EntityKindV2::Concept,
            model::EntityKind::Metric => model_v2::EntityKindV2::Metric,
        },
        name: p.name.clone(),
        aliases: p.aliases.clone(),
        objective_assertion_id: None,
    }
}

fn relation_to_v2(p: &RelationPayload) -> RelationPayloadV2 {
    RelationPayloadV2 {
        payload_type: p.payload_type,
        schema_version: WORLD_V2_SCHEMA_VERSION,
        from_entity_id: p.from_entity_id.clone(),
        to_entity_id: p.to_entity_id.clone(),
        relation_type: match p.relation_type {
            model::RelationType::RelatedTo => RelationTypeV2::RelatedTo,
            model::RelationType::PartOf => RelationTypeV2::PartOf,
            model::RelationType::DependsOn => RelationTypeV2::DependsOn,
            model::RelationType::ImportantFor => RelationTypeV2::ImportantFor,
            model::RelationType::Increases => RelationTypeV2::Increases,
            model::RelationType::Decreases => RelationTypeV2::Decreases,
        },
        effect_input: p.effect_input,
        conditions: p.conditions.clone(),
        comparison_id: None,
        basis: p.basis,
        evidence_stances: p.evidence_stances.clone(),
        target_direction: None,
        correlation_sign: None,
        epistemic: model_v2::Epistemic::Hypothesis,
        confidence: None,
        strength: None,
        assessment_refs: Vec::new(),
        mechanism: model_v2::MechanismState::Unassessed,
        outcome_update: None,
    }
}

fn focus_to_v2(p: &FocusPayload) -> FocusPayloadV2 {
    FocusPayloadV2 {
        payload_type: p.payload_type,
        schema_version: WORLD_V2_SCHEMA_VERSION,
        entity_id: p.entity_id.clone(),
        reason: p.reason,
        objective_assertion_id: p.objective_assertion_id.clone(),
    }
}

/// Read `schema_version` from an already-parsed JSON object, then dispatch to
/// the matching decoder. A missing/invalid version or an unsupported version is
/// `world-invalid-payload`.
pub fn decode_versioned(
    kind_name: &str,
    value: &serde_json::Value,
) -> Result<VersionedWorldPayload, WorldError> {
    let version = value
        .get("schema_version")
        .and_then(|v| v.as_i64())
        .ok_or(WorldError::InvalidPayload)?;
    match version {
        WORLD_SCHEMA_VERSION => WorldPayload::decode(kind_name, value)
            .map(VersionedWorldPayload::V1)
            .map_err(|_| WorldError::InvalidPayload),
        WORLD_V2_SCHEMA_VERSION => WorldPayloadV2::decode(kind_name, value)
            .map(VersionedWorldPayload::V2)
            .map_err(|_| WorldError::InvalidPayload),
        _ => Err(WorldError::InvalidPayload),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SourceKey;
    use serde_json::json;

    fn v1_relation() -> serde_json::Value {
        json!({
            "type": "relation",
            "schema_version": 1,
            "from_entity_id": "a",
            "to_entity_id": "b",
            "relation_type": "increases",
            "effect_input": "intervention",
            "conditions": [],
            "basis": "model_hypothesis",
            "evidence_stances": [{"source": {"id": "s1", "version": 1, "start": 0, "end": 1}, "stance": "context"}]
        })
    }

    #[test]
    fn d08_v1_stays_v1_and_reads_unevaluated() {
        let decoded = decode_versioned("world_relation", &v1_relation()).unwrap();
        assert!(matches!(decoded, VersionedWorldPayload::V1(_)));
        match decoded.view("wm1:key") {
            WorldView::Relation(view) => {
                assert_eq!(view.schema_version, 1);
                assert_eq!(view.semantic_key, "wm1:key");
                assert_eq!(view.payload.epistemic, model_v2::Epistemic::Hypothesis);
                assert!(view.payload.confidence.is_none());
                assert!(view.payload.comparison_id.is_none());
                assert_eq!(view.payload.mechanism, model_v2::MechanismState::Unassessed);
            }
            _ => panic!("expected relation view"),
        }
    }

    #[test]
    fn d08_unknown_version_and_field_are_rejected() {
        let mut value = v1_relation();
        value["schema_version"] = json!(3);
        assert_eq!(
            decode_versioned("world_relation", &value).unwrap_err(),
            WorldError::InvalidPayload
        );

        let mut value = v1_relation();
        value["schema_version"] = json!(2);
        value["unexpected"] = json!(1);
        assert_eq!(
            decode_versioned("world_relation", &value).unwrap_err(),
            WorldError::InvalidPayload
        );
    }

    #[test]
    fn d08_v2_decodes_as_v2() {
        let value = json!({
            "type": "relation",
            "schema_version": 2,
            "from_entity_id": "a",
            "to_entity_id": "b",
            "relation_type": "correlates_with",
            "effect_input": null,
            "conditions": [],
            "comparison_id": null,
            "basis": "model_hypothesis",
            "evidence_stances": [{"source": {"id": "s1", "version": 1, "start": 0, "end": 1}, "stance": "context"}],
            "target_direction": null,
            "correlation_sign": "negative",
            "epistemic": "hypothesis",
            "confidence": null,
            "strength": null,
            "assessment_refs": [],
            "mechanism": "unassessed",
            "outcome_update": null
        });
        let decoded = decode_versioned("world_relation", &value).unwrap();
        assert!(matches!(decoded, VersionedWorldPayload::V2(_)));
    }

    #[test]
    fn d08_evidence_source_key_type_matches() {
        let key = SourceKey {
            id: "s1".into(),
            version: 1,
            start: 0,
            end: 1,
        };
        assert_eq!(key.id, "s1");
    }
}
