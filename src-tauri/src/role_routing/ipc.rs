//! Read-only routing IPC projections. SQLite remains the source of truth for reconnects.
use crate::AppState;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use tauri::Emitter;
use ts_rs::{Config, TS};
#[path = "ipc/routing_snapshot_input.rs"]
pub(crate) mod routing_snapshot_input;
#[path = "ipc/typescript_bindings.rs"]
mod typescript_bindings;
pub(crate) use routing_snapshot_input::{
    cancel_routing_root, decide_routing_proposal, get_routing_learning_snapshot,
    get_routing_snapshot, replay, replay_routing_events, rollback_adaptive_artifact,
    run_routing_learning_once, snapshot, AdaptiveArtifactSnapshot, AdaptiveRollbackInput,
    RoutingCancelInput, RoutingEventRecord, RoutingEventReplayInput, RoutingLearningSnapshot,
    RoutingProposalDecisionInput, RoutingProposalSnapshot, RoutingRootSnapshot, RoutingSnapshot,
    RoutingSnapshotInput,
};
use routing_snapshot_input::{learning_snapshot, root_snapshot};
use typescript_bindings::now_ms;
pub(crate) use typescript_bindings::typescript_bindings;
