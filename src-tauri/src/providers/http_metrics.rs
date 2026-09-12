//! Bounded transport measurements, without prompts, transcripts, URLs, or credentials.
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Mutex, OnceLock},
    time::Duration,
};
fn samples() -> &'static Mutex<BTreeMap<&'static str, VecDeque<u64>>> {
    static SAMPLES: OnceLock<Mutex<BTreeMap<&'static str, VecDeque<u64>>>> = OnceLock::new();
    SAMPLES.get_or_init(Mutex::default)
}
pub(crate) fn record(metric: &'static str, elapsed: Duration) {
    if let Ok(mut samples) = samples().lock() {
        let values = samples.entry(metric).or_default();
        if values.len() == 200 {
            values.pop_front();
        }
        values.push_back(elapsed.as_micros().min(u64::MAX as u128) as u64);
    }
}
pub(crate) fn snapshot() -> Value {
    let Ok(samples) = samples().lock() else {
        return json!({});
    };
    let mut output = serde_json::Map::new();
    for (metric, values) in samples.iter() {
        let mut sorted = values.iter().copied().collect::<Vec<_>>();
        sorted.sort_unstable();
        let percentile = |p: usize| {
            sorted
                .get((sorted.len() * p).div_ceil(100).saturating_sub(1))
                .copied()
        };
        output.insert(
            (*metric).to_string(),
            json!({"samples":values.len(),"p50Us":percentile(50),"p95Us":percentile(95)}),
        );
    }
    Value::Object(output)
}
