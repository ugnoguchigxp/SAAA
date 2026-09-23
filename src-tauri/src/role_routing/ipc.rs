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
pub(crate) use routing_snapshot_input::{RoutingSnapshotInput, RoutingEventReplayInput, RoutingCancelInput, RoutingProposalDecisionInput, AdaptiveRollbackInput, AdaptiveArtifactSnapshot, RoutingLearningSnapshot, RoutingRootSnapshot, RoutingSnapshot, RoutingProposalSnapshot, RoutingEventRecord, get_routing_snapshot, replay_routing_events, cancel_routing_root, decide_routing_proposal, run_routing_learning_once, get_routing_learning_snapshot, rollback_adaptive_artifact, snapshot, replay};
use routing_snapshot_input::{learning_snapshot, root_snapshot};
pub(crate) use typescript_bindings::{typescript_bindings};
use typescript_bindings::{now_ms};
