use crate::*;
use std::collections::BTreeSet;

/// Canonically measured budgets supplied by a certified adapter, not token estimates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Budget {
    pub certified: bool,
    pub native_tokens: u64,
    pub output_reserve: u64,
    pub safety_margin: u64,
    pub requested_input: u64,
    pub max_bytes: u64,
}
impl Budget {
    pub fn input_limit(&self) -> Result<u64, Error> {
        if !self.certified || self.max_bytes == 0 {
            return Err(Error::RequiredMissing);
        }
        Ok(self
            .native_tokens
            .checked_sub(self.output_reserve)
            .and_then(|n| n.checked_sub(self.safety_margin))
            .ok_or(Error::Limit)?
            .min(self.requested_input))
    }
    pub fn validate_materialized(&self, tokens: u64, bytes: u64, output: u64) -> Result<(), Error> {
        if tokens > self.input_limit()? || bytes > self.max_bytes || output > self.output_reserve {
            return Err(Error::Limit);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewBinding {
    pub allocation: String,
    pub runtime: String,
    pub release: String,
    pub lease_epoch: u64,
    pub request_digest: String,
}
#[derive(Debug, PartialEq, Eq)]
pub struct OneShotView {
    pub id: String,
    pub digest: String,
    pub binding: ViewBinding,
    pub expires_at: i64,
    pub ordered_sources: Vec<SourceKey>,
    pub omitted: Vec<(SourceKey, String)>,
    consumed: bool,
}
impl OneShotView {
    /// Keep the host's canonical order. Missing or unexplained inputs fail closed.
    pub fn new(
        id: String,
        digest: String,
        binding: ViewBinding,
        expires_at: i64,
        ordered_sources: Vec<SourceKey>,
        omitted: Vec<(SourceKey, String)>,
    ) -> Self {
        Self {
            id,
            digest,
            binding,
            expires_at,
            ordered_sources,
            omitted,
            consumed: false,
        }
    }
    pub fn consume(
        &mut self,
        expected: &ViewBinding,
        required: &BTreeSet<SourceKey>,
        selected: &BTreeSet<SourceKey>,
        now: i64,
    ) -> Result<(), Error> {
        if self.consumed
            || self.expires_at <= now
            || &self.binding != expected
            || self.id.is_empty()
            || self.digest.is_empty()
        {
            return Err(Error::InvalidFence);
        }
        let delivered: BTreeSet<_> = self.ordered_sources.iter().cloned().collect();
        let omitted: BTreeSet<_> = self.omitted.iter().map(|(key, _)| key.clone()).collect();
        if selected.is_empty()
            || selected.len() > 512
            || self.ordered_sources.len() != delivered.len()
            || self.omitted.len() != omitted.len()
            || !delivered.is_disjoint(&omitted)
            || delivered.union(&omitted).cloned().collect::<BTreeSet<_>>() != *selected
            || !required.is_subset(&delivered)
            || self.omitted.iter().any(|(_, reason)| reason.is_empty())
        {
            return Err(Error::RequiredMissing);
        }
        // Mark before network dispatch. A timeout never makes this view reusable.
        self.consumed = true;
        Ok(())
    }
}

/// Select active items using measured encoded byte sizes. Required items never drop.
pub fn select_projection<'a>(
    projection: &Projection,
    required: &BTreeSet<String>,
    ordered_optional: &'a [(String, usize)],
    measured_required_bytes: usize,
    budget: usize,
) -> Result<(Vec<&'a str>, Vec<&'a str>), Error> {
    if !required
        .iter()
        .all(|id| projection.items.get(id) == Some(&Status::Active))
    {
        return Err(Error::RequiredMissing);
    }
    if measured_required_bytes > budget {
        return Err(Error::Limit);
    }
    let mut used = measured_required_bytes;
    let mut selected = Vec::new();
    let mut omitted = Vec::new();
    let mut seen = BTreeSet::new();
    for (id, bytes) in ordered_optional {
        if !seen.insert(id) || required.contains(id) {
            return Err(Error::Conflict);
        }
        if projection.items.get(id) == Some(&Status::Active) && *bytes <= budget - used {
            selected.push(id.as_str());
            used += bytes;
        } else {
            omitted.push(id.as_str());
        }
    }
    Ok((selected, omitted))
}
