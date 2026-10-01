use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Entry {
    pub(crate) written: String,
    pub(crate) spoken: String,
}

/// Required wrapper distinguishes a missing guard from an explicitly absent row.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ExpectedEntry {
    pub(crate) spoken: Option<String>,
}

pub(crate) fn validate(entry: &Entry) -> Result<(), String> {
    if entry.written.trim() != entry.written
        || entry.spoken.trim() != entry.spoken
        || entry.written.is_empty()
        || entry.written.len() > 200
        || entry.spoken.len() > 400
        || entry.written.chars().any(char::is_control)
        || entry.spoken.chars().any(char::is_control)
    {
        return Err("表記または読み方が不正です。".into());
    }
    Ok(())
}
