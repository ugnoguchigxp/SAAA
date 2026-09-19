//! Local ML worker process and JSONL framing (protocol version 1).
//!
//! One fixed worker is started lazily and reused; requests never load a model. The framing
//! parser is pure so malformed output, NaN vectors, missing/duplicate IDs and dimension drift can
//! be tested without a live model.

use serde_json::{json, Value};
use std::path::PathBuf;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout};

use super::contracts::*;
use super::inference::{EmbedKind, InferenceError, InferenceErrorKind, ModelManifest};

pub fn embed_request(id: &str, kind: EmbedKind, texts: &[String]) -> String {
    let kind = match kind {
        EmbedKind::Query => "query",
        EmbedKind::Passage => "passage",
    };
    json!({
        "version": 1,
        "id": id,
        "op": "embed",
        "kind": kind,
        "texts": texts,
    })
    .to_string()
}

pub fn rerank_request(id: &str, query: &str, documents: &[(String, String)]) -> String {
    let documents: Vec<Value> = documents
        .iter()
        .map(|(document_id, text)| json!({ "id": document_id, "text": text }))
        .collect();
    json!({
        "version": 1,
        "id": id,
        "op": "rerank",
        "query": query,
        "documents": documents,
    })
    .to_string()
}

/// Parses an embed response. Every value is checked: protocol version, matching id, `ok`,
/// model hash, count, dimension, finite floats.
pub fn parse_embed_response(
    line: &str,
    expected_id: &str,
    expected_model_hash: &str,
    expected_dimension: usize,
    expected_count: usize,
) -> Result<Vec<Vec<f32>>, InferenceError> {
    let value: Value = serde_json::from_str(line).map_err(|_| InferenceError::protocol())?;
    if value.get("version").and_then(Value::as_u64) != Some(1) {
        return Err(InferenceError::protocol());
    }
    if value.get("id").and_then(Value::as_str) != Some(expected_id) {
        return Err(InferenceError::protocol());
    }
    if value.get("ok").and_then(Value::as_bool) != Some(true) {
        return Err(InferenceError::protocol());
    }
    if value.get("modelHash").and_then(Value::as_str) != Some(expected_model_hash) {
        return Err(InferenceError::integrity());
    }
    let vectors = value
        .get("vectors")
        .and_then(Value::as_array)
        .ok_or_else(InferenceError::protocol)?;
    if vectors.len() != expected_count {
        return Err(InferenceError::protocol());
    }
    let mut parsed = Vec::with_capacity(vectors.len());
    for vector in vectors {
        let values = vector.as_array().ok_or_else(InferenceError::protocol)?;
        if values.len() != expected_dimension {
            return Err(InferenceError::integrity());
        }
        let mut floats = Vec::with_capacity(values.len());
        for item in values {
            let number = item.as_f64().ok_or_else(InferenceError::protocol)?;
            if !number.is_finite() {
                return Err(InferenceError::integrity());
            }
            floats.push(number as f32);
        }
        parsed.push(floats);
    }
    Ok(parsed)
}

/// Parses a rerank response and requires the scores to line up one-to-one with the request order.
pub fn parse_rerank_response(
    line: &str,
    expected_id: &str,
    expected_model_hash: &str,
    expected_ids: &[String],
) -> Result<Vec<(String, f64)>, InferenceError> {
    let value: Value = serde_json::from_str(line).map_err(|_| InferenceError::protocol())?;
    if value.get("version").and_then(Value::as_u64) != Some(1) {
        return Err(InferenceError::protocol());
    }
    if value.get("id").and_then(Value::as_str) != Some(expected_id) {
        return Err(InferenceError::protocol());
    }
    if value.get("ok").and_then(Value::as_bool) != Some(true) {
        return Err(InferenceError::protocol());
    }
    if value.get("modelHash").and_then(Value::as_str) != Some(expected_model_hash) {
        return Err(InferenceError::integrity());
    }
    let scores = value
        .get("scores")
        .and_then(Value::as_array)
        .ok_or_else(InferenceError::protocol)?;
    if scores.len() != expected_ids.len() {
        return Err(InferenceError::protocol());
    }
    let mut parsed = Vec::with_capacity(scores.len());
    for (index, item) in scores.iter().enumerate() {
        let id = item
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(InferenceError::protocol)?;
        if id != expected_ids[index] {
            return Err(InferenceError::protocol());
        }
        let number = item
            .get("value")
            .and_then(Value::as_f64)
            .ok_or_else(InferenceError::protocol)?;
        if !number.is_finite() {
            return Err(InferenceError::integrity());
        }
        parsed.push((id.to_string(), number));
    }
    Ok(parsed)
}

struct WorkerChild {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

/// Fixed local worker. Requests are serialized; a timeout or protocol failure kills the process so
/// a late answer cannot leak into the next request.
pub struct MlWorker {
    python_path: PathBuf,
    script_path: PathBuf,
    manifest_path: PathBuf,
    model: ModelManifest,
    child: tokio::sync::Mutex<Option<WorkerChild>>,
    pending: tokio::sync::Semaphore,
}

impl MlWorker {
    pub fn new(
        python_path: PathBuf,
        script_path: PathBuf,
        manifest_path: PathBuf,
        model: ModelManifest,
    ) -> Self {
        Self {
            python_path,
            script_path,
            manifest_path,
            model,
            child: tokio::sync::Mutex::new(None),
            pending: tokio::sync::Semaphore::new(WORKER_PENDING_MAX),
        }
    }

    pub fn model(&self) -> &ModelManifest {
        &self.model
    }

