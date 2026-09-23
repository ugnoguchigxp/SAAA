use super::*;
/// String-aware bracket matcher: returns the index of the `]` closing the
/// `[` at `open`. Bounded by construction (body <= 2 MiB).
pub(super) fn match_bracket(body: &str, open: usize) -> Option<usize> {
    let bytes = body.as_bytes();
    if bytes.get(open) != Some(&b'[') {
        return None;
    }
    let mut depth = 0usize;
    let mut in_string: Option<u8> = None;
    let mut escaped = false;
    let mut i = open;
    while i < bytes.len() {
        let byte = bytes[i];
        if let Some(quote) = in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == quote {
                in_string = None;
            }
        } else if byte == b'"' || byte == b'\'' {
            in_string = Some(byte);
        } else if byte == b'[' {
            depth += 1;
        } else if byte == b']' {
            depth -= 1;
            if depth == 0 {
                return Some(i);
            }
        }
        i += 1;
    }
    None
}
/// TS `normalizeResultUrl` parity: unwrap DDG redirect, require http(s),
/// reject userinfo, strip hash + trailing slash, drop `utm_*` params.
pub(super) fn normalize_result_url(raw: &str) -> Option<String> {
    let unwrapped = normalize_ddg_href(raw.to_string());
    let mut url = url::Url::parse(&unwrapped).ok()?;
    if url.scheme() != "http" && url.scheme() != "https" {
        return None;
    }
    if !url.username().is_empty() || url.password().is_some() {
        return None;
    }
    url.set_fragment(None);
    let path = url.path().to_string();
    if path.len() > 1 && path.ends_with('/') {
        url.set_path(path.trim_end_matches('/'));
    }
    let kept: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(key, _)| !key.to_ascii_lowercase().starts_with("utm_"))
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    url.set_query(None);
    if !kept.is_empty() {
        let query = kept
            .iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect::<Vec<_>>()
            .join("&");
        url.set_query(Some(&query));
    }
    Some(url.to_string())
}
pub(super) fn assert_not_challenge(body: &str) -> Result<(), WebFetchFailure> {
    // Cheap substring pre-filter before the regex set; keeps the common path
    // allocation-free of regex evaluation.
    let lower = body.to_ascii_lowercase();
    let challenge = lower.contains("anomaly-modal")
        || lower.contains("bot_challenge")
        || lower.contains("challenge-form")
        || lower.contains("unfortunately, bots use duckduckgo too")
        || lower.contains("please complete the following challenge");
    if challenge {
        return Err(WebFetchFailure::new(
            "BOT_CHALLENGE",
            "The search provider returned a bot challenge.",
            true,
        ));
    }
    Ok(())
}
/// Extract `(href, anchor_text)` pairs from result-class anchors using a
/// bounded hand scanner. No regex HTML parsing: the scanner only recognizes
/// `<a ... class="...result__a..." href="...">text</a>` shapes and decodes a
/// small entity set, with all lengths capped before allocation.
pub(super) fn extract_result_anchors(html: &str, limit: usize) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let bytes = html.as_bytes();
    let mut cursor = 0;
    while out.len() < limit && cursor < bytes.len() {
        let Some(open) = find(bytes, cursor, b"<a") else {
            break;
        };
        let tag_end = match find(bytes, open, b">") {
            Some(end) if end - open < 4_096 => end,
            _ => {
                cursor = open + 2;
                continue;
            }
        };
        let tag = &html[open..tag_end];
        if !is_result_anchor(tag) {
            cursor = tag_end + 1;
            continue;
        }
        let Some(href) = attr_value(tag, "href").map(normalize_ddg_href) else {
            cursor = tag_end + 1;
            continue;
        };
        let text_start = tag_end + 1;
        let Some(close) = find(bytes, text_start, b"</a>") else {
            cursor = tag_end + 1;
            continue;
        };
        if close - text_start > 8_192 {
            cursor = tag_end + 1;
            continue;
        }
        let text = strip_tags(&html[text_start..close]);
        // Snippet: first following `result__snippet` block within 8 KiB.
        let window_end = floor_char_boundary(html, (close + 8_192).min(html.len()));
        let snippet_window = &html[close..window_end];
        let snippet = snippet_after(snippet_window);
        out.push((href, format!("{text}\u{1f}{snippet}")));
        cursor = close + 4;
    }
    out
}
pub(super) fn find(haystack: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    haystack[from..]
        .windows(needle.len())
        .position(|window| window == needle)
        .map(|pos| from + pos)
}
pub(super) fn floor_char_boundary(value: &str, mut end: usize) -> usize {
    end = end.min(value.len());
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    end
}
pub(super) fn is_result_anchor(tag: &str) -> bool {
    let lower = tag.to_ascii_lowercase();
    lower.contains("result__a") || lower.contains("result-link")
}
pub(super) fn attr_value(tag: &str, name: &str) -> Option<String> {
    // Matches name="..." or name='...' case-insensitively.
    let lower = tag.to_ascii_lowercase();
    let pattern_dq = format!("{name}=\"");
    let pattern_sq = format!("{name}='");
    for (pattern, quote) in [(&pattern_dq, b'"'), (&pattern_sq, b'\'')] {
        if let Some(start) = lower.find(pattern.as_str()) {
            let value_start = start + pattern.len();
            let rest = &tag[value_start..];
            if let Some(end) = rest.bytes().position(|b| b == quote) {
                if end <= 2_048 {
                    return Some(rest[..end].to_string());
                }
                return None;
            }
        }
    }
    None
}
pub(super) fn normalize_ddg_href(raw: String) -> String {
    // DuckDuckGo wraps results as /l/?uddg=<url-encoded target>.
    if let Some(encoded) = raw.split("uddg=").nth(1) {
        let encoded = encoded.split('&').next().unwrap_or(encoded);
        if let Ok(decoded) = urlencoding_decode(encoded) {
            return decoded;
        }
    }
    raw
}
pub(super) fn urlencoding_decode(value: &str) -> Result<String, ()> {
    fn hex(value: u8) -> Option<u8> {
        match value {
            b'0'..=b'9' => Some(value - b'0'),
            b'a'..=b'f' => Some(value - b'a' + 10),
            b'A'..=b'F' => Some(value - b'A' + 10),
            _ => None,
        }
    }
    let input = value.as_bytes();
    let mut out = Vec::with_capacity(input.len());
    let mut index = 0;
    while index < input.len() {
        match input[index] {
            b'%' => {
                let hi = hex(*input.get(index + 1).ok_or(())?).ok_or(())?;
                let lo = hex(*input.get(index + 2).ok_or(())?).ok_or(())?;
                out.push((hi << 4) | lo);
                index += 3;
            }
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8(out).map_err(|_| ())
}
pub(super) fn strip_tags(html: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for char in html.chars() {
        match char {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(char),
            _ => {}
        }
    }
    decode_entities(&out)
}
pub(super) fn decode_entities(value: &str) -> String {
    value
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#x27;", "'")
        .replace("&#39;", "'")
}
pub(super) fn snippet_after(window: &str) -> String {
    // Look for class="result__snippet" then capture text until the next tag
    // close. Bounded by construction (window <= 8 KiB).
    let lower = window.to_ascii_lowercase();
    let Some(class_pos) = lower.find("result__snippet") else {
        return String::new();
    };
    let after = &window[class_pos..];
    let Some(tag_end) = after.find('>') else {
        return String::new();
    };
    let text_start = &after[tag_end + 1..];
    let end = text_start.find('<').unwrap_or(text_start.len());
    strip_tags(&text_start[..floor_char_boundary(text_start, end.min(2_000))])
}
pub(super) fn parse_ddg_html(html: &str) -> Result<Vec<RawHit>, WebFetchFailure> {
    parse_ddg_anchors(html, "duckduckgo")
}
pub(super) fn parse_ddg_lite(html: &str) -> Result<Vec<RawHit>, WebFetchFailure> {
    parse_ddg_anchors(html, "duckduckgo")
}
pub(super) fn parse_ddg_anchors(
    html: &str,
    provider: &'static str,
) -> Result<Vec<RawHit>, WebFetchFailure> {
    if html.len() > MAX_RESPONSE_BYTES {
        return Err(WebFetchFailure::new(
            "RESPONSE_TOO_LARGE",
            "The search response was too large.",
            true,
        ));
    }
    let anchors = extract_result_anchors(html, MAX_CANDIDATES + 1);
    if anchors.len() > MAX_CANDIDATES {
        return Err(WebFetchFailure::new(
            "RESPONSE_TOO_LARGE",
            "The search provider returned too many results.",
            true,
        ));
    }
    Ok(anchors
        .into_iter()
        .filter_map(|(href, packed)| {
            let mut parts = packed.split('\u{1f}');
            let title = compact_text(parts.next().unwrap_or(""), 200);
            let snippet = compact_text(parts.next().unwrap_or(""), 500);
            if title.is_empty() || href.is_empty() {
                return None;
            }
            // TS parity: normalize (unwrap DDG redirect, strip tracking)
            // before returning; invalid entries are skipped.
            let url = normalize_result_url(&href)?;
            Some(RawHit {
                provider,
                title,
                url,
                snippet,
            })
        })
        .collect())
}
pub(super) fn parse_brave_json(body: &str) -> Result<Vec<RawHit>, WebFetchFailure> {
    if body.len() > MAX_RESPONSE_BYTES {
        return Err(WebFetchFailure::new(
            "RESPONSE_TOO_LARGE",
            "The search response was too large.",
            true,
        ));
    }
    let value: serde_json::Value =
        serde_json::from_str(body).map_err(|_| WebFetchFailure::unavailable())?;
    let results = value
        .pointer("/web/results")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    if results.len() > MAX_CANDIDATES {
        return Err(WebFetchFailure::new(
            "RESPONSE_TOO_LARGE",
            "The search provider returned too many results.",
            true,
        ));
    }
    Ok(results
        .iter()
        .filter_map(|item| {
            let title = compact_text(
                item.get("title").and_then(|v| v.as_str()).unwrap_or(""),
                200,
            );
            let url = item.get("url").and_then(|v| v.as_str()).unwrap_or("");
            let snippet = compact_text(
                item.get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or(""),
                500,
            );
            if title.is_empty() || url.is_empty() {
                return None;
            }
            Some(RawHit {
                provider: "brave",
                title,
                url: url.to_string(),
                snippet,
            })
        })
        .collect())
}
/// Reject non-HTTP(S), userinfo, literal IP, localhost/local names.
/// Mirrors the sidecar URL policy; the WebView plugin DNS / public-network
/// check runs again at content-fetch time.
pub fn is_allowed_result_url(raw: &str) -> bool {
    if raw.is_empty() || raw.len() > 2048 {
        return false;
    }
    let Ok(url) = url::Url::parse(raw) else {
        return false;
    };
    if url.scheme() != "http" && url.scheme() != "https" {
        return false;
    }
    if !url.username().is_empty() || url.password().is_some() {
        return false;
    }
    let Some(host) = url.host_str() else {
        return false;
    };
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty() || host == "localhost" {
        return false;
    }
    if host.parse::<std::net::IpAddr>().is_ok() {
        return false;
    }
    if host.ends_with(".localhost") || host.ends_with(".local") || host.ends_with(".internal") {
        return false;
    }
    if host == "local" || host == "internal" {
        return false;
    }
    // Private / link-local / reserved literal ranges that slipped through as
    // decimal or unknown forms are handled by the plugin DNS gate; here we
    // additionally reject obvious RFC1918 dot-decimal spellings.
    if host
        .split('.')
        .all(|part| part.bytes().all(|b| b.is_ascii_digit()) && !part.is_empty())
    {
        return false;
    }
    true
}
pub(super) fn filter_and_project(candidates: Vec<RawHit>, input: &SearchInput) -> SearchOutcome {
    let mut seen: HashSet<String> = HashSet::new();
    let mut hits = Vec::new();
    let mut blocked = 0usize;
    let mut warnings: HashSet<String> = HashSet::new();
    for candidate in candidates {
        if hits.len() >= input.limit {
            break;
        }
        if !is_allowed_result_url(&candidate.url) {
            blocked += 1;
            continue;
        }
        let normalized = candidate.url.trim_end_matches('/').to_ascii_lowercase();
        if !seen.insert(normalized) {
            continue;
        }
        // Guard over provider + title + snippet + URL, bounded plain text.
        let inspected_text = format!(
            "{}\n{}\n{}\n{}",
            candidate.provider, candidate.title, candidate.snippet, candidate.url
        );
        let outcome = tauri_plugin_llm_fetch::inspect_plain_text_bounded(&inspected_text, 4_000);
        for category in &outcome.warning_categories {
            warnings.insert(
                serde_json::to_value(category)
                    .and_then(serde_json::from_value::<String>)
                    .unwrap_or_else(|_| "unknown".to_string()),
            );
        }
        use tauri_plugin_llm_fetch::GuardDecision as Decision;
        match outcome.decision {
            Decision::Deny | Decision::RequireApproval => {
                blocked += 1;
                continue;
            }
            Decision::Allow | Decision::AllowWithWarning => {}
        }
        hits.push(SearchHit {
            provider: compact_text(candidate.provider, 100),
            rank: (hits.len() + 1) as u32,
            title: compact_text(&candidate.title, 200),
            url: candidate.url,
            snippet: compact_text(&candidate.snippet, 500),
        });
    }
    let mut warning_categories: Vec<String> = warnings.into_iter().collect();
    warning_categories.sort();
    let decision = if blocked > 0 && hits.is_empty() {
        "deny"
    } else if blocked > 0 || !warning_categories.is_empty() {
        "allow_with_warning"
    } else {
        "allow"
    };
    SearchOutcome {
        hits,
        blocked_result_count: blocked,
        warning_categories,
        decision,
    }
}
/// Render the compact `web_search_result` JSON (TS parity: hits with
/// trust/tainted, `blockedResultCount`, top-level untrusted security).
pub fn render_compact(outcome: &SearchOutcome) -> String {
    serde_json::json!({
        "type": "web_search_result",
        "security": {
            "trust": "untrusted",
            "tainted": true,
            "decision": outcome.decision,
            "warningCategories": outcome.warning_categories,
        },
        "hits": outcome.hits.iter().map(|hit| serde_json::json!({
            "trust": "untrusted",
            "tainted": true,
            "provider": hit.provider,
            "rank": hit.rank,
            "title": hit.title,
            "url": hit.url,
            "snippet": hit.snippet,
        })).collect::<Vec<_>>(),
        "blockedResultCount": outcome.blocked_result_count,
    })
    .to_string()
}
