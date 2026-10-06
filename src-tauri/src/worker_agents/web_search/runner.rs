//! One attempt of the web-search profile (§6.3): the model chooses one JSON action per turn, the
//! host runs the tools, records every source, and keeps flagged material away from the model.
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::time::Instant;

use super::audit::{self, SourceRecord};
use super::checker::{CheckVerdict, Checker};
use super::claims::validate_claims;
use super::envelope::{
    clip, hits_envelope, page_envelope, parse_action, parse_fetch, parse_search, Action,
    FetchEnvelope, RawHit, MAX_SNIPPET_CHARS, MAX_TITLE_CHARS,
};
use super::sources::{
    citable_url, host_of, normalize_url, url_hash, SourceBook, SourceEntry, SourceKind,
    SourceStatus, MAX_FAILED_SOURCES, MAX_FETCHES, MAX_SEARCHES,
};
use crate::worker_agents::contracts::*;

const MODEL_TIMEOUT: Duration = Duration::from_secs(30);
const TOOL_TIMEOUT: Duration = Duration::from_secs(25);
const MAX_QUERY_CHARS: usize = 400;
const MAX_KNOWN_SOURCES: usize = 10;

/// The attempt runner of the builtin web-search profile.
pub(crate) struct WebSearchRunner;

pub(crate) fn runner() -> Arc<dyn AttemptRunner> {
    Arc::new(WebSearchRunner)
}

#[async_trait]
impl AttemptRunner for WebSearchRunner {
    async fn run(&self, env: &AttemptEnv<'_>) -> Result<WorkerOutput, AttemptError> {
        Attempt::start(env)?.drive().await
    }
}

/// System context of the revision followed by its skills (`## Skill: <name>`).
pub(crate) fn compose_system(revision: &LoadedRevision) -> String {
    let mut system = revision.system_context.clone();
    for skill in &revision.skills {
        system.push_str(&format!("\n\n## Skill: {}\n{}", skill.name, skill.body));
    }
    system
}

fn ledger(error: String) -> AttemptError {
    // A source that cannot be recorded cannot be vouched for: fail closed.
    AttemptError::Transport(format!("worker source ledger unavailable: {error}"))
}

struct Attempt<'a, 'e> {
    env: &'a AttemptEnv<'e>,
    book: SourceBook,
    checker: Checker<'a>,
    query: String,
    supplied: Vec<String>,
    searches: u32,
    fetches: u32,
}

impl<'a, 'e> Attempt<'a, 'e> {
    fn start(env: &'a AttemptEnv<'e>) -> Result<Self, AttemptError> {
        let query = env
            .input
            .get("query")
            .and_then(Value::as_str)
            .map(|query| clip(query.trim(), MAX_QUERY_CHARS))
            .filter(|query| !query.is_empty())
            .ok_or(AttemptError::Terminal(FailureCode::InputInvalid))?;
        let book = audit::load_sources(env.writer, env.task_id).map_err(ledger)?;
        // Budgets belong to the task, not to one attempt: a retry continues from what was spent.
        let spent = audit::task_stats(env.writer, env.task_id).map_err(ledger)?;
        let mut attempt = Self {
            env,
            book,
            checker: Checker::new(env.model, env.route, env.cancellation),
            query,
            supplied: Vec::new(),
            searches: spent.searches,
            fetches: spent.fetches,
        };
        attempt.record_supplied_urls()?;
        Ok(attempt)
    }

    /// `input.urls` entries count only when the exact string occurs in the user's utterance.
    fn record_supplied_urls(&mut self) -> Result<(), AttemptError> {
        let urls: Vec<String> = self
            .env
            .input
            .get("urls")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        for raw in urls.into_iter().take(3) {
            if raw.is_empty() || !self.env.user_text.contains(&raw) {
                continue;
            }
            let (Some(url), Some(host)) = (normalize_url(&raw), host_of(&raw)) else {
                continue;
            };
            if self.book.entry(SourceKind::UserSupplied, &url).is_some() {
                if self.book.is_usable(SourceKind::UserSupplied, &url) {
                    self.supplied.push(url);
                }
                continue;
            }
            let blocked = match url_hash(&url) {
                Some(hash) => audit::is_blocklisted(self.env.writer, &hash).map_err(ledger)?,
                None => true,
            };
            let reason = if blocked {
                Some("blocklisted")
            } else if self.book.host_excluded(&host) {
                Some("host_excluded")
            } else if !citable_url(&url) {
                Some("url_not_citable")
            } else {
                None
            };
            let status = if reason.is_some() {
                SourceStatus::Excluded
            } else {
                SourceStatus::Usable
            };
            let mut record = SourceRecord::new(&url, &host, SourceKind::UserSupplied, status);
            record.fail_reason = reason;
            audit::upsert_source(self.env.writer, self.env.task_id, &record).map_err(ledger)?;
            self.book.insert(
                SourceKind::UserSupplied,
                &url,
                SourceEntry {
                    status,
                    host,
                    date: None,
                },
            );
            if status == SourceStatus::Usable {
                self.supplied.push(url);
            }
        }
        Ok(())
    }

