//! `web_search` Rust provider (WF-07 / WF-08 / WF-09).
//!
//! Fixed-endpoint HTTP clients only: DuckDuckGo HTML/Lite first, Brave
//! Search as fallback when `BRAVE_SEARCH_API_KEY` is present. Search pages
//! are never loaded into the generic WebView worker.
//!
//! Bounds (all enforced before parsing):
//! raw HTML <= 2 MiB, candidates <= 1_000, results <= 20,
//! title <= 200 / snippet <= 500 / URL <= 2048 chars.

use super::contracts::{compact_text, SearchInput, WebFetchCancel, WebFetchFailure};
use async_trait::async_trait;
use futures_util::StreamExt;
use std::{collections::HashSet, time::Duration};
#[path = "search/match_bracket.rs"]
mod match_bracket;
#[path = "search/raw_hit.rs"]
mod raw_hit;
use match_bracket::{
    assert_not_challenge, attr_value, filter_and_project, find, match_bracket,
    normalize_result_url, parse_brave_json, parse_ddg_html, parse_ddg_lite, snippet_after,
    strip_tags, urlencoding_decode,
};
pub use match_bracket::{is_allowed_result_url, render_compact};
pub use raw_hit::{RawHit, RustSearchProvider, SearchHit, SearchOutcome, SearchProvider};
use raw_hit::{MAX_CANDIDATES, MAX_RESPONSE_BYTES};
#[cfg(test)]
#[path = "search/tests.rs"]
mod tests;
