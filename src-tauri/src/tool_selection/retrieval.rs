//! Retrieval helpers that sit between SQL (FTS/vectors), the pure ranking math and the ML worker.
//! The service snapshots eligible revisions under the writer lock, then runs this module without
//! holding the database lock.

use super::contracts::*;
use super::ranking::{fuse, FusedCandidate};
use super::repository;

/// Raw, bounded embedding query. The E5 `query: ` prefix is added by the local worker (the model
/// boundary), so it is applied exactly once; the reranker receives the same raw text.
pub fn query_text(intent: &str) -> String {
    repository::truncate_utf8(intent, SEARCH_INTENT_MAX_BYTES).to_string()
}

/// Raw, bounded document text. The E5 `passage: ` prefix is added by the local worker; the
/// reranker receives the same raw text.
pub fn document_text(search_text: &str) -> String {
    repository::truncate_utf8(search_text, SEARCH_TEXT_MAX_BYTES).to_string()
}

/// Cosine ranking of one query against precomputed revision vectors. Descending score, then
/// revision id ascending. Non-finite or mismatched vectors are dropped.
pub fn embedding_candidates(
    query_vector: &[f32],
    embeddings: &[(String, Vec<f32>)],
) -> Vec<(String, f64)> {
    let mut scored: Vec<(String, f64)> = embeddings
        .iter()
        .filter_map(|(revision_id, vector)| {
            super::ranking::cosine_similarity(query_vector, vector)
                .map(|score| (revision_id.clone(), score))
        })
        .collect();
    scored.sort_by(|left, right| {
        right
            .1
            .partial_cmp(&left.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.0.cmp(&right.0))
    });
    scored
}

/// True when a query is long enough for trigram matching (three Unicode scalar values).
pub fn lexical_eligible(query: &str) -> bool {
    query.chars().count() >= 3
}

/// Builds a safe FTS5 MATCH expression: every token is quoted (no raw MATCH operators) and
/// joined with OR so a long natural-language intent still matches on its distinctive terms.
/// Returns `None` when no token is long enough to produce a trigram.
pub fn fts_match_query(intent: &str) -> Option<String> {
    let mut terms: Vec<String> = Vec::new();
    for token in intent.split(|character: char| !character.is_alphanumeric()) {
        if token.chars().count() >= 3 {
            let escaped = token.replace('"', "\"\"");
            let term = format!("\"{escaped}\"");
            if !terms.contains(&term) {
                terms.push(term);
            }
        }
    }
    if terms.is_empty() {
        None
    } else {
        Some(terms.join(" OR "))
    }
}

pub fn fuse_candidates(lexical: &[String], vector: &[String], top: usize) -> Vec<FusedCandidate> {
    fuse(lexical, vector, top)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_queries_skip_lexical_search() {
        assert!(!lexical_eligible("あい"));
        assert!(lexical_eligible("あいう"));
        assert!(!lexical_eligible("ab"));
        assert!(lexical_eligible("abc"));
    }

    #[test]
    fn embedding_candidates_sort_descending_then_by_id() {
        let query = [1.0_f32, 0.0];
        let embeddings = vec![
            ("b".to_string(), vec![1.0, 0.0]),
            ("a".to_string(), vec![1.0, 0.0]),
            ("c".to_string(), vec![0.0, 1.0]),
        ];
        let scored = embedding_candidates(&query, &embeddings);
        let ids: Vec<&str> = scored.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(ids, vec!["a", "b", "c"]);
    }

    #[test]
    fn query_and_document_text_are_raw_and_bounded() {
        // Prefixes are the worker's responsibility; the Rust side must not add them twice.
        assert_eq!(query_text("x"), "x");
        assert_eq!(document_text("doc"), "doc");
        let long = "a".repeat(SEARCH_TEXT_MAX_BYTES + 100);
        assert_eq!(document_text(&long).len(), SEARCH_TEXT_MAX_BYTES);
    }
}