    fn guard_run(&self) -> Result<(), AttemptError> {
        if self.env.cancellation.is_cancelled() {
            return Err(AttemptError::Cancelled);
        }
        if Instant::now() >= self.env.deadline {
            return Err(AttemptError::DeadlineExceeded);
        }
        Ok(())
    }

    fn remaining(&self) -> Duration {
        self.env.deadline.saturating_duration_since(Instant::now())
    }

    fn render_input(&self, turns: &[String], step: u32) -> String {
        let max_steps = self.env.revision.limits.max_steps;
        let mut known: Vec<String> = Vec::new();
        if turns.is_empty() {
            known = self
                .book
                .usable_hit_urls()
                .into_iter()
                .take(MAX_KNOWN_SOURCES)
                .collect();
        }
        let mut out = format!(
            "REQUEST_DATA {}\n",
            json!({"query": self.query, "urls": self.supplied})
        );
        if !known.is_empty() {
            out.push_str(&format!(
                "KNOWN_USABLE_URLS_FROM_EARLIER_ATTEMPT {}\n",
                json!(known)
            ));
        }
        for turn in turns {
            out.push_str(turn);
            out.push('\n');
        }
        out.push_str(&format!(
            "STATUS {}\nReply with exactly one JSON action object.",
            json!({
                "step": step,
                "stepsLeft": max_steps.saturating_sub(step - 1),
                "searchesLeft": MAX_SEARCHES.saturating_sub(self.searches),
                "fetchesLeft": MAX_FETCHES.saturating_sub(self.fetches),
            })
        ));
        out
    }

    async fn drive(mut self) -> Result<WorkerOutput, AttemptError> {
        let env = self.env;
        let system = compose_system(env.revision);
        let max_steps = env.revision.limits.max_steps;
        let mut turns: Vec<String> = Vec::new();
        let mut gave_up = false;
        for step in 1..=max_steps {
            self.guard_run()?;
            if self.book.failed_count() >= MAX_FAILED_SOURCES {
                break;
            }
            let input = self.render_input(&turns, step);
            let timeout = self.remaining().min(MODEL_TIMEOUT);
            let reply = env
                .model
                .complete(env.route, &system, &input, env.cancellation, timeout)
                .await;
            self.guard_run()?;
            let reply = reply.map_err(AttemptError::Transport)?;
            let action = parse_action(&reply).map_err(AttemptError::InvalidOutput)?;
            let (echo, result) = match action {
                Action::GiveUp => {
                    gave_up = true;
                    break;
                }
                Action::Finish { claims, coverage } => {
                    let claims = validate_claims(
                        claims,
                        &self.book,
                        env.revision.completion.min_items,
                        coverage,
                    )?;
                    return Ok(WorkerOutput::WebClaimsV1(claims));
                }
                Action::WebSearch { query } => {
                    let query = clip(query.trim(), MAX_QUERY_CHARS);
                    let echo = json!({"action": "web_search", "query": query});
                    (echo, self.search(&query).await?)
                }
                Action::FetchContent { url, query } => {
                    let query = clip(query.as_deref().unwrap_or("").trim(), MAX_QUERY_CHARS);
                    let echo =
                        json!({"action": "fetch_content", "url": clip(&url, 2048), "query": query});
                    (echo, self.fetch(&url, &query).await?)
                }
            };
            turns.push(format!("ACTION {echo}"));
            turns.push(format!("RESULT_DATA {result}"));
        }
        Err(self.terminal_error(gave_up))
    }

    fn terminal_error(&self, gave_up: bool) -> AttemptError {
        if self.book.usable_count() == 0 && self.book.excluded_count() >= 1 {
            AttemptError::Terminal(FailureCode::NoSafeSources)
        } else if self.book.hits_seen == 0 && self.book.unsafe_dropped == 0 && self.searches >= 1 {
            AttemptError::Terminal(FailureCode::NoResults)
        } else if gave_up {
            AttemptError::CompletionUnmet
        } else {
            AttemptError::Terminal(FailureCode::BudgetExhausted)
        }
    }

