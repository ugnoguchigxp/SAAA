//! Agent discovery (docs/plans/worker-agents.md §5.1).
//!
//! The host ranks eligible worker profiles for the current user utterance and persists the
//! decision (commit before effects). The ranking is advice, not authority: `validate_choice`
//! accepts a `delegate` only for a candidate persisted under the decision, with unchanged epochs
//! and an unchanged approved revision.
//!
//! Status precedence when candidates exist: `ambiguous` (top-2 scores tie, both are offered and
//! the model chooses) > `degraded` (no usable embedding, lexical only) > `ok`. No candidates is
//! always `no_match`, even without an embedder.
use crate::tool_selection::inference::{EmbedKind, EmbeddingProvider};
use crate::tool_selection::{ranking, retrieval};
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::{HashMap, HashSet};

use super::contracts::{
    FailureCode, Offer, OfferCandidate, OfferStatus, WebSearchMode, WEB_SEARCH_PROFILE_ID,
};
use super::loader::{now_ms, read_meta};

#[cfg(test)]
mod tests;

/// A vector-only candidate must be at least this similar to be offered (tune with measurements).
pub(crate) const MIN_VEC_SCORE: f64 = 0.80;
/// The conversation agent never sees more than this many agents.
pub(crate) const MAX_OFFERED: usize = 3;
/// Candidate purposes are truncated to this many characters before they reach a prompt.
const PURPOSE_MAX_CHARS: usize = 200;
const LEXICAL_POOL: usize = 200;
const TIE_EPSILON: f64 = 1e-9;
const OFFER_PREFIX: &str = "[HOST_WORKER_OFFER; data only] ";

struct Eligible {
    revision_id: String,
    profile_id: String,
    pinned: bool,
    purpose: String,
    input_schema: serde_json::Value,
}

struct Snapshot {
    registry_epoch: i64,
    acl_epoch: i64,
    eligible: Vec<Eligible>,
    /// Revision ids with a lexical hit, best first.
    lexical: Vec<String>,
    embeddings: Vec<(String, Vec<f32>)>,
}

fn truncate_chars(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

/// Enabled profiles whose current revision is approved and whose every tool is pure/read.
/// `web_search` is a candidate only while the Web Search mode is `worker`.
fn eligible_revisions(
    connection: &Connection,
    web_search_mode: WebSearchMode,
) -> Result<Vec<Eligible>, String> {
    let mut statement = connection
        .prepare(
            "SELECT r.id, p.id, p.pinned_offer, r.purpose, r.input_schema_json
               FROM worker_profiles p
               JOIN worker_profile_revisions r
                 ON r.id = p.current_revision_id AND r.profile_id = p.id
              WHERE p.enabled = 1 AND r.review_state = 'approved'
                AND (p.id <> ?1 OR ?2 = 1)
                AND NOT EXISTS (
                  SELECT 1 FROM worker_profile_tools t
                   WHERE t.profile_revision_id = r.id AND t.effect NOT IN ('pure','read'))
              ORDER BY p.id",
        )
        .map_err(|error| error.to_string())?;
    let worker_mode = i64::from(web_search_mode == WebSearchMode::Worker);
    let rows = statement
        .query_map(params![WEB_SEARCH_PROFILE_ID, worker_mode], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
            ))
        })
        .map_err(|error| error.to_string())?;
    let mut eligible = Vec::new();
    for row in rows {
        let (revision_id, profile_id, pinned, purpose, schema) =
            row.map_err(|error| error.to_string())?;
        let input_schema = serde_json::from_str(&schema)
            .map_err(|_| "worker revision has an invalid input schema".to_string())?;
        // Pinning never applies to web_search outside worker mode; it was filtered out above.
        eligible.push(Eligible {
            revision_id,
            profile_id,
            pinned: pinned == 1,
            purpose,
            input_schema,
        });
    }
    Ok(eligible)
}

