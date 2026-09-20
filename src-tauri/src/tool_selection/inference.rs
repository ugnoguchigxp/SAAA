//! Inference boundary: embedding and rerank traits, the local worker manifest, and deterministic
//! doubles used by the hand-checked fixtures. The real worker is in `worker.rs`.

use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EmbedKind {
    Query,
    Passage,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InferenceErrorKind {
    Timeout,
    Unavailable,
    Busy,
    Protocol,
    Integrity,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InferenceError {
    pub kind: InferenceErrorKind,
}

impl InferenceError {
    pub fn new(kind: InferenceErrorKind) -> Self {
        Self { kind }
    }

    pub fn timeout() -> Self {
        Self::new(InferenceErrorKind::Timeout)
    }

    pub fn unavailable() -> Self {
        Self::new(InferenceErrorKind::Unavailable)
    }

    pub fn protocol() -> Self {
        Self::new(InferenceErrorKind::Protocol)
    }

    pub fn integrity() -> Self {
        Self::new(InferenceErrorKind::Integrity)
    }
}

impl std::fmt::Display for InferenceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self.kind {
            InferenceErrorKind::Timeout => "inference-timeout",
            InferenceErrorKind::Unavailable => "inference-unavailable",
            InferenceErrorKind::Busy => "inference-busy",
            InferenceErrorKind::Protocol => "inference-protocol",
            InferenceErrorKind::Integrity => "inference-integrity",
        })
    }
}

impl std::error::Error for InferenceError {}

#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    fn model_hash(&self) -> &str;
    fn dimension(&self) -> usize;
    async fn embed(
        &self,
        kind: EmbedKind,
        texts: &[String],
    ) -> Result<Vec<Vec<f32>>, InferenceError>;
}

#[async_trait]
pub trait RerankProvider: Send + Sync {
    fn model_hash(&self) -> &str;
    async fn rerank(
        &self,
        query: &str,
        documents: &[(String, String)],
    ) -> Result<Vec<(String, f64)>, InferenceError>;
}

/// Model manifest produced by `scripts/tool-selection/prepare_models.py`. A missing or malformed
/// manifest degrades discovery; it is never silently replaced by another model.
#[derive(Clone, Debug)]
pub struct ModelManifest {
    pub embedding_hash: String,
    pub reranker_hash: String,
    pub embedding_dimension: usize,
    pub no_match_threshold: f64,
}

pub fn load_manifest(path: &Path) -> Result<ModelManifest, InferenceError> {
    let raw = std::fs::read(path).map_err(|_| InferenceError::unavailable())?;
    let value: Value = serde_json::from_slice(&raw).map_err(|_| InferenceError::protocol())?;
    let format = value
        .get("formatVersion")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    if format != 1 {
        return Err(InferenceError::protocol());
    }
    let embedding = value
        .get("embedding")
        .ok_or_else(InferenceError::protocol)?;
    let reranker = value.get("reranker").ok_or_else(InferenceError::protocol)?;
    let embedding_hash = embedding
        .get("hash")
        .and_then(Value::as_str)
        .filter(|hash| !hash.is_empty())
        .ok_or_else(InferenceError::protocol)?
        .to_string();
    let reranker_hash = reranker
        .get("hash")
        .and_then(Value::as_str)
        .filter(|hash| !hash.is_empty())
        .ok_or_else(InferenceError::protocol)?
        .to_string();
    let embedding_dimension = embedding
        .get("dimension")
        .and_then(Value::as_u64)
        .filter(|dimension| *dimension > 0)
        .ok_or_else(InferenceError::protocol)? as usize;
    let no_match_threshold = value
        .get("noMatchThreshold")
        .and_then(Value::as_f64)
        .ok_or_else(InferenceError::protocol)?;
    Ok(ModelManifest {
        embedding_hash,
        reranker_hash,
        embedding_dimension,
        no_match_threshold,
    })
}

/// Deterministic embedding double. Vectors are a normalized byte-gram bag so identical text
/// always produces the same vector and shared tokens raise cosine similarity.
pub struct HashEmbedding {
    model_hash: String,
    dimension: usize,
}

impl HashEmbedding {
    pub fn new(dimension: usize) -> Self {
        Self {
            model_hash: format!("test-hash-{dimension}"),
            dimension,
        }
    }

    fn vector(&self, text: &str) -> Vec<f32> {
        let mut vector = vec![0.0_f32; self.dimension];
        let bytes = text.as_bytes();
        if bytes.is_empty() {
            return vector;
        }
        for window in bytes.windows(3.min(bytes.len())) {
            let mut hash = 2166136261_u32;
            for byte in window {
                hash ^= u32::from(*byte);
                hash = hash.wrapping_mul(16777619);
            }
            let slot = (hash as usize) % self.dimension;
            vector[slot] += 1.0;
        }
        let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
        if norm > 0.0 {
            for value in &mut vector {
                *value /= norm;
            }
        }
        vector
    }
}