    async fn call_tool(&self, tool: &str, arguments: Value) -> Result<String, AttemptError> {
        self.guard_run()?;
        let timeout = self.remaining().min(TOOL_TIMEOUT);
        let raw = self
            .env
            .tools
            .run(tool, &arguments.to_string(), timeout, self.env.cancellation)
            .await;
        self.guard_run()?;
        Ok(raw)
    }

    async fn search(&mut self, query: &str) -> Result<Value, AttemptError> {
        if query.is_empty() {
            return Ok(json!({"error": "invalid_query"}));
        }
        if self.searches >= MAX_SEARCHES {
            return Ok(json!({"error": "search_limit_reached"}));
        }
        self.searches += 1;
        let raw = self
            .call_tool("web_search", json!({"query": query, "limit": 5}))
            .await?;
        let Some(found) = parse_search(&raw) else {
            return Ok(json!({"error": "tool_failed"}));
        };
        self.book.unsafe_dropped += found.blocked_results;
        let task_id = self.env.task_id;
        let writer = self.env.writer;
        let mut visible: Vec<(String, RawHit)> = Vec::new();
        let mut pending: Vec<(String, String, RawHit)> = Vec::new();
        for hit in found.hits {
            let (Some(url), Some(host)) = (normalize_url(&hit.url), host_of(&hit.url)) else {
                continue;
            };
            if let Some(existing) = self.book.entry(SourceKind::SearchHit, &url) {
                if existing.status == SourceStatus::Usable
                    && !visible.iter().any(|(seen, _)| *seen == url)
                {
                    visible.push((url, hit));
                }
                continue;
            }
            if pending.iter().any(|(seen, _, _)| *seen == url) {
                continue;
            }
            self.book.hits_seen += 1;
            let blocked = match url_hash(&url) {
                Some(hash) => audit::is_blocklisted(writer, &hash).map_err(ledger)?,
                None => true,
            };
            let reason = if found.withheld {
                Some("search_guard")
            } else if blocked {
                Some("blocklisted")
            } else if self.book.host_excluded(&host) {
                Some("host_excluded")
            } else if !citable_url(&url) {
                Some("url_not_citable")
            } else {
                None
            };
            let status = if reason.is_some() {
                SourceStatus::Excluded
            } else {
                SourceStatus::Recorded
            };
            let mut record = SourceRecord::new(&url, &host, SourceKind::SearchHit, status);
            record.fail_reason = reason;
            audit::upsert_source(writer, task_id, &record).map_err(ledger)?;
            self.book.insert(
                SourceKind::SearchHit,
                &url,
                SourceEntry {
                    status,
                    host: host.clone(),
                    date: hit.date.clone(),
                },
            );
            if reason.is_none() {
                pending.push((url, host, hit));
            }
        }
        if !pending.is_empty() {
            let items: Vec<String> = pending
                .iter()
                .map(|(_, _, hit)| {
                    format!(
                        "{} | {}",
                        clip(&hit.title, MAX_TITLE_CHARS),
                        clip(&hit.snippet, MAX_SNIPPET_CHARS)
                    )
                })
                .collect();
            let verdict = self.checker.check_batch(&items).await;
            self.guard_run()?;
            for (index, (url, host, hit)) in pending.into_iter().enumerate() {
                if verdict.flagged.contains(&index) {
                    let mut record =
                        SourceRecord::new(&url, &host, SourceKind::SearchHit, SourceStatus::Failed);
                    record.check_suspected = Some(true);
                    record.check_categories = &verdict.categories;
                    record.fail_reason = Some(if verdict.failed_closed {
                        "checker_unavailable"
                    } else {
                        "checker_flag"
                    });
                    audit::upsert_source(writer, task_id, &record).map_err(ledger)?;
                    self.book
                        .set_status(SourceKind::SearchHit, &url, SourceStatus::Failed);
                    self.book
                        .add_categories(verdict.categories.iter().map(String::as_str));
                } else {
                    let mut record =
                        SourceRecord::new(&url, &host, SourceKind::SearchHit, SourceStatus::Usable);
                    record.check_suspected = Some(false);
                    audit::upsert_source(writer, task_id, &record).map_err(ledger)?;
                    self.book
                        .set_status(SourceKind::SearchHit, &url, SourceStatus::Usable);
                    visible.push((url, hit));
                }
            }
        }
        Ok(hits_envelope(&visible))
    }