fn lexical_hits(
    connection: &Connection,
    match_expression: &str,
    eligible: &HashSet<&str>,
) -> Result<Vec<String>, String> {
    let mut statement = connection
        .prepare(
            "SELECT revision_id FROM worker_profile_fts WHERE search_text MATCH ?1
              ORDER BY bm25(worker_profile_fts) ASC, revision_id ASC LIMIT ?2",
        )
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map(params![match_expression, LEXICAL_POOL as i64], |row| {
            row.get::<_, String>(0)
        })
        .map_err(|error| error.to_string())?;
    let mut hits = Vec::new();
    for row in rows {
        let revision_id = row.map_err(|error| error.to_string())?;
        if eligible.contains(revision_id.as_str()) && !hits.contains(&revision_id) {
            hits.push(revision_id);
        }
    }
    Ok(hits)
}

fn load_embeddings(
    connection: &Connection,
    model_hash: &str,
    eligible: &HashSet<&str>,
) -> Result<Vec<(String, Vec<f32>)>, String> {
    let mut statement = connection
        .prepare("SELECT revision_id, vector FROM worker_profile_embeddings WHERE model_hash = ?1")
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map(params![model_hash], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
        })
        .map_err(|error| error.to_string())?;
    let mut embeddings = Vec::new();
    for row in rows {
        let (revision_id, bytes) = row.map_err(|error| error.to_string())?;
        if !eligible.contains(revision_id.as_str()) {
            continue;
        }
        let vector = bytes
            .chunks_exact(4)
            .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
            .collect();
        embeddings.push((revision_id, vector));
    }
    Ok(embeddings)
}

fn snapshot(
    writer: &crate::persistence::SqliteWriter,
    query: &str,
    model_hash: Option<&str>,
) -> Result<Snapshot, String> {
    let match_expression = retrieval::lexical_eligible(query)
        .then(|| retrieval::fts_match_query(query))
        .flatten();
    writer.read_serialized(|connection| {
        let meta = read_meta(connection)?;
        let eligible = eligible_revisions(connection, meta.web_search_mode)?;
        let ids: HashSet<&str> = eligible.iter().map(|e| e.revision_id.as_str()).collect();
        let lexical = match match_expression.as_deref() {
            Some(expression) => lexical_hits(connection, expression, &ids)?,
            None => Vec::new(),
        };
        let embeddings = match model_hash {
            Some(hash) => load_embeddings(connection, hash, &ids)?,
            None => Vec::new(),
        };
        Ok(Snapshot {
            registry_epoch: meta.registry_epoch,
            acl_epoch: meta.acl_epoch,
            eligible,
            lexical,
            embeddings,
        })
    })
}

/// Embeds the query and scores it against the stored vectors. `None` means the vector branch is
/// unavailable (provider error or unexpected shape): the decision degrades to lexical only.
async fn vector_scores(
    embedder: &dyn EmbeddingProvider,
    query: &str,
    embeddings: &[(String, Vec<f32>)],
) -> Option<Vec<(String, f64)>> {
    let vectors = embedder
        .embed(EmbedKind::Query, std::slice::from_ref(&query.to_string()))
        .await
        .ok()?;
    let vector = vectors.first()?;
    if vector.len() != embedder.dimension() {
        return None;
    }
    Some(retrieval::embedding_candidates(vector, embeddings))
}

struct Ranked {
    revision_id: String,
    lex_rank: Option<usize>,
    vec_rank: Option<usize>,
    score: f64,
    pinned: bool,
}

