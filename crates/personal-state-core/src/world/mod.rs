//! Pure World value types, normalization, keys, validation, traversal, relevance.

pub mod conditions_v2;
pub mod identity;
pub mod identity_v2;
pub mod model;
pub mod model_v2;
pub mod outcome_v2;
pub mod relevance;
pub mod relevance_v2;
pub mod runtime_frame;
pub mod slice_v2;
pub mod traversal;
pub mod traversal_v2;
pub mod validation;
pub mod validation_v2;
pub mod versioned;

pub use identity::{
    entity_key, focus_key, normalize_name, order_undirected, relation_key, RelationKeyInput,
};
pub use model::{
    Basis, Condition, EffectInput, EntityKind, EntityPayload, EvidenceStance, FocusPayload,
    FocusReason, RelationPayload, RelationType, ResearchGap, SliceEvidence, SliceFocus, SliceNode,
    SlicePath, SliceRelation, SliceStep, Stance, WorldPayload, WorldSlice, MAX_ALIASES,
    MAX_CONDITIONS, MAX_EVIDENCE_STANCES, WORLD_SCHEMA_VERSION, WORLD_SEMANTIC_KEY_PREFIX,
};
pub use validation::{
    validate_world_patch, WorldError, WorldPatchInput, MAX_NEW_WORLD_ASSERTIONS,
    MAX_WORLD_ASSERTIONS, MAX_WORLD_SOURCES,
};
