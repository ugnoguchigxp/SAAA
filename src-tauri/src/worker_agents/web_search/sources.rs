//! Task-local source book: which URLs the host has recorded, which of them are usable, and
//! which hosts are excluded. The worker only ever sees URLs this book marks usable, and claims
//! are validated against it.
use std::collections::{BTreeSet, HashMap, HashSet};

use sha2::{Digest, Sha256};

pub(crate) const MAX_SEARCHES: u32 = 3;
pub(crate) const MAX_FETCHES: u32 = 4;
pub(crate) const MAX_FAILED_SOURCES: u32 = 4;
/// Flagged pages on one host before the host is excluded for the rest of the task. Task-local
/// only: no host-level permanent entry exists.
pub(crate) const HOST_EXCLUDE_THRESHOLD: u32 = 2;
pub(crate) const MAX_URL_BYTES: usize = 2048;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum SourceKind {
    SearchHit,
    Fetched,
    UserSupplied,
}

impl SourceKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::SearchHit => "search_hit",
            Self::Fetched => "fetched",
            Self::UserSupplied => "user_supplied",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "search_hit" => Some(Self::SearchHit),
            "fetched" => Some(Self::Fetched),
            "user_supplied" => Some(Self::UserSupplied),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SourceStatus {
    Recorded,
    Usable,
    Failed,
    Excluded,
}

impl SourceStatus {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Recorded => "recorded",
            Self::Usable => "usable",
            Self::Failed => "failed",
            Self::Excluded => "excluded",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "recorded" => Some(Self::Recorded),
            "usable" => Some(Self::Usable),
            "failed" => Some(Self::Failed),
            "excluded" => Some(Self::Excluded),
            _ => None,
        }
    }
}

/// Parses an http(s) URL and drops the fragment. `None` for anything else, for URLs with
/// credentials and for URLs over the stored length limit.
pub(crate) fn normalize_url(raw: &str) -> Option<String> {
    if raw.len() > MAX_URL_BYTES {
        return None;
    }
    let mut parsed = url::Url::parse(raw.trim()).ok()?;
    if !matches!(parsed.scheme(), "http" | "https")
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return None;
    }
    parsed.set_fragment(None);
    let normalized = parsed.to_string();
    (normalized.len() <= MAX_URL_BYTES).then_some(normalized)
}

const MAX_CITABLE_URL_BYTES: usize = 300;

/// A URL may reach the conversation agent only if a page author cannot use it as a free-text
/// channel: short, no whitespace, and its decoded words pass the plugin guard like claim text.
pub(crate) fn citable_url(url: &str) -> bool {
    if url.len() > MAX_CITABLE_URL_BYTES || url.chars().any(char::is_whitespace) {
        return false;
    }
    let bytes = url.as_bytes();
    let mut decoded: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        let escaped = (byte == b'%')
            .then(|| url.get(index + 1..index + 3))
            .flatten()
            .and_then(|digits| u8::from_str_radix(digits, 16).ok());
        if let Some(value) = escaped {
            decoded.push(value);
            index += 3;
            continue;
        }
        decoded.push(match byte {
            b'+' | b'-' | b'_' | b'/' | b'.' | b'?' | b'&' | b'=' | b':' => b' ',
            other => other,
        });
        index += 1;
    }
    let words = String::from_utf8_lossy(&decoded);
    tauri_plugin_llm_fetch::inspect_plain_text_bounded(&words, 2_000).decision
        == tauri_plugin_llm_fetch::GuardDecision::Allow
}

/// Lowercased host without a leading `www.`. No public-suffix handling (not a dependency).
pub(crate) fn host_of(raw: &str) -> Option<String> {
    let parsed = url::Url::parse(raw.trim()).ok()?;
    let host = parsed.host_str()?.to_ascii_lowercase();
    let host = host.strip_prefix("www.").unwrap_or(&host);
    (!host.is_empty()).then(|| host.to_string())
}

/// Key of `worker_url_blocklist`: sha256 over the normalized URL.
pub(crate) fn url_hash(raw: &str) -> Option<String> {
    let normalized = normalize_url(raw)?;
    Some(format!("{:x}", Sha256::digest(normalized.as_bytes())))
}

#[derive(Debug, Clone)]
pub(crate) struct SourceEntry {
    pub status: SourceStatus,
    pub host: String,
    pub date: Option<String>,
}

#[derive(Debug, Default)]
pub(crate) struct SourceBook {
    entries: HashMap<(SourceKind, String), SourceEntry>,
    host_flags: HashMap<String, u32>,
    categories: BTreeSet<String>,
    /// Results the host dropped before any model saw them (blocklist, excluded host, plugin
    /// `blockedResultCount`). Counted as excluded, not as failed sources.
    pub unsafe_dropped: u32,
    pub hits_seen: u32,
}

