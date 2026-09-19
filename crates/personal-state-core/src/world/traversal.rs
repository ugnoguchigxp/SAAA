//! IO-free World graph traversal (WM-10/WM-11).

use std::collections::{BTreeMap, BTreeSet, VecDeque};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeStatus {
    Active,
    Disputed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorldEdge {
    pub assertion_id: String,
    pub semantic_key: String,
    pub from: String,
    pub to: String,
    pub relation_type: String,
    pub status: EdgeStatus,
    pub conditions: Vec<(String, String)>,
    /// `user_statement` or `model_hypothesis`.
    pub basis: String,
}

impl WorldEdge {
    pub fn is_causal(&self) -> bool {
        matches!(self.relation_type.as_str(), "increases" | "decreases")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub depth: usize,
    pub nodes: usize,
    pub edges: usize,
    /// Return cap applied by the caller after Focus reachability and maximal-path
    /// selection. Traversal expansion is bounded by `scan`/`edges`/`nodes`, not by
    /// intermediate prefixes (R10).
    pub paths: usize,
    pub scan: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            depth: MAX_DEPTH,
            nodes: MAX_NODES,
            edges: MAX_EDGES,
            paths: MAX_PATHS,
            scan: MAX_SCAN,
        }
    }
}

/// Fixed M1 exploration maxima (plan section 5.4). A caller may request less but
/// never more, so no request can turn a bounded query into an unbounded scan.
pub const MAX_DEPTH: usize = 3;
pub const MAX_NODES: usize = 30;
pub const MAX_EDGES: usize = 60;
pub const MAX_PATHS: usize = 10;
pub const MAX_SCAN: usize = 500;