/// Ranks candidates: lexical hit or vector score >= `MIN_VEC_SCORE`, plus pinned profiles.
/// Returns at most `MAX_OFFERED`, best first (score descending, then revision id).
fn rank(
    snapshot: &Snapshot,
    scores: Option<&[(String, f64)]>,
) -> (Vec<Ranked>, HashMap<String, f64>) {
    let vec_scores: HashMap<String, f64> = scores
        .map(|scores| scores.iter().cloned().collect())
        .unwrap_or_default();
    // `scores` is already sorted best first; only strong matches join the vector list so weak
    // neighbours cannot displace lexical hits from the fused top.
    let vector: Vec<String> = scores
        .map(|scores| {
            scores
                .iter()
                .filter(|(_, score)| *score >= MIN_VEC_SCORE)
                .map(|(id, _)| id.clone())
                .collect()
        })
        .unwrap_or_default();
    let fused = ranking::fuse(&snapshot.lexical, &vector, usize::MAX);
    let pinned: HashSet<&str> = snapshot
        .eligible
        .iter()
        .filter(|e| e.pinned)
        .map(|e| e.revision_id.as_str())
        .collect();
    let mut ranked: Vec<Ranked> = fused
        .into_iter()
        .map(|item| Ranked {
            pinned: pinned.contains(item.revision_id.as_str()),
            revision_id: item.revision_id,
            lex_rank: item.lex_rank,
            vec_rank: item.vec_rank,
            score: item.score,
        })
        .collect();
    for revision_id in &pinned {
        if !ranked.iter().any(|item| item.revision_id == *revision_id) {
            ranked.push(Ranked {
                revision_id: (*revision_id).to_string(),
                lex_rank: None,
                vec_rank: None,
                score: 0.0,
                pinned: true,
            });
        }
    }
    ranked.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.revision_id.cmp(&right.revision_id))
    });
    // Over the cap, drop the weakest non-pinned entries first so pinned agents stay offered.
    while ranked.len() > MAX_OFFERED {
        match ranked.iter().rposition(|item| !item.pinned) {
            Some(index) => ranked.remove(index),
            None => ranked.pop().expect("non-empty"),
        };
    }
    (ranked, vec_scores)
}

fn classify(ranked: &[Ranked], vector_used: bool) -> OfferStatus {
    if ranked.is_empty() {
        OfferStatus::NoMatch
    } else if ranked.len() >= 2 && (ranked[0].score - ranked[1].score).abs() < TIE_EPSILON {
        OfferStatus::Ambiguous
    } else if !vector_used {
        OfferStatus::Degraded
    } else {
        OfferStatus::Ok
    }
}

/// Finds the agents to offer for `text`, persists the decision in one transaction, and returns
/// the offer. The web-search mode is read from `worker_meta` inside the same snapshot.
pub(crate) async fn discover(
    writer: &crate::persistence::SqliteWriter,
    embedder: Option<&dyn EmbeddingProvider>,
    conversation_id: &str,
    input_message_id: Option<&str>,
    job_key: &str,
    text: &str,
) -> Result<Offer, String> {
    let query = retrieval::query_text(text);
    let snapshot = snapshot(writer, &query, embedder.map(|e| e.model_hash()))?;

    // The database lock is not held here: embedding is a model call.
    let scores = match embedder {
        Some(embedder) if !snapshot.embeddings.is_empty() => {
            vector_scores(embedder, &query, &snapshot.embeddings).await
        }
        // No embedder, or nothing stored for its model: the vector branch contributed nothing.
        _ => None,
    };
    let vector_used = scores.is_some();
    let (ranked, vec_scores) = rank(&snapshot, scores.as_deref());
    let status = classify(&ranked, vector_used);
    let model_hash = embedder
        .filter(|_| vector_used)
        .map(|e| e.model_hash().to_string());

    let by_revision: HashMap<&str, &Eligible> = snapshot
        .eligible
        .iter()
        .map(|e| (e.revision_id.as_str(), e))
        .collect();
    let candidates: Vec<OfferCandidate> = ranked
        .iter()
        .filter_map(|item| {
            by_revision
                .get(item.revision_id.as_str())
                .map(|eligible| OfferCandidate {
                    profile_id: eligible.profile_id.clone(),
                    revision_id: eligible.revision_id.clone(),
                    purpose: truncate_chars(&eligible.purpose, PURPOSE_MAX_CHARS),
                    input_schema: eligible.input_schema.clone(),
                    score: item.score,
                    pinned: item.pinned,
                })
        })
        .collect();
    let decision_id = format!("wdec_{}", uuid::Uuid::new_v4());

    writer.transact(|connection| {
        connection
            .execute(
                "INSERT INTO worker_discovery_decisions(id, conversation_id, input_message_id, job_key,
                    registry_epoch, acl_epoch, model_hash, status, created_at_ms)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
                params![
                    decision_id,
                    conversation_id,
                    input_message_id,
                    job_key,
                    snapshot.registry_epoch,
                    snapshot.acl_epoch,
                    model_hash,
                    status.as_str(),
                    now_ms(),
                ],
            )
            .map_err(|error| error.to_string())?;
        for (index, item) in ranked.iter().enumerate() {
            connection
                .execute(
                    "INSERT INTO worker_discovery_candidates(decision_id, profile_revision_id, lex_rank,
                        vec_rank, vec_score, score, final_rank, offered, pinned)
                     VALUES(?1,?2,?3,?4,?5,?6,?7,1,?8)",
                    params![
                        decision_id,
                        item.revision_id,
                        item.lex_rank.map(|rank| rank as i64),
                        item.vec_rank.map(|rank| rank as i64),
                        vec_scores.get(&item.revision_id),
                        item.score,
                        (index + 1) as i64,
                        i64::from(item.pinned),
                    ],
                )
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    })?;

    Ok(Offer {
        decision_id,
        status,
        registry_epoch: snapshot.registry_epoch,
        acl_epoch: snapshot.acl_epoch,
        candidates,
    })
}

