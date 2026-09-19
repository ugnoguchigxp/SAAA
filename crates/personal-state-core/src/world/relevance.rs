//! Focus ordering, ResearchGap candidates and byte-budget trimming (WM-12/WM-13).

use crate::world::model::{
    ResearchGap, SliceNode, SlicePath, SliceRelation, SliceStep, WorldSlice,
};
use crate::world::traversal::{WorldEdge, WorldPath};
use crate::world::validation::WorldError;
use sha2::{Digest, Sha256};

pub const MAX_SLICE_BYTES: usize = 8_192;

pub fn focus_rank(reason: &str) -> u8 {
    match reason {
        "current_work" => 0,
        "explicit_interest" => 1,
        _ => 2,
    }
}

pub fn gap_key(
    project_scope: &str,
    relation_semantic_key: &str,
    focus_entity_id: &str,
    reason: &str,
) -> String {
    let input = serde_json::json!([
        project_scope,
        relation_semantic_key,
        focus_entity_id,
        reason
    ]);
    format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&input).expect("json"))
    )
}

/// Fixed question templates per gap reason, assembled from entity names and
/// conditions. No LLM call and no DB persistence.
pub fn gap_question(reason: &str, focus_name: &str, conditions: &[(String, String)]) -> String {
    let rendered = if conditions.is_empty() {
        "条件未指定".to_string()
    } else {
        conditions
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join(",")
    };
    match reason {
        "disputed" => format!(
            "「{focus_name}」に至る関係は支持と反証が競合しています。どちらが現在の構成で成立するか確認が必要です（{rendered}）。"
        ),
        "conditions_unverified" => format!(
            "「{focus_name}」に至る作用関係は適用条件が未確認です。現在の構成で成立するか計測が必要です（{rendered}）。"
        ),
        _ => format!(
            "「{focus_name}」に至る関係はモデル仮説です。未導入時との比較計測が必要です（{rendered}）。"
        ),
    }
}

/// One causal edge reached through a specific Focus entity, with the ranking
/// inputs the plan fixes (Focus区分 -> 経路長 -> relation assertion ID -> key).
pub struct GapInput<'a> {
    pub focus_entity_id: String,
    pub focus_rank: u8,
    pub path_len: usize,
    pub edge: &'a WorldEdge,
}