impl Limits {
    pub fn capped(self) -> Self {
        Self {
            depth: self.depth.min(MAX_DEPTH),
            nodes: self.nodes.min(MAX_NODES),
            edges: self.edges.min(MAX_EDGES),
            paths: self.paths.min(MAX_PATHS),
            scan: self.scan.min(MAX_SCAN),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CausalDirection {
    Forward,
    Reverse,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathStep {
    pub edge: usize,
    /// True when traversed against the edge's declared from/to.
    pub traversed_reverse: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorldPath {
    pub nodes: Vec<String>,
    pub steps: Vec<PathStep>,
    /// True when a limit stopped this path from being extended.
    pub truncated: bool,
    /// A path touching an edge with no conditions is not treated as verified.
    pub conditions_unverified: bool,
}

impl WorldPath {
    /// Edge indices with traversal direction, used for deduplication.
    pub fn signature(&self) -> Vec<(usize, bool)> {
        self.steps
            .iter()
            .map(|s| (s.edge, s.traversed_reverse))
            .collect()
    }

    pub fn is_prefix_of(&self, other: &Self) -> bool {
        let a = self.signature();
        let b = other.signature();
        a.len() < b.len() && b.starts_with(&a)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TraversalOutcome {
    pub paths: Vec<WorldPath>,
    pub scanned: usize,
    pub truncated: bool,
    pub truncation_reason: Option<&'static str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TraversalMode {
    Related,
    Causal,
}

struct Neighbour {
    edge: usize,
    other: String,
    traversed_reverse: bool,
}

/// Deterministic breadth-first expansion. Neighbours are read in
/// (relation_type, other entity id, assertion id) order. Each path keeps its
/// own visited set so distinct valid paths survive; a single path never
/// revisits an entity.
pub fn traverse(
    edges: &[WorldEdge],
    seeds: &[String],
    limits: Limits,
    mode: TraversalMode,
    direction: CausalDirection,
) -> TraversalOutcome {
    let mut outcome = TraversalOutcome::default();
    let mut adjacency: BTreeMap<String, Vec<Neighbour>> = BTreeMap::new();
    for (index, edge) in edges.iter().enumerate() {
        let eligible = match mode {
            TraversalMode::Related => true,
            TraversalMode::Causal => edge.is_causal() && edge.status == EdgeStatus::Active,
        };
        if !eligible {
            continue;
        }
        let forward = Neighbour {
            edge: index,
            other: edge.to.clone(),
            traversed_reverse: false,
        };
        let reverse = Neighbour {
            edge: index,
            other: edge.from.clone(),
            traversed_reverse: true,
        };
        match mode {
            // Related traversal may follow an edge both ways. Causal traversal keeps a
            // direction: Forward follows declared from->to, Reverse follows to->from, so a
            // reverse search never returns the seed's own downstream results (R4).
            TraversalMode::Related => {
                adjacency
                    .entry(edge.from.clone())
                    .or_default()
                    .push(forward);
                adjacency.entry(edge.to.clone()).or_default().push(reverse);
            }
            TraversalMode::Causal if direction == CausalDirection::Forward => {
                adjacency
                    .entry(edge.from.clone())
                    .or_default()
                    .push(forward);
            }
            TraversalMode::Causal => {
                adjacency.entry(edge.to.clone()).or_default().push(reverse);
            }
        }
    }
    for list in adjacency.values_mut() {
        list.sort_by(|a, b| {
            let ea = &edges[a.edge];
            let eb = &edges[b.edge];
            (&ea.relation_type, &a.other, &ea.assertion_id).cmp(&(
                &eb.relation_type,
                &b.other,
                &eb.assertion_id,
            ))
        });
    }

    let mut seed_list: Vec<String> = seeds.to_vec();
    seed_list.sort();
    seed_list.dedup();
    if seed_list.is_empty() {
        return outcome;
    }
    let mut distinct_nodes: BTreeSet<String> = seed_list.iter().cloned().collect();
    let mut collected_edges = 0usize;

    let mut queue: VecDeque<(WorldPath, BTreeSet<String>)> = VecDeque::new();
    for seed in &seed_list {
        queue.push_back((
            WorldPath {
                nodes: vec![seed.clone()],
                steps: Vec::new(),
                truncated: false,
                conditions_unverified: false,
            },
            BTreeSet::from([seed.clone()]),
        ));
    }

    while let Some((path, path_visited)) = queue.pop_front() {
        let Some(current) = path.nodes.last().cloned() else {
            continue;
        };
        let Some(neighbours) = adjacency.get(&current) else {
            continue;
        };
        let depth_blocked = path.steps.len() >= limits.depth;
        for neighbour in neighbours {
            if depth_blocked {
                // Only a genuinely extendable path counts as truncated.
                if !path_visited.contains(&neighbour.other) {
                    outcome.truncated = true;
                    outcome.truncation_reason.get_or_insert("depth");
                }
                continue;
            }
            if outcome.scanned >= limits.scan {
                outcome.truncated = true;
                outcome.truncation_reason.get_or_insert("scan");
                break;
            }
            outcome.scanned += 1;
            if path_visited.contains(&neighbour.other) {
                // A single path must not revisit an entity.
                continue;
            }
            if collected_edges >= limits.edges {
                outcome.truncated = true;
                outcome.truncation_reason.get_or_insert("edges");
                break;
            }
            if !distinct_nodes.contains(&neighbour.other) {
                if distinct_nodes.len() >= limits.nodes {
                    outcome.truncated = true;
                    outcome.truncation_reason.get_or_insert("nodes");
                    break;
                }
                distinct_nodes.insert(neighbour.other.clone());
            }
            let edge = &edges[neighbour.edge];
            let mut next_path = path.clone();
            next_path.nodes.push(neighbour.other.clone());
            next_path.steps.push(PathStep {
                edge: neighbour.edge,
                traversed_reverse: neighbour.traversed_reverse,
            });
            if edge.conditions.is_empty() {
                next_path.conditions_unverified = true;
            }
            collected_edges += 1;
            let mut next_visited = path_visited.clone();
            next_visited.insert(neighbour.other.clone());
            outcome.paths.push(next_path.clone());
            queue.push_back((next_path, next_visited));
        }
    }
    deduplicate(&mut outcome.paths);
    outcome
}

fn deduplicate(paths: &mut Vec<WorldPath>) {
    let mut seen: BTreeSet<Vec<(usize, bool)>> = BTreeSet::new();
    paths.retain(|path| seen.insert(path.signature()));
}

/// True when every edge in the path shares exactly the same condition set. Order is
/// ignored so a logically-equal multi-condition set cannot split an otherwise valid path (R5).
pub fn conditions_consistent(edges: &[&WorldEdge]) -> bool {
    let mut reference: Option<Vec<(String, String)>> = None;
    for edge in edges {
        let mut sorted = edge.conditions.clone();
        sorted.sort();
        match &reference {
            None => reference = Some(sorted),
            Some(existing) => {
                if existing != &sorted {
                    return false;
                }
            }
        }
    }
    true
}

/// Drop paths that are strict prefixes of another retained path.
pub fn maximal_only(paths: &[WorldPath]) -> Vec<WorldPath> {
    paths
        .iter()
        .filter(|candidate| !paths.iter().any(|other| candidate.is_prefix_of(other)))
        .cloned()
        .collect()
}
