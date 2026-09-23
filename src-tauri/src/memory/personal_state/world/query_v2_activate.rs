use super::*;
#[path = "query_v2_activate/edge_load.rs"]
mod edge_load;
#[path = "query_v2_activate/activate_v2_with_stats.rs"]
mod activate_v2_with_stats;
use edge_load::{EdgeLoad, load_edges_bfs, relation_to_slice, seed_text, slice_focus_of, goal_ids, project_ids, node_of, path_view};
pub use activate_v2_with_stats::{activate_v2_with_stats, activate_v2};
