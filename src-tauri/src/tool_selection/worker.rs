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
use super::inference::{
    EmbedKind, EmbeddingProvider, InferenceError, InferenceErrorKind, ModelManifest, RerankProvider,
};

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
    embedding_loaded: std::sync::atomic::AtomicBool,
    reranker_loaded: std::sync::atomic::AtomicBool,
    spawn_failures: std::sync::atomic::AtomicU32,
    disabled: std::sync::atomic::AtomicBool,
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
            embedding_loaded: std::sync::atomic::AtomicBool::new(false),
            reranker_loaded: std::sync::atomic::AtomicBool::new(false),
            spawn_failures: std::sync::atomic::AtomicU32::new(0),
            disabled: std::sync::atomic::AtomicBool::new(false),
        }
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
        let stdout = child
            .stdout
            .take()
            .ok_or_else(InferenceError::unavailable)?;
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

    /// A restarted process reloads its models; each model gets the longer load deadline once more.
    fn reset_loaded(&self) {
        self.embedding_loaded
            .store(false, std::sync::atomic::Ordering::SeqCst);
        self.reranker_loaded
            .store(false, std::sync::atomic::Ordering::SeqCst);
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
        if self.disabled.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(InferenceError::unavailable());
        }
        let permit = self
            .pending
            .try_acquire()
            .map_err(|_| InferenceError::new(InferenceErrorKind::Busy))?;
        let mut guard = self.child.lock().await;
        if guard.is_none() {
            match self.spawn().await {
                Ok(worker) => {
                    self.spawn_failures
                        .store(0, std::sync::atomic::Ordering::SeqCst);
                    *guard = Some(worker);
                }
                Err(error) => {
                    let failures = self
                        .spawn_failures
                        .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                        + 1;
                    if failures >= WORKER_SPAWN_FAILURE_LIMIT {
                        self.disabled
                            .store(true, std::sync::atomic::Ordering::SeqCst);
                    }
                    return Err(error);
                }
            }
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
        let outcome = match result {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(error)) => {
                if let Some(worker) = guard.take() {
                    self.reset_loaded();
                    self.kill(worker).await;
                }
                Err(error)
            }
            Err(_) => {
                if let Some(worker) = guard.take() {
                    self.reset_loaded();
                    self.kill(worker).await;
                }
                Err(InferenceError::timeout())
            }
        };
        drop(permit);
        outcome
    }

    pub async fn shutdown(&self) {
        let mut guard = self.child.lock().await;
        if let Some(mut worker) = guard.take() {
            let _ = worker.child.start_kill();
            let _ = worker.child.wait().await;
        }
    }
}

#[async_trait::async_trait]
impl EmbeddingProvider for MlWorker {
    fn model_hash(&self) -> &str {
        &self.model.embedding_hash
    }

    fn dimension(&self) -> usize {
        self.model.embedding_dimension
    }

    async fn embed(
        &self,
        kind: EmbedKind,
        texts: &[String],
    ) -> Result<Vec<Vec<f32>>, InferenceError> {
        let id = uuid::Uuid::new_v4().to_string();
        let request = embed_request(&id, kind, texts);
        let timeout = self.call_timeout(true);
        let line = self.call(request, timeout).await?;
        let vectors = parse_embed_response(
            &line,
            &id,
            &self.model.embedding_hash,
            self.model.embedding_dimension,
            texts.len(),
        )?;
        self.embedding_loaded
            .store(true, std::sync::atomic::Ordering::SeqCst);
        Ok(vectors)
    }
}

#[async_trait::async_trait]
impl RerankProvider for MlWorker {
    fn model_hash(&self) -> &str {
        &self.model.reranker_hash
    }

    async fn rerank(
        &self,
        query: &str,
        documents: &[(String, String)],
    ) -> Result<Vec<(String, f64)>, InferenceError> {
        let id = uuid::Uuid::new_v4().to_string();
        let request = rerank_request(&id, query, documents);
        let expected: Vec<String> = documents.iter().map(|(id, _)| id.clone()).collect();
        let timeout = self.call_timeout(false);
        let line = self.call(request, timeout).await?;
        let scores = parse_rerank_response(&line, &id, &self.model.reranker_hash, &expected)?;
        self.reranker_loaded
            .store(true, std::sync::atomic::Ordering::SeqCst);
        Ok(scores)
    }
}

impl MlWorker {
    /// Loading the embedding and reranker models are independent first calls; each gets the
    /// longer load deadline exactly once.
    fn call_timeout(&self, embedding: bool) -> Duration {
        let loaded = if embedding {
            self.embedding_loaded
                .load(std::sync::atomic::Ordering::SeqCst)
        } else {
            self.reranker_loaded
                .load(std::sync::atomic::Ordering::SeqCst)
        };
        if loaded {
            WORKER_REQUEST_TIMEOUT
        } else {
            WORKER_LOAD_TIMEOUT
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

    #[tokio::test]
    async fn timeout_kills_the_worker_and_the_next_request_is_isolated() {
        use std::io::Write;
        let directory = tempfile::tempdir().expect("tempdir");
        let script = directory.path().join("fake_worker.py");
        let manifest_path = directory.path().join("manifest.json");
        std::fs::write(&manifest_path, "{}").expect("manifest");
        let mut file = std::fs::File::create(&script).expect("script");
        writeln!(
            file,
            "import sys, json, time\n\
             for line in sys.stdin:\n\
             \x20   req = json.loads(line)\n\
             \x20   if req['id'] == 'first':\n\
             \x20       time.sleep(30)\n\
             \x20   print(json.dumps({{'version':1,'id':req['id'],'ok':True,'vectors':[[0.1,0.2]],'dimension':2,'modelHash':'test'}}), flush=True)"
        )
        .expect("write script");
        let python = std::env::var("SAAA_TEST_PYTHON").unwrap_or_else(|_| "python3".to_string());
        let worker = MlWorker::new(
            std::path::PathBuf::from(python),
            script,
            manifest_path,
            ModelManifest {
                embedding_hash: "test".to_string(),
                reranker_hash: "test".to_string(),
                embedding_dimension: 2,
                no_match_threshold: 0.0,
            },
        );
        let first = worker
            .call(
                embed_request("first", EmbedKind::Query, &["a".to_string()]),
                Duration::from_millis(300),
            )
            .await;
        assert_eq!(first.unwrap_err().kind, InferenceErrorKind::Timeout);
        let second = worker
            .call(
                embed_request("second", EmbedKind::Query, &["b".to_string()]),
                Duration::from_secs(5),
            )
            .await
            .expect("restarted worker answers");
        let parsed = parse_embed_response(&second, "second", "test", 2, 1).expect("parsed");
        assert_eq!(parsed, vec![vec![0.1_f32, 0.2_f32]]);
        worker.shutdown().await;
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
        assert!(parse_embed_response(
            &embed_line("other", "hash", json!([[0.1, 0.2]])),
            "req",
            "hash",
            2,
            1
        )
        .is_err());
        assert!(parse_embed_response(
            &embed_line("req", "other", json!([[0.1, 0.2]])),
            "req",
            "hash",
            2,
            1
        )
        .is_err());
        assert!(parse_embed_response(
            &embed_line("req", "hash", json!([[0.1]])),
            "req",
            "hash",
            2,
            1
        )
        .is_err());
        assert!(
            parse_embed_response(&embed_line("req", "hash", json!([])), "req", "hash", 2, 1)
                .is_err()
        );
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
