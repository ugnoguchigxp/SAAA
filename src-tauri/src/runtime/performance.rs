use std::{
    collections::VecDeque,
    sync::{Mutex, OnceLock},
    time::Duration,
};

use serde_json::{json, Value};

const SAMPLE_LIMIT: usize = 4_096;

#[derive(Default)]
struct StreamingMetrics {
    socket_to_hub_ns: VecDeque<u64>,
    hub_to_tts_ns: VecDeque<u64>,
    tts_boundary_to_dispatch_ns: VecDeque<u64>,
    tts_boundary_to_player_spawn_ns: VecDeque<u64>,
    reconnects: u64,
    sequence_gaps: u64,
    max_tts_queue_depth: usize,
    ui_batches: u64,
}

fn metrics() -> &'static Mutex<StreamingMetrics> {
    static METRICS: OnceLock<Mutex<StreamingMetrics>> = OnceLock::new();
    METRICS.get_or_init(|| Mutex::new(StreamingMetrics::default()))
}

fn push_sample(samples: &mut VecDeque<u64>, duration: Duration) {
    if samples.len() == SAMPLE_LIMIT {
        samples.pop_front();
    }
    samples.push_back(duration.as_nanos().try_into().unwrap_or(u64::MAX));
}

pub(crate) fn record_socket_to_hub(duration: Duration) {
    let mut metrics = metrics()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    push_sample(&mut metrics.socket_to_hub_ns, duration);
}

pub(crate) fn record_hub_to_tts(duration: Duration) {
    let mut metrics = metrics()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    push_sample(&mut metrics.hub_to_tts_ns, duration);
}

pub(crate) fn record_tts_boundary_to_dispatch(duration: Duration) {
    let mut metrics = metrics()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    push_sample(&mut metrics.tts_boundary_to_dispatch_ns, duration);
}

pub(crate) fn record_tts_boundary_to_player_spawn(duration: Duration) {
    let mut metrics = metrics()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    push_sample(&mut metrics.tts_boundary_to_player_spawn_ns, duration);
}

pub(crate) fn record_reconnect() {
    metrics()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .reconnects += 1;
}

pub(crate) fn record_sequence_gap() {
    metrics()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .sequence_gaps += 1;
}

pub(crate) fn record_tts_queue_depth(depth: usize) {
    let mut metrics = metrics()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    metrics.max_tts_queue_depth = metrics.max_tts_queue_depth.max(depth);
}

pub(crate) fn record_ui_batch() {
    metrics()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .ui_batches += 1;
}

fn percentiles(samples: &VecDeque<u64>) -> Value {
    let mut values = samples.iter().copied().collect::<Vec<_>>();
    values.sort_unstable();
    let percentile = |percent: usize| {
        if values.is_empty() {
            return None;
        }
        let index = ((values.len() * percent).div_ceil(100)).saturating_sub(1);
        values.get(index).copied()
    };
    json!({
        "samples": values.len(),
        "p50Ns": percentile(50),
        "p95Ns": percentile(95),
        "p99Ns": percentile(99)
    })
}

pub(crate) fn snapshot() -> Value {
    let metrics = metrics()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    json!({
        "protocol": crate::providers::llm_websocket::protocol::SUBPROTOCOL,
        "socketReceiveToHubAccept": percentiles(&metrics.socket_to_hub_ns),
        "hubAcceptToTtsAppend": percentiles(&metrics.hub_to_tts_ns),
        "ttsBoundaryToDispatch": percentiles(&metrics.tts_boundary_to_dispatch_ns),
        "ttsBoundaryToPlayerSpawn": percentiles(&metrics.tts_boundary_to_player_spawn_ns),
        "reconnectCount": metrics.reconnects,
        "sequenceGapCount": metrics.sequence_gaps,
        "maxTtsQueueDepth": metrics.max_tts_queue_depth,
        "uiBatchCount": metrics.ui_batches
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_is_bounded_and_contains_no_turn_content_or_identifier() {
        for value in 1..=5_000 {
            record_socket_to_hub(Duration::from_nanos(value));
        }
        record_hub_to_tts(Duration::from_nanos(10));
        record_tts_boundary_to_dispatch(Duration::from_nanos(11));
        record_tts_boundary_to_player_spawn(Duration::from_nanos(12));
        record_reconnect();
        record_sequence_gap();
        record_tts_queue_depth(3);
        let snapshot = snapshot();
        assert_eq!(
            snapshot["socketReceiveToHubAccept"]["samples"],
            SAMPLE_LIMIT
        );
        assert_eq!(snapshot["maxTtsQueueDepth"], 3);
        let encoded = snapshot.to_string();
        assert!(!encoded.contains("runId"));
        assert!(!encoded.contains("content"));
    }
}