    async fn fetch(&mut self, url: &str, query: &str) -> Result<Value, AttemptError> {
        if !self.book.fetch_allowed(url) {
            return Ok(json!({"error": "url_not_recorded"}));
        }
        if self.fetches >= MAX_FETCHES {
            return Ok(json!({"error": "fetch_limit_reached"}));
        }
        self.fetches += 1;
        let mut arguments = json!({"url": url, "maxCharacters": 3000});
        if !query.is_empty() {
            arguments["query"] = json!(query);
        }
        let raw = self.call_tool("fetch_content", arguments).await?;
        let Some(page) = parse_fetch(&raw) else {
            return Ok(json!({"error": "tool_failed"}));
        };
        let plugin_flag = page.plugin_flagged();
        // Always run the checker on whatever text arrived, even when the plugin already flagged.
        let verdict = if page.text.trim().is_empty() {
            CheckVerdict {
                suspected: false,
                categories: Vec::new(),
                excerpt: String::new(),
                failed_closed: false,
            }
        } else {
            let verdict = self.checker.check_text(&page.text).await;
            self.guard_run()?;
            verdict
        };
        if plugin_flag || verdict.suspected {
            self.fail_page(url, &page, plugin_flag, &verdict)?;
            return Ok(json!({"url": url, "status": "failed", "reason": "unsafe_source"}));
        }
        if page.text.trim().is_empty() {
            return Ok(json!({"url": url, "status": "unavailable"}));
        }
        let host = host_of(url).unwrap_or_default();
        let mut record = SourceRecord::new(url, &host, SourceKind::Fetched, SourceStatus::Usable);
        record.guard_decision = Some(&page.decision);
        record.warning_categories = &page.warning_categories;
        record.check_suspected = Some(false);
        audit::upsert_source(self.env.writer, self.env.task_id, &record).map_err(ledger)?;
        self.book.insert(
            SourceKind::Fetched,
            url,
            SourceEntry {
                status: SourceStatus::Usable,
                host,
                date: page.fetched_at.clone(),
            },
        );
        Ok(page_envelope(url, &page))
    }

    /// Records a flagged page: failed everywhere, permanently blocklisted, host counted. The
    /// page text is dropped here and never reaches a transcript.
    fn fail_page(
        &mut self,
        url: &str,
        page: &FetchEnvelope,
        plugin_flag: bool,
        verdict: &CheckVerdict,
    ) -> Result<(), AttemptError> {
        let writer = self.env.writer;
        let task_id = self.env.task_id;
        let host = host_of(url).unwrap_or_default();
        // A checker that could not run (timeout, unparsable output) says nothing about the page:
        // the source fails for this task only and is neither blocklisted nor counted against its host.
        let persistent = plugin_flag || !verdict.failed_closed;
        let reason = match (plugin_flag, verdict.suspected) {
            (true, true) => "plugin_and_checker_flag",
            (true, false) => "plugin_flag",
            _ if verdict.failed_closed => "checker_unavailable",
            _ => "checker_flag",
        };
        let mut record = SourceRecord::new(url, &host, SourceKind::Fetched, SourceStatus::Failed);
        record.guard_decision = Some(&page.decision);
        record.warning_categories = &page.warning_categories;
        record.check_suspected = Some(verdict.suspected);
        record.check_categories = &verdict.categories;
        record.check_excerpt = verdict.suspected.then_some(verdict.excerpt.as_str());
        record.fail_reason = Some(reason);
        audit::upsert_source(writer, task_id, &record).map_err(ledger)?;
        audit::mark_url_failed(writer, task_id, url, reason).map_err(ledger)?;
        self.book.insert(
            SourceKind::Fetched,
            url,
            SourceEntry {
                status: SourceStatus::Failed,
                host: host.clone(),
                date: None,
            },
        );
        for kind in [SourceKind::SearchHit, SourceKind::UserSupplied] {
            self.book.set_status(kind, url, SourceStatus::Failed);
        }
        self.supplied.retain(|supplied| supplied != url);
        self.book.add_categories(
            page.warning_categories
                .iter()
                .map(String::as_str)
                .filter(|name| *name != "low_trust_attribute")
                .chain(verdict.categories.iter().map(String::as_str)),
        );
        // Permanent blocklist for the requested URL and the final URL after redirects.
        let mut blocked: Vec<(String, String)> = Vec::new();
        if let Some(hash) = url_hash(url) {
            blocked.push((hash, host.clone()));
        }
        if let Some(final_url) = page.final_url.as_deref() {
            if let (Some(hash), Some(final_host)) = (url_hash(final_url), host_of(final_url)) {
                if blocked.iter().all(|(existing, _)| *existing != hash) {
                    blocked.push((hash, final_host));
                }
            }
        }
        for (hash, blocked_host) in blocked.iter().filter(|_| persistent) {
            audit::insert_blocklist(writer, hash, blocked_host, reason).map_err(ledger)?;
            self.book.flag_host(blocked_host);
        }
        Ok(())
    }
}