    async fn spawn(&self) -> Result<WorkerChild, InferenceError> {
        let mut child = tokio::process::Command::new(&self.python_path)
            .arg(&self.script_path)
            .arg("--manifest")
            .arg(&self.manifest_path)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|_| InferenceError::unavailable())?;
        let stdin = child.stdin.take().ok_or_else(InferenceError::unavailable)?;
        let stdout = child.stdout.take().ok_or_else(InferenceError::unavailable)?;
        if let Some(stderr) = child.stderr.take() {
            // Bounded drain; the contents are never returned to the model.
            tokio::spawn(async move {
                let mut reader = BufReader::new(stderr);
                let mut buffer = Vec::new();
                let mut line = String::new();
                while reader.read_line(&mut line).await.unwrap_or(0) > 0 {
                    if buffer.len() < WORKER_STDERR_MAX_BYTES {
                        buffer.extend_from_slice(line.as_bytes());
                    }
                    line.clear();
                }
            });
        }
        Ok(WorkerChild {
            child,
            stdin,
            stdout: BufReader::new(stdout),
        })
    }

    async fn kill(&self, mut worker: WorkerChild) {
        let _ = worker.child.start_kill();
        let _ = worker.child.wait().await;
    }

    /// Sends one request line and reads one response line. `timeout` covers the whole exchange.
    pub async fn call(&self, request: String, timeout: Duration) -> Result<String, InferenceError> {
        if request.len() > WORKER_LINE_MAX_BYTES {
            return Err(InferenceError::protocol());
        }
        let permit = self
            .pending
            .try_acquire()
            .map_err(|_| InferenceError::new(InferenceErrorKind::Busy))?;
        let mut guard = self.child.lock().await;
        if guard.is_none() {
            *guard = Some(self.spawn().await?);
        }
        let result = tokio::time::timeout(timeout, async {
            let worker = guard.as_mut().ok_or_else(InferenceError::unavailable)?;
            worker
                .stdin
                .write_all(request.as_bytes())
                .await
                .map_err(|_| InferenceError::unavailable())?;
            worker
                .stdin
                .write_all(b"\n")
                .await
                .map_err(|_| InferenceError::unavailable())?;
            worker
                .stdin
                .flush()
                .await
                .map_err(|_| InferenceError::unavailable())?;
            let mut response = String::new();
            let read = worker
                .stdout
                .read_line(&mut response)
                .await
                .map_err(|_| InferenceError::unavailable())?;
            if read == 0 {
                return Err(InferenceError::unavailable());
            }
            if response.len() > WORKER_LINE_MAX_BYTES {
                return Err(InferenceError::protocol());
            }
            Ok(response)
        })
        .await;
        match result {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(error)) => {
                if let Some(worker) = guard.take() {
                    self.kill(worker).await;
                }
                Err(error)
            }
            Err(_) => {
                if let Some(worker) = guard.take() {
                    self.kill(worker).await;
                }
                Err(InferenceError::timeout())
            }
        }
        .map(|response| {
            drop(permit);
            response
        })
    }

    pub async fn shutdown(&self) {
        let mut guard = self.child.lock().await;
        if let Some(mut worker) = guard.take() {
            let _ = worker.child.start_kill();
            let _ = worker.child.wait().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn embed_line(id: &str, hash: &str, vectors: Value) -> String {
        json!({
            "version": 1,
            "id": id,
            "ok": true,
            "vectors": vectors,
            "dimension": 2,
            "modelHash": hash,
        })
        .to_string()
    }

    #[test]
    fn valid_embed_response_parses() {
        let line = embed_line("req", "hash", json!([[0.1, 0.2]]));
        let parsed = parse_embed_response(&line, "req", "hash", 2, 1).expect("parse");
        assert_eq!(parsed, vec![vec![0.1_f32, 0.2_f32]]);
    }

    #[test]
    fn nan_and_infinity_are_rejected() {
        for value in ["NaN", "Infinity", "-Infinity"] {
            let line = format!(
                "{{\"version\":1,\"id\":\"req\",\"ok\":true,\"vectors\":[[{value},0.2]],\"modelHash\":\"hash\"}}"
            );
            assert!(parse_embed_response(&line, "req", "hash", 2, 1).is_err());
        }
    }

    #[test]
    fn wrong_id_hash_dimension_or_count_is_rejected() {
        assert!(parse_embed_response(&embed_line("other", "hash", json!([[0.1, 0.2]])), "req", "hash", 2, 1).is_err());
        assert!(parse_embed_response(&embed_line("req", "other", json!([[0.1, 0.2]])), "req", "hash", 2, 1).is_err());
        assert!(parse_embed_response(&embed_line("req", "hash", json!([[0.1]])), "req", "hash", 2, 1).is_err());
        assert!(parse_embed_response(&embed_line("req", "hash", json!([])), "req", "hash", 2, 1).is_err());
    }

    #[test]
    fn rerank_requires_exact_id_order() {
        let line = json!({
            "version": 1,
            "id": "req",
            "ok": true,
            "scores": [{"id": "a", "value": 1.0}, {"id": "b", "value": 0.5}],
            "modelHash": "hash",
        })
        .to_string();
        let expected = vec!["a".to_string(), "b".to_string()];
        let parsed = parse_rerank_response(&line, "req", "hash", &expected).expect("parse");
        assert_eq!(parsed.len(), 2);
        let missing = vec!["b".to_string(), "a".to_string()];
        assert!(parse_rerank_response(&line, "req", "hash", &missing).is_err());
    }
}
