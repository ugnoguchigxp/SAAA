//! Source-backed additions to a WorldFrame v2.
//!
//! These types deliberately contain only state which is safe to project to a
//! provider.  They are a wire contract, not a mirror of any runtime table.

use serde::{Deserialize, Deserializer, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorldScope {
    /// The selected Project, when the turn has one.  A user-only turn leaves
    /// this unset and may still carry Situation state.
    pub focus_scope_key: Option<String>,
    pub allowed_scope_keys: Vec<String>,
    pub digest: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorldSourceKind {
    Situation,
    Coding,
    Delegation,
    Schedule,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorldSourceAvailability {
    Available,
    Unknown,
    Unavailable,
    Stale,
    Denied,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum WorldSourcePayload {
    Situation {
        scene: Option<String>,
        attention: Option<String>,
        hold: Option<String>,
        signal_health: String,
        sequence: u64,
        monitor_enabled: bool,
        inferred: bool,
    },
    Coding {
        job_id: String,
        owner_state: String,
        phase: String,
        revision: Option<u64>,
    },
    Delegation {
        task_id: String,
        delegation_id: Option<String>,
        delegation_status: String,
        goal_id: Option<String>,
        goal_status: String,
        status: String,
        loop_state: Option<String>,
        revision: Option<u64>,
    },
    Schedule {
        entry_id: String,
        status: String,
        due_at_ms: Option<i64>,
        revision: Option<u64>,
    },
}

impl WorldSourcePayload {
    pub fn kind(&self) -> WorldSourceKind {
        match self {
            Self::Situation { .. } => WorldSourceKind::Situation,
            Self::Coding { .. } => WorldSourceKind::Coding,
            Self::Delegation { .. } => WorldSourceKind::Delegation,
            Self::Schedule { .. } => WorldSourceKind::Schedule,
        }
    }
}

/// An individual, source-owned observation.  Denied entries are intentionally
/// rejected here: the caller must retain their omission reason locally rather
/// than serializing identifiers to a provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorldSourceEntry {
    pub kind: WorldSourceKind,
    pub source_id: String,
    pub owner_scope_key: String,
    pub availability: WorldSourceAvailability,
    pub observed_at_ms: i64,
    pub as_of_ms: i64,
    pub version: Option<String>,
    pub digest: String,
    pub payload: Option<WorldSourcePayload>,
    pub reason_code: Option<String>,
}

impl WorldSourceEntry {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.source_id.is_empty()
            || self.owner_scope_key.is_empty()
            || self.digest.is_empty()
            || self.observed_at_ms < 0
            || self.as_of_ms < 0
        {
            return Err("invalid source entry");
        }
        match self.availability {
            WorldSourceAvailability::Available => match (&self.payload, &self.reason_code) {
                (Some(payload), None) if payload.kind() == self.kind => Ok(()),
                _ => Err("available source needs matching payload"),
            },
            WorldSourceAvailability::Denied => Err("denied source must not be serialized"),
            _ => match (&self.payload, &self.reason_code) {
                (None, Some(reason)) if !reason.is_empty() => Ok(()),
                _ => Err("unavailable source needs reason and no payload"),
            },
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WorldSourceEntryWire {
    kind: WorldSourceKind,
    source_id: String,
    owner_scope_key: String,
    availability: WorldSourceAvailability,
    observed_at_ms: i64,
    as_of_ms: i64,
    version: Option<String>,
    digest: String,
    payload: Option<WorldSourcePayload>,
    reason_code: Option<String>,
}

impl<'de> Deserialize<'de> for WorldSourceEntry {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = WorldSourceEntryWire::deserialize(deserializer)?;
        let value = Self {
            kind: wire.kind,
            source_id: wire.source_id,
            owner_scope_key: wire.owner_scope_key,
            availability: wire.availability,
            observed_at_ms: wire.observed_at_ms,
            as_of_ms: wire.as_of_ms,
            version: wire.version,
            digest: wire.digest,
            payload: wire.payload,
            reason_code: wire.reason_code,
        };
        value.validate().map_err(serde::de::Error::custom)?;
        Ok(value)
    }
}

/// Group state preserves the important distinction between "nothing is due"
/// and "the source was unavailable" without inventing a sentinel source row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorldSourceGroup {
    pub kind: WorldSourceKind,
    pub availability: WorldSourceAvailability,
    pub entries: Vec<WorldSourceEntry>,
    pub omission_reason: Option<String>,
}

impl WorldSourceGroup {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.availability == WorldSourceAvailability::Denied {
            return Err("denied group must stay host-side");
        }
        if self.availability == WorldSourceAvailability::Available {
            if self.omission_reason.is_some()
                || self.entries.iter().any(|entry| entry.kind != self.kind)
            {
                return Err("invalid available source group");
            }
        } else if !self.entries.is_empty()
            || self.omission_reason.as_deref().unwrap_or("").is_empty()
        {
            return Err("unavailable group needs an omission reason");
        }
        for entry in &self.entries {
            entry.validate()?;
        }
        Ok(())
    }
}

pub fn canonicalize_scope(scope: &mut WorldScope) -> Result<(), &'static str> {
    scope.allowed_scope_keys.sort();
    scope.allowed_scope_keys.dedup();
    if scope.allowed_scope_keys.is_empty() || scope.digest.is_empty() {
        return Err("invalid world scope");
    }
    if scope.allowed_scope_keys.iter().any(|key| key.is_empty()) {
        return Err("invalid world scope key");
    }
    if let Some(focus) = &scope.focus_scope_key {
        if !scope.allowed_scope_keys.contains(focus) {
            return Err("focus is not allowed");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(availability: WorldSourceAvailability) -> WorldSourceEntry {
        WorldSourceEntry {
            kind: WorldSourceKind::Situation,
            source_id: "situation".into(),
            owner_scope_key: "user:primary".into(),
            availability,
            observed_at_ms: 1,
            as_of_ms: 1,
            version: None,
            digest: "d".into(),
            payload: Some(WorldSourcePayload::Situation {
                scene: None,
                attention: None,
                hold: None,
                signal_health: "healthy".into(),
                sequence: 1,
                monitor_enabled: true,
                inferred: false,
            }),
            reason_code: None,
        }
    }

    #[test]
    fn wr_t01_available_requires_matching_typed_payload() {
        assert!(entry(WorldSourceAvailability::Available).validate().is_ok());
        let mut bad = entry(WorldSourceAvailability::Available);
        bad.payload = None;
        assert!(bad.validate().is_err());
    }

    #[test]
    fn wr_t01_unavailable_is_not_an_empty_available_list() {
        let mut unavailable = entry(WorldSourceAvailability::Unavailable);
        unavailable.payload = None;
        unavailable.reason_code = Some("source_offline".into());
        assert!(unavailable.validate().is_ok());
        assert_ne!(
            serde_json::to_value(unavailable).unwrap(),
            serde_json::to_value(entry(WorldSourceAvailability::Available)).unwrap()
        );
    }
}