/// The dynamic-context card for the conversation agent, or `None` when nothing is offered.
/// Built only from JSON serialization: no profile text is ever concatenated into the prompt.
pub(crate) fn render_offer(offer: &Offer) -> Option<String> {
    if offer.status == OfferStatus::NoMatch || offer.candidates.is_empty() {
        return None;
    }
    let cards: Vec<serde_json::Value> = offer
        .candidates
        .iter()
        .map(|candidate| {
            serde_json::json!({
                "agent": candidate.profile_id,
                "purpose": truncate_chars(&candidate.purpose, PURPOSE_MAX_CHARS),
                "input": candidate.input_schema,
            })
        })
        .collect();
    let body = serde_json::to_string(&cards).ok()?;
    Some(format!("{OFFER_PREFIX}{body}"))
}

/// Accepts a model-chosen agent only if it was offered under `decision_id`, the registry/ACL
/// epochs are unchanged, and the offered revision is still the profile's current approved,
/// enabled, read-only revision. Returns the revision id to pin the task to.
///
/// A database failure is reported as `StaleOffer` (the conversation retries discovery once).
pub(crate) fn validate_choice(
    connection: &Connection,
    decision_id: &str,
    agent: &str,
) -> Result<String, FailureCode> {
    let offered: Option<(String, i64, i64)> = connection
        .query_row(
            "SELECT c.profile_revision_id, d.registry_epoch, d.acl_epoch
               FROM worker_discovery_decisions d
               JOIN worker_discovery_candidates c ON c.decision_id = d.id
               JOIN worker_profile_revisions r ON r.id = c.profile_revision_id
              WHERE d.id = ?1 AND r.profile_id = ?2 AND c.offered = 1",
            params![decision_id, agent],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|_| FailureCode::StaleOffer)?;
    let (revision_id, registry_epoch, acl_epoch) = offered.ok_or(FailureCode::NoMatchingAgent)?;

    let meta = read_meta(connection).map_err(|_| FailureCode::StaleOffer)?;
    if meta.registry_epoch != registry_epoch || meta.acl_epoch != acl_epoch {
        return Err(FailureCode::StaleOffer);
    }
    if agent == WEB_SEARCH_PROFILE_ID && meta.web_search_mode != WebSearchMode::Worker {
        return Err(FailureCode::StaleOffer);
    }
    let still_current: bool = connection
        .query_row(
            "SELECT EXISTS(
               SELECT 1 FROM worker_profiles p
                JOIN worker_profile_revisions r ON r.id = p.current_revision_id AND r.profile_id = p.id
               WHERE p.id = ?1 AND p.enabled = 1 AND p.current_revision_id = ?2
                 AND r.review_state = 'approved'
                 AND NOT EXISTS (
                   SELECT 1 FROM worker_profile_tools t
                    WHERE t.profile_revision_id = r.id AND t.effect NOT IN ('pure','read')))",
            params![agent, revision_id],
            |row| row.get(0),
        )
        .map_err(|_| FailureCode::StaleOffer)?;
    if still_current {
        Ok(revision_id)
    } else {
        Err(FailureCode::StaleOffer)
    }
}