impl SourceBook {
    pub(crate) fn entry(&self, kind: SourceKind, url: &str) -> Option<&SourceEntry> {
        self.entries.get(&(kind, url.to_string()))
    }

    pub(crate) fn insert(&mut self, kind: SourceKind, url: &str, entry: SourceEntry) {
        self.entries.insert((kind, url.to_string()), entry);
    }

    pub(crate) fn set_status(&mut self, kind: SourceKind, url: &str, status: SourceStatus) {
        if let Some(entry) = self.entries.get_mut(&(kind, url.to_string())) {
            entry.status = status;
        }
    }

    /// `kind` of `url` is recorded and usable.
    pub(crate) fn is_usable(&self, kind: SourceKind, url: &str) -> bool {
        self.entry(kind, url)
            .is_some_and(|entry| entry.status == SourceStatus::Usable)
    }

    /// A URL may be fetched only when it is a usable search hit or user-supplied URL of this
    /// task (exact match) and its host is not excluded.
    pub(crate) fn fetch_allowed(&self, url: &str) -> bool {
        [SourceKind::SearchHit, SourceKind::UserSupplied]
            .into_iter()
            .filter_map(|kind| self.entry(kind, url))
            .any(|entry| entry.status == SourceStatus::Usable && !self.host_excluded(&entry.host))
    }

    /// Usable search-hit URLs (sorted) that a fetch would currently be allowed for.
    pub(crate) fn usable_hit_urls(&self) -> Vec<String> {
        let mut urls: Vec<String> = self
            .entries
            .iter()
            .filter(|((kind, _), entry)| {
                *kind == SourceKind::SearchHit
                    && entry.status == SourceStatus::Usable
                    && !self.host_excluded(&entry.host)
            })
            .map(|((_, url), _)| url.clone())
            .collect();
        urls.sort();
        urls
    }

    pub(crate) fn host_excluded(&self, host: &str) -> bool {
        self.host_flags
            .get(host)
            .is_some_and(|count| *count >= HOST_EXCLUDE_THRESHOLD)
    }

    pub(crate) fn flag_host(&mut self, host: &str) {
        *self.host_flags.entry(host.to_string()).or_insert(0) += 1;
    }

    pub(crate) fn add_categories<'a>(&mut self, categories: impl IntoIterator<Item = &'a str>) {
        self.categories
            .extend(categories.into_iter().map(str::to_string));
    }

    /// Distinct URLs that are usable (a fetched page and its hit count once).
    pub(crate) fn usable_count(&self) -> u32 {
        self.entries
            .iter()
            .filter(|(_, entry)| entry.status == SourceStatus::Usable)
            .map(|((_, url), _)| url.as_str())
            .collect::<HashSet<_>>()
            .len() as u32
    }

    pub(crate) fn failed_count(&self) -> u32 {
        self.distinct(|status| status == SourceStatus::Failed)
    }

    /// Failed or excluded URLs plus results dropped before recording.
    pub(crate) fn excluded_count(&self) -> u32 {
        self.distinct(|status| matches!(status, SourceStatus::Failed | SourceStatus::Excluded))
            + self.unsafe_dropped
    }

    pub(crate) fn excluded_domains(&self) -> u32 {
        self.entries
            .values()
            .filter(|entry| matches!(entry.status, SourceStatus::Failed | SourceStatus::Excluded))
            .map(|entry| entry.host.as_str())
            .collect::<HashSet<_>>()
            .len() as u32
    }

    pub(crate) fn categories(&self) -> Vec<String> {
        self.categories.iter().cloned().collect()
    }

    fn distinct(&self, wanted: impl Fn(SourceStatus) -> bool) -> u32 {
        self.entries
            .iter()
            .filter(|(_, entry)| wanted(entry.status))
            .map(|((_, url), _)| url.as_str())
            .collect::<HashSet<_>>()
            .len() as u32
    }

    /// Rebuilds the task-local host counters from failed fetched pages of earlier attempts (a flagged
    /// page is always recorded as a failed `fetched` row, whatever its origin).
    pub(crate) fn rebuild_host_flags(&mut self) {
        let mut flags: HashMap<String, u32> = HashMap::new();
        for ((kind, _), entry) in &self.entries {
            if entry.status == SourceStatus::Failed && *kind == SourceKind::Fetched {
                *flags.entry(entry.host.clone()).or_insert(0) += 1;
            }
        }
        self.host_flags = flags;
    }
}
