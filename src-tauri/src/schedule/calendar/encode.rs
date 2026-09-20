use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::schedule::ledger::{Entry, Status};

pub(crate) fn event_title(entry: &Entry, body: Option<&str>, classification: &str) -> String {
    let restricted = matches!(classification, "confidential" | "restricted");
    let base = if restricted {
        entry.subject_ref.clone()
    } else {
        body.filter(|value| !value.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| entry.subject_ref.clone())
    };
    match entry.status {
        Status::Fired => format!("✓ {base}"),
        Status::Withdrawn => format!("✕ {base}"),
        _ => base,
    }
}

pub(crate) fn event_hash(entry: &Entry, title: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(entry.id.as_bytes());
    hasher.update(entry.revision.to_le_bytes());
    hasher.update(entry.status.as_str().as_bytes());
    hasher.update(entry.due_at.to_le_bytes());
    hasher.update(title.as_bytes());
    format!("{:x}", hasher.finalize())
}

pub(crate) fn event_body(entry: &Entry, title: &str, hash: &str) -> Value {
    json!({
        "summary": title,
        "start": { "dateTime": crate::schedule::calendar::rfc3339(entry.due_at), "timeZone": "UTC" },
        "end": { "dateTime": crate::schedule::calendar::rfc3339(entry.window_end_at.unwrap_or(entry.due_at + 15 * 60 * 1000)), "timeZone": "UTC" },
        "extendedProperties": {
            "private": {
                "saaa_entry_id": entry.id,
                "saaa_rev": entry.revision.to_string(),
                "saaa_hash": hash
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schedule::ledger::{Entry, Kind, Origin, Status};

    fn entry(status: Status) -> Entry {
        Entry {
            id: "e".into(),
            kind: Kind::Reminder,
            subject_ref: "goal:g".into(),
            scope_ref: "scope:primary".into(),
            due_at: 1,
            window_end_at: None,
            status,
            origin: Origin::UserExplicit,
            delegation_ref: None,
            revision: 1,
            supersedes: None,
            created_at: 1,
            fired_at: None,
            fire_result: None,
            payload_id: None,
        }
    }

    #[test]
    fn sl_20_marks_terminal_titles() {
        assert_eq!(
            event_title(&entry(Status::Fired), Some("secret"), "confidential"),
            "✓ goal:g"
        );
        assert_eq!(
            event_title(&entry(Status::Withdrawn), Some("body"), "internal"),
            "✕ body"
        );
    }
}