#[async_trait]
impl EmbeddingProvider for HashEmbedding {
    fn model_hash(&self) -> &str {
        &self.model_hash
    }

    fn dimension(&self) -> usize {
        self.dimension
    }

    async fn embed(
        &self,
        _kind: EmbedKind,
        texts: &[String],
    ) -> Result<Vec<Vec<f32>>, InferenceError> {
        Ok(texts.iter().map(|text| self.vector(text)).collect())
    }
}

/// Fixed reranker double. Missing documents fall back to `default_score`.
pub struct FixedReranker {
    model_hash: String,
    scores: HashMap<String, f64>,
    default_score: f64,
}

impl FixedReranker {
    pub fn new(scores: &[(&str, f64)]) -> Self {
        Self {
            model_hash: "test-fixed-reranker".to_string(),
            scores: scores
                .iter()
                .map(|(id, score)| ((*id).to_string(), *score))
                .collect(),
            default_score: 0.0,
        }
    }
}

#[async_trait]
impl RerankProvider for FixedReranker {
    fn model_hash(&self) -> &str {
        &self.model_hash
    }

    async fn rerank(
        &self,
        _query: &str,
        documents: &[(String, String)],
    ) -> Result<Vec<(String, f64)>, InferenceError> {
        let mut scores = Vec::with_capacity(documents.len());
        for (id, _) in documents {
            let score = self.scores.get(id).copied().unwrap_or(self.default_score);
            if !score.is_finite() {
                return Err(InferenceError::integrity());
            }
            scores.push((id.clone(), score));
        }
        Ok(scores)
    }
}

/// Deterministic mock reranker used by the evaluation CLI's mock lane and by tests. It scores
/// with the same hash embedding so the hybrid pipeline is exercised without a live model.
pub struct HashReranker {
    model_hash: String,
    embedding: HashEmbedding,
}

impl Default for HashReranker {
    fn default() -> Self {
        Self::new()
    }
}

impl HashReranker {
    pub fn new() -> Self {
        Self {
            model_hash: "test-hash-reranker".to_string(),
            embedding: HashEmbedding::new(384),
        }
    }
}

#[async_trait]
impl RerankProvider for HashReranker {
    fn model_hash(&self) -> &str {
        &self.model_hash
    }

    async fn rerank(
        &self,
        query: &str,
        documents: &[(String, String)],
    ) -> Result<Vec<(String, f64)>, InferenceError> {
        let query_vector = self.embedding.vector(query);
        let mut scores = Vec::with_capacity(documents.len());
        for (id, text) in documents {
            let document = self.embedding.vector(text);
            let score = super::ranking::cosine_similarity(&query_vector, &document)
                .ok_or_else(InferenceError::integrity)?;
            scores.push((id.clone(), score));
        }
        Ok(scores)
    }
}

/// A provider that always fails, used to assert degraded paths in tests.
pub struct UnavailableEmbedding;

#[async_trait]
impl EmbeddingProvider for UnavailableEmbedding {
    fn model_hash(&self) -> &str {
        "unavailable"
    }

    fn dimension(&self) -> usize {
        0
    }

    async fn embed(
        &self,
        _kind: EmbedKind,
        _texts: &[String],
    ) -> Result<Vec<Vec<f32>>, InferenceError> {
        Err(InferenceError::unavailable())
    }
}

pub struct UnavailableReranker;

#[async_trait]
impl RerankProvider for UnavailableReranker {
    fn model_hash(&self) -> &str {
        "unavailable"
    }

    async fn rerank(
        &self,
        _query: &str,
        _documents: &[(String, String)],
    ) -> Result<Vec<(String, f64)>, InferenceError> {
        Err(InferenceError::unavailable())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn hash_embedding_is_deterministic_and_normalized() {
        let embedding = HashEmbedding::new(16);
        let first = embedding
            .embed(EmbedKind::Query, &["same text".into()])
            .await
            .unwrap();
        let second = embedding
            .embed(EmbedKind::Query, &["same text".into()])
            .await
            .unwrap();
        assert_eq!(first, second);
        let norm = first[0].iter().map(|value| value * value).sum::<f32>();
        assert!((norm - 1.0).abs() < 1e-5);
    }

    #[tokio::test]
    async fn fixed_reranker_rejects_non_finite_scores() {
        let reranker = FixedReranker::new(&[("rev1", f64::NAN)]);
        let error = reranker
            .rerank("query", &[("rev1".into(), "doc".into())])
            .await
            .unwrap_err();
        assert_eq!(error.kind, InferenceErrorKind::Integrity);
    }
}
