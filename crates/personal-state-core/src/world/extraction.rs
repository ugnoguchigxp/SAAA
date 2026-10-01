//! Untrusted natural-language candidates. Source identity, scope, time and authority are host-owned.
use super::model_v2::{self, Epistemic, WorldPayloadV2};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Extraction {
    pub candidates: Vec<Candidate>,
    pub no_change: bool,
    #[serde(default)]
    pub outcomes: Vec<OutcomeObservation>,
    #[serde(default)]
    pub deferred_reason: Option<String>,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Candidate {
    pub kind: String,
    pub payload: Value,
    pub quote: String,
    pub quote_start: usize,
    pub quote_end: usize,
    pub epistemic: EvidenceKind,
    pub replaces: Option<String>,
    #[serde(default)]
    pub additional_quotes: Vec<SourceQuote>,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SourceQuote {
    pub source_id: String,
    pub version: u64,
    pub start: usize,
    pub end: usize,
    pub quote: String,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OutcomeObservation {
    pub prior_assertion_id: String,
    pub metric_entity_id: String,
    pub comparison_id: String,
    pub conditions: Vec<super::model::Condition>,
    pub direction: super::model_v2::EffectDirection,
    pub quote: String,
    pub quote_start: usize,
    pub quote_end: usize,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceKind {
    UserReported,
    Inferred,
}

impl Extraction {
    pub fn parse(raw: &str, source: &str) -> Result<Self, String> {
        Self::parse_with_sources(raw, source, &std::collections::BTreeMap::new())
    }
    pub fn parse_with_sources(
        raw: &str,
        source: &str,
        additional: &std::collections::BTreeMap<(String, u64), String>,
    ) -> Result<Self, String> {
        if raw.len() > 16384 {
            return Err("world-extraction-budget".into());
        }
        let value: Self = serde_json::from_str(raw).map_err(|_| "world-extraction-schema")?;
        if value.candidates.len() + value.outcomes.len()
            > super::validation::MAX_NEW_WORLD_ASSERTIONS
            || value.no_change != (value.candidates.is_empty() && value.outcomes.is_empty())
        {
            return Err("world-extraction-count".into());
        }
        if value
            .deferred_reason
            .as_deref()
            .is_some_and(|reason| reason != "evidence_budget" || !value.no_change)
        {
            return Err("world-extraction-deferral".into());
        }
        for outcome in &value.outcomes {
            if outcome.quote.is_empty()
                || outcome.quote.len() > 2000
                || source.get(outcome.quote_start..outcome.quote_end)
                    != Some(outcome.quote.as_str())
                || outcome.prior_assertion_id.is_empty()
                || outcome.prior_assertion_id.len() > 160
                || outcome.metric_entity_id.is_empty()
                || outcome.comparison_id.is_empty()
                || !outcome.quote.contains(&outcome.comparison_id)
                || outcome
                    .conditions
                    .iter()
                    .any(|condition| !outcome.quote.contains(&condition.value))
            {
                return Err("world-extraction-evidence".into());
            }
            super::model::check_conditions_struct(&outcome.conditions)?;
        }
        for c in &value.candidates {
            let mut cited = std::collections::BTreeSet::new();
            if c.additional_quotes.len() > 3 {
                return Err("world-extraction-evidence-limit".into());
            }
            for quote in &c.additional_quotes {
                let key = (quote.source_id.clone(), quote.version);
                let text = additional.get(&key).ok_or("world-extraction-evidence")?;
                if !cited.insert(key)
                    || quote.quote.is_empty()
                    || quote.quote.len() > 2000
                    || text.get(quote.start..quote.end) != Some(quote.quote.as_str())
                {
                    return Err("world-extraction-evidence".into());
                }
            }
            if c.quote.is_empty()
                || c.quote.len() > 2000
                || source.get(c.quote_start..c.quote_end) != Some(c.quote.as_str())
            {
                return Err("world-extraction-evidence".into());
            }
            if c.replaces
                .as_ref()
                .is_some_and(|v| v.is_empty() || v.len() > 160)
            {
                return Err("world-extraction-replacement".into());
            }
            let payload = WorldPayloadV2::decode(&c.kind, &c.payload)?;
            match &payload {
                WorldPayloadV2::Entity(p) => model_v2::check_entity_v2_struct(p)?,
                WorldPayloadV2::Focus(p) => model_v2::check_focus_v2_struct(p)?,
                WorldPayloadV2::Relation(p) => {
                    if p.comparison_id.as_ref().is_some_and(|id| {
                        !c.quote.contains(id)
                            && !c.additional_quotes.iter().any(|q| q.quote.contains(id))
                    }) {
                        return Err("world-extraction-evidence".into());
                    }
                    // Full relation structure is checked after the host binds its source key.
                    // Only the host assigns evidence, assessments and outcome observations.
                    if !p.evidence_stances.is_empty()
                        || !p.assessment_refs.is_empty()
                        || p.outcome_update.is_some()
                        || p.confidence.is_some()
                        || p.strength.is_some()
                        || !matches!(p.epistemic, Epistemic::Observation | Epistemic::Hypothesis)
                        || (c.epistemic == EvidenceKind::Inferred
                            && p.epistemic != Epistemic::Hypothesis)
                    {
                        return Err("world-extraction-authority".into());
                    }
                }
            }
            if payload.byte_len()? > 2000 {
                return Err("world-extraction-budget".into());
            }
        }
        Ok(value)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn wr_t08_rejects_fake_evidence_authority_and_unknown_fields() {
        let c = json!({"kind":"world_entity","payload":{"type":"entity","schema_version":2,"entity_id":"e","entity_kind":"concept","name":"開発","aliases":[],"objective_assertion_id":null},"quote":"開発","quote_start":0,"quote_end":6,"epistemic":"user_reported","replaces":null});
        let mut v = json!({"candidates":[c],"no_change":false});
        assert!(Extraction::parse(&v.to_string(), "開発").is_ok());
        v["candidates"][0]["quote_end"] = json!(5);
        assert!(Extraction::parse(&v.to_string(), "開発").is_err());
        v["candidates"][0]["quote_end"] = json!(6);
        v["scope"] = json!("project:forged");
        assert!(Extraction::parse(&v.to_string(), "開発").is_err());
    }
}

#[cfg(test)]
mod contextual_quote_tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn world_maintenance_context_quotes_require_exact_version_and_utf8_range() {
        let text = "条件";
        let mut value = json!({"candidates":[{"kind":"world_entity","payload":{"type":"entity","schema_version":2,"entity_id":"e","entity_kind":"concept","name":"対象","aliases":[],"objective_assertion_id":null},"quote":"対象","quote_start":0,"quote_end":6,"epistemic":"user_reported","replaces":null,"additional_quotes":[{"source_id":"prior","version":2,"start":0,"end":6,"quote":text}]}],"no_change":false});
        let sources = std::collections::BTreeMap::from([(("prior".into(), 2), text.into())]);
        assert!(Extraction::parse(&value.to_string(), "対象").is_err());
        assert!(Extraction::parse_with_sources(&value.to_string(), "対象", &sources).is_ok());
        value["candidates"][0]["additional_quotes"][0]["version"] = json!(1);
        assert!(Extraction::parse_with_sources(&value.to_string(), "対象", &sources).is_err());
        value["candidates"][0]["additional_quotes"][0]["version"] = json!(2);
        value["candidates"][0]["additional_quotes"][0]["end"] = json!(5);
        assert!(Extraction::parse_with_sources(&value.to_string(), "対象", &sources).is_err());
    }
}