/// ResearchGap candidates for causal relations that lie on a relevant path to a
/// Focus entity. Each gap is tied to the exact `(focus, edge)` pair that
/// connected them, never a cross product of unrelated Focus entities (R6).
/// Ordering: gap reason priority, then Focus区分, path length, relation
/// assertion ID, then the stable key.
pub fn build_gaps(
    project_scope: &str,
    focus_names: &std::collections::BTreeMap<String, String>,
    relevant: &[GapInput<'_>],
) -> Vec<ResearchGap> {
    let mut gaps: Vec<(u8, u8, usize, String, ResearchGap)> = Vec::new();
    for input in relevant {
        let edge = input.edge;
        if !edge.is_causal() {
            continue;
        }
        let reason = if edge.status == crate::world::traversal::EdgeStatus::Disputed {
            "disputed"
        } else if edge.conditions.is_empty() {
            "conditions_unverified"
        } else if edge.basis == "model_hypothesis" {
            "hypothesis_unverified"
        } else {
            continue;
        };
        let priority = match reason {
            "disputed" => 0,
            "conditions_unverified" => 1,
            _ => 2,
        };
        let focus_name = focus_names
            .get(&input.focus_entity_id)
            .cloned()
            .unwrap_or_else(|| input.focus_entity_id.clone());
        gaps.push((
            priority,
            input.focus_rank,
            input.path_len,
            edge.assertion_id.clone(),
            ResearchGap {
                key: gap_key(
                    project_scope,
                    &edge.semantic_key,
                    &input.focus_entity_id,
                    reason,
                ),
                reason: reason.to_string(),
                relation_assertion_id: edge.assertion_id.clone(),
                focus_entity_id: input.focus_entity_id.clone(),
                question: gap_question(reason, &focus_name, &edge.conditions),
            },
        ));
    }
    gaps.sort_by(|a, b| (a.0, a.1, a.2, &a.3, &a.4.key).cmp(&(b.0, b.1, b.2, &b.3, &b.4.key)));
    let mut seen = std::collections::BTreeSet::new();
    gaps.into_iter()
        .filter(|(_, _, _, _, gap)| seen.insert(gap.key.clone()))
        .map(|(_, _, _, _, gap)| gap)
        .collect()
}

pub fn slice_path(path: &WorldPath, edges: &[WorldEdge]) -> SlicePath {
    SlicePath {
        nodes: path.nodes.clone(),
        steps: path
            .steps
            .iter()
            .map(|step| SliceStep {
                assertion_id: edges[step.edge].assertion_id.clone(),
                relation_type: edges[step.edge].relation_type.clone(),
                traversed_reverse: step.traversed_reverse,
            })
            .collect(),
        conditions_unverified: path.conditions_unverified,
        truncated: path.truncated,
    }
}

pub fn node(entity_id: &str, name: &str, kind: crate::world::model::EntityKind) -> SliceNode {
    SliceNode {
        entity_id: entity_id.to_string(),
        name: name.to_string(),
        entity_kind: kind,
    }
}

pub fn relation(edge: &WorldEdge) -> SliceRelation {
    SliceRelation {
        assertion_id: edge.assertion_id.clone(),
        semantic_key: edge.semantic_key.clone(),
        from_entity_id: edge.from.clone(),
        to_entity_id: edge.to.clone(),
        relation_type: edge.relation_type.clone(),
        status: match edge.status {
            crate::world::traversal::EdgeStatus::Active => "active",
            crate::world::traversal::EdgeStatus::Disputed => "disputed",
        }
        .to_string(),
        basis: edge.basis.clone(),
        conditions: edge
            .conditions
            .iter()
            .map(|(k, v)| vec![k.clone(), v.clone()])
            .collect(),
        evidence: Vec::new(),
    }
}

/// Keep only nodes/relations still referenced by a retained path, Focus or Gap,
/// and drop any Focus/Gap whose referenced node or relation is absent, so the
/// returned slice never names something it cannot explain (R9).
fn prune(slice: &mut WorldSlice) {
    let mut node_ids: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut assertion_ids: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for path in slice
        .relevance_paths
        .iter()
        .chain(slice.causal_paths.iter())
    {
        node_ids.extend(path.nodes.iter().cloned());
        assertion_ids.extend(path.steps.iter().map(|s| s.assertion_id.clone()));
    }
    node_ids.extend(slice.focus.iter().map(|f| f.entity_id.clone()));
    node_ids.extend(
        slice
            .research_gaps
            .iter()
            .map(|g| g.focus_entity_id.clone()),
    );
    assertion_ids.extend(
        slice
            .research_gaps
            .iter()
            .map(|g| g.relation_assertion_id.clone()),
    );
    slice.nodes.retain(|n| node_ids.contains(&n.entity_id));
    slice
        .relations
        .retain(|r| assertion_ids.contains(&r.assertion_id));
    let present_nodes: std::collections::BTreeSet<String> =
        slice.nodes.iter().map(|n| n.entity_id.clone()).collect();
    slice.focus.retain(|f| present_nodes.contains(&f.entity_id));
    slice
        .research_gaps
        .retain(|g| present_nodes.contains(&g.focus_entity_id));
}

/// Drop lowest-priority content until the slice encodes within `max_bytes`.
/// Never truncate mid-JSON and never strip the evidence/condition/hypothesis
/// labels from a retained path.
pub fn trim_to_budget(mut slice: WorldSlice, max_bytes: usize) -> Result<WorldSlice, WorldError> {
    if max_bytes < 256 {
        return Err(WorldError::BudgetTooSmall);
    }
    // Check referential integrity before and after every drop so a trimmed slice
    // never returns a Focus or Gap whose node/relation is gone (R9).
    prune(&mut slice);
    let encode = |slice: &WorldSlice| slice.encoded_len().unwrap_or(usize::MAX);
    if encode(&slice) <= max_bytes {
        return Ok(slice);
    }
    // Causal paths are the most expensive and lowest priority, then relevance
    // paths, then Gap candidates. Nodes/relations are pruned after every drop.
    loop {
        if encode(&slice) <= max_bytes {
            return Ok(slice);
        }
        if slice.causal_paths.pop().is_some()
            || slice.relevance_paths.pop().is_some()
            || slice.research_gaps.pop().is_some()
            || slice.focus.pop().is_some()
        {
            slice.truncated = true;
            prune(&mut slice);
            continue;
        }
        break;
    }
    if encode(&slice) > max_bytes {
        return Err(WorldError::BudgetTooSmall);
    }
    Ok(slice)
}
