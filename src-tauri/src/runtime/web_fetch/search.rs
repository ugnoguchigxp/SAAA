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
include!("search.d/01.rs");
include!("search.d/02.rs");
include!("search.d/03.rs");
