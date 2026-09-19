//! Pure World value types, normalization, keys, validation, traversal, relevance.

pub mod identity;
pub mod model;
pub mod relevance;
pub mod traversal;
pub mod validation;

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
