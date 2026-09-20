//! Read-only routing IPC projections. SQLite remains the source of truth for reconnects.
use crate::AppState;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use tauri::Emitter;
use ts_rs::{Config, TS};

#[derive(Debug, Clone, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RoutingSnapshotInput {
    pub(crate) conversation_id: String,
}

#[derive(Debug, Clone, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RoutingEventReplayInput {
    pub(crate) root_id: String,
    pub(crate) after_seq: i64,
}

#[derive(Debug, Clone, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RoutingCancelInput {
    pub(crate) root_id: String,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RoutingLearningSnapshot {
    pub(crate) dirty_roots: i64,
    pub(crate) ready_datasets: i64,
    pub(crate) active_artifacts: i64,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RoutingRootSnapshot {
    pub(crate) root_id: String,
    pub(crate) runtime_run_id: Option<String>,
    pub(crate) phase: String,
    pub(crate) revision: u32,
    pub(crate) active_slot: Option<String>,
    pub(crate) cancel_requested: bool,
    pub(crate) last_event_seq: i64,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RoutingSnapshot {
    pub(crate) active: Option<RoutingRootSnapshot>,
    pub(crate) queued: Vec<RoutingRootSnapshot>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RoutingEventRecord {
    pub(crate) root_id: String,
    pub(crate) seq: i64,
    pub(crate) kind: String,
    pub(crate) data_json: String,
    pub(crate) created_at_ms: i64,
}

#[tauri::command]
pub(crate) fn get_routing_snapshot(
    state: tauri::State<'_, AppState>,
    input: RoutingSnapshotInput,
) -> Result<RoutingSnapshot, String> {
    crate::validate_identifier(&input.conversation_id, "conversation id")?;
    state
        .sqlite_readers
        .read(|connection| snapshot(connection, &input.conversation_id))
}

#[tauri::command]
pub(crate) fn replay_routing_events(
    state: tauri::State<'_, AppState>,
    input: RoutingEventReplayInput,
) -> Result<Vec<RoutingEventRecord>, String> {
    crate::validate_identifier(&input.root_id, "routing root id")?;
    if input.after_seq < 0 {
        return Err("Routing event sequence cannot be negative".into());
    }
    state
        .sqlite_readers
        .read(|connection| replay(connection, &input.root_id, input.after_seq))
}

/// Persists cancellation before signalling the local run.  A reconnect can therefore always
/// observe the cancellation even when the process stops immediately after this command returns.
#[tauri::command]
pub(crate) fn cancel_routing_root(
    state: tauri::State<'_, crate::AppState>,
    app: tauri::AppHandle,
    input: RoutingCancelInput,
) -> Result<RoutingRootSnapshot, String> {
    crate::validate_identifier(&input.root_id, "routing root id")?;
    let (runtime_run_id, root) = state.sqlite_writer.write(|connection| {
        let runtime_run_id: Option<String> = connection
            .query_row(
                "SELECT runtime_run_id FROM rr_roots WHERE root_id=?1",
                [&input.root_id],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "Role-routing root was not found".to_string())?;
        crate::role_routing::coordinator::apply(
            connection,
            &input.root_id,
            crate::role_routing::reducer::Event::Cancel,
            now_ms(),
        )?;
        Ok((runtime_run_id, root_snapshot(connection, &input.root_id)?))
    })?;
    if let Some(run_id) = runtime_run_id {
        if let Ok(active) = state.active_runs.lock() {
            if let Some(cancellation) = active.get(&run_id) {
                cancellation.cancel();
            }
        }
        state.streaming_tts.cancel(&run_id);
    }
    // The durable snapshot is already committed. Event delivery is best-effort; reporting an
    // emitter failure here would incorrectly tell the caller that its cancellation failed.
    let _ = app.emit(
        "role-routing-updated",
        serde_json::json!({"rootId":root.root_id}),
    );
    Ok(root)
}

/// Runs one locally authorized materialization batch. It is intentionally a database-only
/// operation: no provider, labeler, or artifact activation is performed on the UI thread.
#[tauri::command]
pub(crate) fn run_routing_learning_once(
    state: tauri::State<'_, AppState>,
) -> Result<RoutingLearningSnapshot, String> {
    state.sqlite_writer.write(|connection| {
        let settings = crate::persistence::load_role_routing_settings(connection)?;
        if !settings.learning.enabled {
            return Err("Role-routing learning is disabled".into());
        }
        let _ = crate::role_routing::learning::repository::materialize_dirty_roots(
            connection,
            now_ms(),
            settings.learning.batch_size,
        )?;
        learning_snapshot(connection)
    })
}

#[tauri::command]
pub(crate) fn get_routing_learning_snapshot(
    state: tauri::State<'_, AppState>,
) -> Result<RoutingLearningSnapshot, String> {
    state.sqlite_readers.read(learning_snapshot)
}

fn learning_snapshot(connection: &Connection) -> Result<RoutingLearningSnapshot, String> {
    let (dirty_roots, ready_datasets, active_artifacts) = connection
        .query_row(
            "SELECT
                (SELECT count(*) FROM rr_learning_dirty),
                (SELECT count(*) FROM rr_datasets WHERE state='ready'),
                (SELECT count(*) FROM rr_ranker_artifacts WHERE state IN ('candidate','shadow'))",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(|error| error.to_string())?;
    Ok(RoutingLearningSnapshot {
        dirty_roots,
        ready_datasets,
        active_artifacts,
    })
}

pub(crate) fn snapshot(
    connection: &Connection,
    conversation_id: &str,
) -> Result<RoutingSnapshot, String> {
    let mut statement = connection
        .prepare(
            "SELECT root_id,runtime_run_id,phase,revision,active_slot,cancel_requested,
                    COALESCE((SELECT MAX(seq) FROM rr_events WHERE root_id=rr_roots.root_id),0)
             FROM rr_roots WHERE conversation_id=?1 AND phase IN ('queued','responding','draining')
             ORDER BY CASE phase WHEN 'queued' THEN 1 ELSE 0 END, started_at_ms, root_id",
        )
        .map_err(|error| error.to_string())?;
    let roots = statement
        .query_map([conversation_id], |row| {
            Ok(RoutingRootSnapshot {
                root_id: row.get(0)?,
                runtime_run_id: row.get(1)?,
                phase: row.get(2)?,
                revision: row.get(3)?,
                active_slot: row.get(4)?,
                cancel_requested: row.get::<_, i64>(5)? != 0,
                last_event_seq: row.get(6)?,
            })
        })
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    let mut active = None;
    let mut queued = Vec::new();
    for root in roots {
        if root.phase == "queued" {
            queued.push(root);
        } else if active.is_none() {
            active = Some(root);
        }
    }
    Ok(RoutingSnapshot { active, queued })
}

fn root_snapshot(connection: &Connection, root_id: &str) -> Result<RoutingRootSnapshot, String> {
    connection
        .query_row(
            "SELECT root_id,runtime_run_id,phase,revision,active_slot,cancel_requested,
                    COALESCE((SELECT MAX(seq) FROM rr_events WHERE root_id=rr_roots.root_id),0)
             FROM rr_roots WHERE root_id=?1",
            [root_id],
            |row| {
                Ok(RoutingRootSnapshot {
                    root_id: row.get(0)?,
                    runtime_run_id: row.get(1)?,
                    phase: row.get(2)?,
                    revision: row.get(3)?,
                    active_slot: row.get(4)?,
                    cancel_requested: row.get::<_, i64>(5)? != 0,
                    last_event_seq: row.get(6)?,
                })
            },
        )
        .optional()
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "Role-routing root was not found".into())
}

pub(crate) fn replay(
    connection: &Connection,
    root_id: &str,
    after_seq: i64,
) -> Result<Vec<RoutingEventRecord>, String> {
    connection
        .prepare("SELECT root_id,seq,kind,data_json,created_at_ms FROM rr_events WHERE root_id=?1 AND seq>?2 ORDER BY seq LIMIT 256")
        .map_err(|error| error.to_string())?
        .query_map(params![root_id, after_seq], |row| Ok(RoutingEventRecord { root_id: row.get(0)?, seq: row.get(1)?, kind: row.get(2)?, data_json: row.get(3)?, created_at_ms: row.get(4)? }))
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())
}

pub(crate) fn typescript_bindings() -> String {
    fn declaration<T: TS>() -> String {
        format!("export {}", T::decl(&Config::default()))
    }
    [
        declaration::<RoutingSnapshotInput>(),
        declaration::<RoutingEventReplayInput>(),
        declaration::<RoutingCancelInput>(),
        declaration::<RoutingLearningSnapshot>(),
        declaration::<RoutingRootSnapshot>(),
        declaration::<RoutingSnapshot>(),
        declaration::<RoutingEventRecord>(),
    ]
    .join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rr_14_snapshot_and_replay_preserve_queue_order() {
        let connection = Connection::open_in_memory().expect("connection");
        connection.execute_batch("PRAGMA foreign_keys=ON; CREATE TABLE conversations(id TEXT PRIMARY KEY); CREATE TABLE runtime_runs(id TEXT PRIMARY KEY); CREATE TABLE conversation_messages(id TEXT PRIMARY KEY); INSERT INTO conversations VALUES('c');").expect("base");
        crate::role_routing::schema::migrate(&connection).expect("schema");
        connection
            .execute(
                "INSERT INTO rr_policy_versions VALUES('p',1,'{}','d',1)",
                [],
            )
            .expect("policy");
        connection.execute_batch("INSERT INTO rr_roots(root_id,conversation_id,policy_id,phase,origin,presentation_mode,started_at_ms,scope_digest) VALUES('active','c','p','responding','text','visual',1,''); INSERT INTO rr_roots(root_id,conversation_id,policy_id,phase,origin,presentation_mode,started_at_ms,scope_digest) VALUES('later','c','p','queued','text','visual',3,''); INSERT INTO rr_roots(root_id,conversation_id,policy_id,phase,origin,presentation_mode,started_at_ms,scope_digest) VALUES('first','c','p','queued','text','visual',2,''); INSERT INTO rr_events VALUES('active',1,'root_started','{}',1); INSERT INTO rr_events VALUES('active',2,'activity','{}',2);").expect("rows");
        let projection = snapshot(&connection, "c").expect("snapshot");
        assert_eq!(projection.active.expect("active").root_id, "active");
        assert_eq!(
            projection
                .queued
                .iter()
                .map(|root| root.root_id.as_str())
                .collect::<Vec<_>>(),
            vec!["first", "later"]
        );
        assert_eq!(
            replay(&connection, "active", 1).expect("replay")[0].kind,
            "activity"
        );
    }

    #[test]
    fn rr_27_cancel_is_durable_and_replayable() {
        let mut connection = Connection::open_in_memory().expect("connection");
        connection.execute_batch("PRAGMA foreign_keys=ON; CREATE TABLE conversations(id TEXT PRIMARY KEY); CREATE TABLE runtime_runs(id TEXT PRIMARY KEY); CREATE TABLE conversation_messages(id TEXT PRIMARY KEY); INSERT INTO conversations VALUES('c');").expect("base");
        crate::role_routing::schema::migrate(&connection).expect("schema");
        connection
            .execute(
                "INSERT INTO rr_policy_versions VALUES('p',1,'{}','d',1)",
                [],
            )
            .expect("policy");
        connection.execute_batch("INSERT INTO rr_roots(root_id,conversation_id,policy_id,phase,origin,presentation_mode,started_at_ms,scope_digest) VALUES('r','c','p','responding','text','visual',1,''); INSERT INTO rr_events VALUES('r',1,'root_started','{}',1);").expect("root");
        crate::role_routing::coordinator::apply(
            &mut connection,
            "r",
            crate::role_routing::reducer::Event::Cancel,
            2,
        )
        .expect("cancel");
        let root = root_snapshot(&connection, "r").expect("snapshot");
        assert_eq!(root.phase, "cancelled");
        assert!(root.cancel_requested);
        assert_eq!(root.last_event_seq, 2);
        assert_eq!(
            replay(&connection, "r", 1).expect("events")[0].kind,
            "root_cancelled"
        );
    }

    #[test]
    fn rr_37_learning_snapshot_contains_counts_only() {
        let connection = Connection::open_in_memory().expect("connection");
        connection
            .execute_batch(
                "CREATE TABLE rr_roots(root_id TEXT PRIMARY KEY);
                 CREATE TABLE rr_decisions(id TEXT PRIMARY KEY);",
            )
            .expect("base");
        crate::role_routing::learning::schema::migrate(&connection).expect("learning schema");
        connection
            .execute_batch(
                "INSERT INTO rr_datasets(id,upper_seq,feature_version,labeler_version,manifest_json,state,created_at_ms) VALUES('ready',0,'f','l','{}','ready',1);
                 INSERT INTO rr_ranker_artifacts(id,dataset_id,algorithm,feature_version,candidate_fingerprint,weights_json,metrics_json,digest,state,created_at_ms) VALUES('artifact','ready','linear-v1','f','c','{}','{}','d','shadow',1);",
            )
            .expect("learning rows");
        let snapshot = learning_snapshot(&connection).expect("snapshot");
        assert_eq!(snapshot.dirty_roots, 0);
        assert_eq!(snapshot.ready_datasets, 1);
        assert_eq!(snapshot.active_artifacts, 1);
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}
