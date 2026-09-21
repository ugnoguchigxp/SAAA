//! Untrusted natural-language candidates. Source identity, scope, time and authority are host-owned.
use super::model_v2::{self, Epistemic, WorldPayloadV2};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Extraction {
    pub candidates: Vec<Candidate>,
    pub no_change: bool,
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
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceKind {
    UserReported,
    Inferred,
}

impl Extraction {
    pub fn parse(raw: &str, source: &str) -> Result<Self, String> {
        if raw.len() > 16384 {
            return Err("world-extraction-budget".into());
        }
        let value: Self = serde_json::from_str(raw).map_err(|_| "world-extraction-schema")?;
        if value.candidates.len() > super::validation::MAX_NEW_WORLD_ASSERTIONS || value.no_change != value.candidates.is_empty() {
            return Err("world-extraction-count".into());
        }
        for c in &value.candidates {
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
