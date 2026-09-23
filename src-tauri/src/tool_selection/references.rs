//! In-memory opaque references issued by a search result. They are run-scoped, TTL-bounded and
//! never survive a restart; invoke re-checks epochs before using one.

use serde_json::Value;
use std::collections::HashMap;
use std::sync::Mutex;

use super::contracts::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReferenceKind {
    Candidate,
    Execution,
}

impl ReferenceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Candidate => "candidate",
            Self::Execution => "execution",
        }
    }
}

#[derive(Clone, Debug)]
pub struct ReferenceEntry {
    pub kind: ReferenceKind,
    pub decision_id: String,
    pub revision_id: String,
    pub tool_id: String,
    pub schema_hash: String,
    pub principal_id: String,
    pub conversation_id: String,
    pub run_id: Option<String>,
    pub project_id: Option<String>,
    pub task_id: Option<String>,
    pub catalog_epoch: i64,
    pub acl_epoch: i64,
    pub rule_epoch: i64,
    pub input_schema: Value,
    pub created_at_ms: i64,
}

impl ReferenceEntry {
    pub fn scope_key(&self) -> String {
        self.run_id
            .clone()
            .unwrap_or_else(|| format!("conversation:{}", self.conversation_id))
    }
}

#[derive(Default)]
pub struct ReferenceStore {
    pub(super) entries: Mutex<HashMap<String, ReferenceEntry>>,
}

impl ReferenceStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Issues one reference for the run, enforcing the per-run cap and TTL.
    pub fn issue(&self, entry: ReferenceEntry, now_ms: i64) -> ToolSelectionResult<String> {
        let mut entries = self
            .entries
            .lock()
            .map_err(|_| ToolSelectionError::unavailable())?;
        purge_expired(&mut entries, now_ms);
        let scope = entry.scope_key();
        let open = entries
            .values()
            .filter(|existing| existing.scope_key() == scope)
            .count();
        if open >= REFERENCE_MAX_PER_RUN {
            return Err(ToolSelectionError::capacity());
        }
        let reference = uuid::Uuid::new_v4().to_string();
        entries.insert(reference.clone(), entry);
        Ok(reference)
    }

    /// Resolves a reference without consuming it. `kind` must match; a mismatched or expired
    /// reference is reported as not found.
    pub fn resolve(
        &self,
        reference: &str,
        kind: ReferenceKind,
        now_ms: i64,
    ) -> Option<ReferenceEntry> {
        let mut entries = self.entries.lock().ok()?;
        purge_expired(&mut entries, now_ms);
        let entry = entries.get(reference)?;
        (entry.kind == kind).then(|| entry.clone())
    }

    /// Drops every reference owned by a finished run or an invalidated scope.
    pub fn invalidate_scope(&self, scope: &str) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.retain(|_, entry| entry.scope_key() != scope);
        }
    }
}

fn purge_expired(entries: &mut HashMap<String, ReferenceEntry>, now_ms: i64) {
    entries.retain(|_, entry| now_ms - entry.created_at_ms < REFERENCE_TTL_MILLIS);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(kind: ReferenceKind) -> ReferenceEntry {
        ReferenceEntry {
            kind,
            decision_id: "d".into(),
            revision_id: "r".into(),
            tool_id: "t".into(),
            schema_hash: "s".into(),
            principal_id: "P1".into(),
            conversation_id: "C1".into(),
            run_id: Some("run-1".into()),
            project_id: None,
            task_id: None,
            catalog_epoch: 1,
            acl_epoch: 1,
            rule_epoch: 1,
            input_schema: Value::Null,
            created_at_ms: 0,
        }
    }

    #[test]
    fn reference_resolves_and_expires() {
        let store = ReferenceStore::new();
        let reference = store
            .issue(entry(ReferenceKind::Candidate), 0)
            .expect("issue");
        assert!(store
            .resolve(&reference, ReferenceKind::Candidate, 1000)
            .is_some());
        assert!(store
            .resolve(&reference, ReferenceKind::Candidate, REFERENCE_TTL_MILLIS)
            .is_none());
    }

    #[test]
    fn wrong_kind_is_not_found() {
        let store = ReferenceStore::new();
        let reference = store
            .issue(entry(ReferenceKind::Candidate), 0)
            .expect("issue");
        assert!(store
            .resolve(&reference, ReferenceKind::Execution, 0)
            .is_none());
    }

    #[test]
    fn per_run_capacity_is_enforced() {
        let store = ReferenceStore::new();
        for _ in 0..REFERENCE_MAX_PER_RUN {
            store
                .issue(entry(ReferenceKind::Candidate), 0)
                .expect("issue");
        }
        let error = store.issue(entry(ReferenceKind::Candidate), 0).unwrap_err();
        assert_eq!(error.code, ToolSelectionErrorCode::Capacity);
    }

    #[test]
    fn invalidation_removes_run_references() {
        let store = ReferenceStore::new();
        let reference = store
            .issue(entry(ReferenceKind::Candidate), 0)
            .expect("issue");
        store.invalidate_scope("run-1");
        assert!(store
            .resolve(&reference, ReferenceKind::Candidate, 0)
            .is_none());
    }
}
