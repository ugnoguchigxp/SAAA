use crate::*;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

/// SHA-256 of the canonical patch JSON. Used for re-send detection before any
/// World re-validation (C5).
pub fn patch_fingerprint(patch: &StatePatch) -> Result<String, Error> {
    let encoded = serde_json::to_vec(patch).map_err(|_| Error::InvalidPatch)?;
    Ok(patch_fingerprint_bytes(&encoded))
}

pub fn patch_fingerprint_bytes(encoded: &[u8]) -> String {
    format!("{:x}", Sha256::digest(encoded))
}

/// World semantic-key version prefix (`wm1`, `wm2`). Used to allow a v1 to v2
/// replacement while still rejecting same-version key mismatches.
fn world_key_version(key: &str) -> &str {
    key.split(':').next().unwrap_or("")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Projection {
    pub revision: u64,
    pub input_epoch: u64,
    pub items: BTreeMap<String, Status>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForgetEffect {
    pub assertions: BTreeSet<String>,
    pub payload_refs: BTreeSet<String>,
}

impl Ledger {
    /// A replacement is a new version. Same version with different facts conflicts.
    pub fn record_source(&mut self, source: SourceRef) -> Result<bool, Error> {
        if source.access.principal != self.principal || source.access.scope != self.scope {
            return Err(Error::Unauthorized);
        }
        if self.tombstones.contains(&source.key.id)
            || !source.available
            || source.key.id.is_empty()
            || source.key.version == 0
            || source.key.end <= source.key.start
            || source.digest.len() != 64
            || !source.digest.bytes().all(|b| b.is_ascii_hexdigit())
            || source.access.policy_revision != self.policy_revision
        {
            return Err(Error::InvalidSource);
        }
        if let Some(existing) = self.sources.get(&source.key) {
            return if existing == &source {
                Ok(false)
            } else {
                Err(Error::Conflict)
            };
        }
        // Old imports must not restore a version superseded by a current edit.
        if self
            .sources
            .keys()
            .any(|k| k.id == source.key.id && k.version > source.key.version)
        {
            return Err(Error::InvalidSource);
        }
        self.input_epoch = self.input_epoch.checked_add(1).ok_or(Error::Limit)?;
        for prior in self.sources.values_mut() {
            if prior.key.id == source.key.id && prior.key.version < source.key.version {
                prior.available = false;
            }
        }
        self.coverage.insert(source.key.clone(), Coverage::Pending);
        self.sources.insert(source.key.clone(), source);
        Ok(true)
    }

    pub fn source_valid(&self, key: &SourceKey, now: i64) -> bool {
        self.sources.get(key).is_some_and(|s| {
            s.available
                && s.recorded_at <= now
                && !self.tombstones.contains(&key.id)
                && s.access.policy_revision == self.policy_revision
                && s.valid_until.is_none_or(|until| now < until)
        })
    }

    pub fn status(&self, id: &str, now: i64) -> Status {
        self.status_inner(id, now, &mut BTreeSet::new())
    }

    fn status_inner(&self, id: &str, now: i64, visited: &mut BTreeSet<String>) -> Status {
        let Some(a) = self.assertions.get(id) else {
            return Status::Stale;
        };
        if visited.len() >= 256 || !visited.insert(id.to_string()) {
            return Status::Invalidated;
        }
        let mut status = Status::Candidate;
        for event in self
            .transitions
            .iter()
            .filter(|t| t.assertion_id == id && t.recorded_at <= now)
        {
            status = match event.action {
                Action::Assert => Status::Candidate,
                Action::Activate => Status::Active,
                Action::Dispute => Status::Disputed,
                Action::Supersede { .. } => Status::Superseded,
                Action::Retract => Status::Retracted,
                Action::Invalidate => Status::Invalidated,
                Action::Resolve => Status::Resolved,
            };
        }
        let transition_inputs: BTreeSet<_> = self
            .transitions
            .iter()
            .filter(|t| t.assertion_id == id)
            .flat_map(|t| t.input_dependencies.iter())
            .collect();
        if a.evidence
            .iter()
            .chain(&a.input_dependencies)
            .chain(transition_inputs.iter().copied())
            .any(|k| self.tombstones.contains(&k.id))
        {
            status = Status::Invalidated;
        } else if matches!(
            status,
            Status::Active | Status::Candidate | Status::Disputed
        ) {
            if a.recorded_at > now
                || a.effective_at > now
                || a.valid_from > now
                || a.valid_until.is_some_and(|until| now >= until)
                || a.access.policy_revision != self.policy_revision
                || a.evidence
                    .iter()
                    .chain(&a.input_dependencies)
                    .chain(transition_inputs.iter().copied())
                    .any(|k| !self.source_valid(k, now))
            {
                status = Status::Stale;
            } else {
                for dependency in &a.depends_on {
                    if !matches!(self.status_inner(dependency, now, visited), Status::Active) {
                        status = Status::Invalidated;
                        break;
                    }
                }
            }
        }
        visited.remove(id);
        status
    }

    pub fn project(&self, access: &AccessRequest<'_>, now: i64) -> Result<Projection, Error> {
        if !access.authorized
            || access.principal != self.principal
            || access.scope != self.scope
            || access.policy_revision != self.policy_revision
        {
            return Err(Error::Unauthorized);
        }
        let mut items = BTreeMap::new();
        for (id, a) in &self.assertions {
            if !a.access.permits(access) {
                continue;
            }
            let mut status = self.status(id, now);
            // An unprocessed newer input cannot silently leave the old value certain.
            if status == Status::Active
                && self.sources.values().any(|s| {
                    s.access.permits(access)
                        && self.source_valid(&s.key, now)
                        && !a.input_dependencies.contains(&s.key)
                        && matches!(
                            self.coverage.get(&s.key),
                            None | Some(Coverage::Pending | Coverage::Failed)
                        )
                        && s.sequence
                            > a.input_dependencies
                                .iter()
                                .filter_map(|k| self.sources.get(k))
                                .map(|s| s.sequence)
                                .max()
                                .unwrap_or(0)
                })
            {
                status = Status::Stale;
            }
            items.insert(id.clone(), status);
        }
        Ok(Projection {
            revision: self.revision,
            input_epoch: self.input_epoch,
            items,
        })
    }

    /// Atomically validates and applies a patch on a copy. Caller persists under CAS.
    pub fn apply(
        &mut self,
        patch: &StatePatch,
        context: &CommitContext<'_>,
    ) -> Result<bool, Error> {
        let encoded = serde_json::to_vec(patch).map_err(|_| Error::InvalidPatch)?;
        if encoded.len() > 16 * 1024 || patch.assertions.len() + patch.transitions.len() > 32 {
            return Err(Error::Limit);
        }
        let fingerprint = patch_fingerprint_bytes(&encoded);
        if !context.access.authorized
            || context.access.principal != self.principal
            || context.access.scope != self.scope
        {
            return Err(Error::Unauthorized);
        }
        if let Some(prior) = self.applied_patches.get(&patch.id) {
            return if prior == &fingerprint {
                Ok(false)
            } else {
                Err(Error::Conflict)
            };
        }
        if !context.enabled {
            return Err(Error::Disabled);
        }
        if patch.base_revision != self.revision
            || patch.input_epoch != self.input_epoch
            || patch.policy_revision != self.policy_revision
            || context.access.policy_revision != self.policy_revision
        {
            return Err(Error::StalePatch);
        }
        if patch.fence.is_empty()
            || patch.fence != context.live_fence
            || patch.id != context.issued_patch_id
        {
            return Err(Error::InvalidFence);
        }
        for key in &context.input_dependencies {
            if !self.source_valid(key, context.now)
                || !self.sources[key].access.permits(&context.access)
            {
                return Err(Error::InvalidSource);
            }
        }
        if !context
            .evidence_allowlist
            .is_subset(&context.input_dependencies)
        {
            return Err(Error::InvalidSource);
        }
        let mut next = self.clone();
        for assertion in &patch.assertions {
            next.validate_assertion(assertion, context)?;
            if next
                .assertions
                .insert(assertion.id.clone(), assertion.clone())
                .is_some()
            {
                return Err(Error::Conflict);
            }
        }
        next.validate_graph()?;
        for event in &patch.transitions {
            next.apply_transition(event, context)?;
        }
        for a in &patch.assertions {
            if !next
                .transitions
                .iter()
                .any(|t| t.assertion_id == a.id && t.action == Action::Assert)
            {
                return Err(Error::InvalidTransition);
            }
        }
        let mut covered = BTreeSet::new();
        for (source, coverage) in &patch.coverage {
            if !covered.insert(source)
                || !context.evidence_allowlist.contains(source)
                || !next.source_valid(source, context.now)
            {
                return Err(Error::InvalidSource);
            }
            if matches!(coverage, Coverage::Applied | Coverage::NoChange)
                && !next.sources[source].finalized
            {
                return Err(Error::InvalidSource);
            }
            if *coverage == Coverage::Applied
                && !patch.assertions.iter().any(|a| a.evidence.contains(source))
            {
                return Err(Error::InvalidPatch);
            }
            next.coverage.insert(source.clone(), *coverage);
        }
        next.revision = next.revision.checked_add(1).ok_or(Error::Limit)?;
        next.applied_patches.insert(patch.id.clone(), fingerprint);
        *self = next;
        Ok(true)
    }

    fn validate_assertion(&self, a: &Assertion, context: &CommitContext<'_>) -> Result<(), Error> {
        if !context.issued_assertion_ids.contains(&a.id)
            || a.id.is_empty()
            || a.payload_ref.is_empty()
            || a.semantic_key.is_empty()
            || a.semantic_key.len() > 128
            || a.evidence.is_empty()
            || !a.evidence.is_subset(&context.evidence_allowlist)
            || a.input_dependencies != context.input_dependencies
            || a.recorded_at != context.now
            || a.valid_until.is_some_and(|until| until <= a.valid_from)
            || a.provenance.model.is_empty()
            || a.provenance.release.is_empty()
            || a.provenance.extractor_version.is_empty()
            || a.provenance.prompt_digest.is_empty()
            || a.provenance.schema_version.is_empty()
            || a.provenance.config_digest.is_empty()
        {
            return Err(Error::InvalidPatch);
        }
        if context
            .issued_payload_bytes
            .get(&a.payload_ref)
            .is_none_or(|bytes| *bytes > 2_000)
        {
            return Err(Error::Limit);
        }
        if !a.access.permits(&context.access) {
            return Err(Error::Unauthorized);
        }
        for key in a.evidence.iter().chain(&a.input_dependencies) {
            let source = self.sources.get(key).ok_or(Error::InvalidSource)?;
            if !self.source_valid(key, context.now) || !a.access.derives_from(&source.access) {
                return Err(Error::Unauthorized);
            }
        }
        Ok(())
    }

    fn validate_graph(&self) -> Result<(), Error> {
        fn visit(
            ledger: &Ledger,
            id: &str,
            path: &mut BTreeSet<String>,
            done: &mut BTreeSet<String>,
        ) -> Result<(), Error> {
            if done.contains(id) {
                return Ok(());
            }
            if path.len() >= 256 {
                return Err(Error::Limit);
            }
            if !path.insert(id.to_string()) {
                return Err(Error::Cycle);
            }
            let assertion = ledger.assertions.get(id).ok_or(Error::UnknownDependency)?;
            for parent in &assertion.depends_on {
                let dependency = ledger
                    .assertions
                    .get(parent)
                    .ok_or(Error::UnknownDependency)?;
                if !assertion.access.derives_from(&dependency.access) {
                    return Err(Error::Unauthorized);
                }
                visit(ledger, parent, path, done)?;
            }
            path.remove(id);
            done.insert(id.to_string());
            Ok(())
        }
        let mut done = BTreeSet::new();
        for id in self.assertions.keys() {
            visit(self, id, &mut BTreeSet::new(), &mut done)?;
        }
        Ok(())
    }

    fn apply_transition(
        &mut self,
        t: &Transition,
        context: &CommitContext<'_>,
    ) -> Result<(), Error> {
        let a = self
            .assertions
            .get(&t.assertion_id)
            .ok_or(Error::UnknownDependency)?;
        if !a.access.permits(&context.access) {
            return Err(Error::Unauthorized);
        }
        if t.id.is_empty()
            || t.reason_code.is_empty()
            || t.reason_code.len() > 64
            || !t
                .reason_code
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            || t.recorded_at != context.now
            || t.evidence.is_empty()
            || !t.evidence.is_subset(&context.evidence_allowlist)
            || t.input_dependencies != context.input_dependencies
            || t.sequence != self.transitions.last().map_or(1, |last| last.sequence + 1)
            || self.transitions.iter().any(|prior| prior.id == t.id)
        {
            return Err(Error::InvalidTransition);
        }
        // Corrections themselves inherit the restrictions of their full input.
        if context
            .input_dependencies
            .iter()
            .any(|k| !a.access.derives_from(&self.sources[k].access))
        {
            return Err(Error::Unauthorized);
        }
        let existing = self.transitions.iter().any(|e| e.assertion_id == a.id);
        let status = self.status(&a.id, context.now);
        if t.action == Action::Assert {
            if existing {
                return Err(Error::InvalidTransition);
            }
        } else if !existing
            || !matches!(
                status,
                Status::Candidate | Status::Active | Status::Disputed
            )
        {
            return Err(Error::InvalidTransition);
        }
        if t.action == Action::Activate {
            let newest_support = |item: &Assertion| {
                item.evidence
                    .iter()
                    .filter_map(|key| self.sources.get(key))
                    .map(|source| source.sequence)
                    .max()
                    .unwrap_or(0)
            };
            if self.assertions.values().any(|other| {
                other.id != a.id
                    && other.kind == a.kind
                    && other.semantic_key == a.semantic_key
                    && other.access.task_request == a.access.task_request
                    && newest_support(other) > newest_support(a)
                    && self.transitions.iter().any(|event| {
                        event.assertion_id == other.id
                            && matches!(
                                event.action,
                                Action::Activate | Action::Retract | Action::Supersede { .. }
                            )
                    })
            }) {
                return Err(Error::InvalidSource);
            }
            if a.evidence
                .iter()
                .chain(&a.input_dependencies)
                .any(|k| !self.sources[k].finalized)
                || a.depends_on
                    .iter()
                    .any(|id| self.status(id, context.now) != Status::Active)
            {
                return Err(Error::InvalidSource);
            }
            if self.assertions.values().any(|other| {
                other.id != a.id
                    && other.kind == a.kind
                    && other.semantic_key == a.semantic_key
                    && other.access.task_request == a.access.task_request
                    && self.status(&other.id, context.now) == Status::Active
            }) {
                return Err(Error::Conflict);
            }
        }
        if let Action::Supersede { by } = &t.action {
            let replacement = self.assertions.get(by).ok_or(Error::UnknownDependency)?;
            // World assertions are versioned: a v1 assertion may be replaced by
            // an equivalent v2 assertion, so the fixed `wm1:`/`wm2:` key differs
            // even though the logical identity is the same. The versioned World
            // validator enforces logical identity; here we only allow a
            // cross-version key change and reject same-version mismatches.
            let same_identity = replacement.semantic_key == a.semantic_key
                || (a.kind.is_world()
                    && replacement.kind.is_world()
                    && world_key_version(&replacement.semantic_key)
                        != world_key_version(&a.semantic_key));
            if by == &a.id
                || replacement.kind != a.kind
                || !same_identity
                || replacement.access.task_request != a.access.task_request
                || !replacement.access.derives_from(&a.access)
            {
                return Err(Error::InvalidTransition);
            }
        }
        self.transitions.push(t.clone());
        Ok(())
    }

    /// Returns erasable payload references; persistence must erase them atomically
    /// with tombstones/outbox. This operation is independent of feature enablement.
    pub fn forget(&mut self, source_id: &str) -> Result<ForgetEffect, Error> {
        if source_id.is_empty() {
            return Err(Error::InvalidSource);
        }
        if !self.tombstones.contains(source_id) {
            self.input_epoch = self.input_epoch.checked_add(1).ok_or(Error::Limit)?;
            self.tombstones.insert(source_id.to_string());
        }
        let mut affected: BTreeSet<String> = self
            .assertions
            .values()
            .filter(|a| {
                a.evidence
                    .iter()
                    .chain(&a.input_dependencies)
                    .any(|k| k.id == source_id)
            })
            .map(|a| a.id.clone())
            .collect();
        for transition in &self.transitions {
            if transition
                .input_dependencies
                .iter()
                .any(|k| k.id == source_id)
            {
                affected.insert(transition.assertion_id.clone());
            }
        }
        loop {
            let before = affected.len();
            for a in self.assertions.values() {
                if !a.depends_on.is_disjoint(&affected) {
                    affected.insert(a.id.clone());
                }
            }
            if before == affected.len() {
                break;
            }
        }
        let payload_refs = affected
            .iter()
            .map(|id| self.assertions[id].payload_ref.clone())
            .collect();
        for source in self.sources.values_mut().filter(|s| s.key.id == source_id) {
            source.available = false;
            source.digest.clear();
        }
        for id in &affected {
            // Semantic keys and provenance may contain identifying derived values.
            let a = self.assertions.get_mut(id).expect("known dependency");
            a.semantic_key.clear();
            a.provenance.prompt_digest.clear();
            a.provenance.config_digest.clear();
        }
        for t in &mut self.transitions {
            if affected.contains(&t.assertion_id) {
                t.reason_code.clear();
            }
        }
        // Fingerprints may encode forgotten semantic keys; IDs still block replay.
        for fingerprint in self.applied_patches.values_mut() {
            fingerprint.clear();
        }
        Ok(ForgetEffect {
            assertions: affected,
            payload_refs,
        })
    }
}
