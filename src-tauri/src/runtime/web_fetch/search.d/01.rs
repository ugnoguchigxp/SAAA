const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
const MAX_CANDIDATES: usize = 1_000;
const DDG_HTML_ENDPOINT: &str = "https://html.duckduckgo.com/html/";
const DDG_LITE_ENDPOINT: &str = "https://lite.duckduckgo.com/lite/";
const DDG_WEB_ENDPOINT: &str = "https://duckduckgo.com/";
const BRAVE_ENDPOINT: &str = "https://api.search.brave.com/res/v1/web/search";
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";
/// Raw provider hit before guard filtering.
#[derive(Debug, Clone)]
pub struct RawHit {
    pub provider: &'static str,
    pub title: String,
    pub url: String,
    pub snippet: String,
}
/// Guard-filtered, model-facing hit.
#[derive(Debug, Clone)]
pub struct SearchHit {
    pub provider: String,
    pub rank: u32,
    pub title: String,
    pub url: String,
    pub snippet: String,
}
#[derive(Debug, Clone)]
pub struct SearchOutcome {
    pub hits: Vec<SearchHit>,
    pub blocked_result_count: usize,
    pub warning_categories: Vec<String>,
    pub decision: &'static str,
}
#[async_trait]
pub trait SearchProvider: Send + Sync {
    async fn search(
        &self,
        input: SearchInput,
        deadline: Duration,
        cancellation: WebFetchCancel,
    ) -> Result<SearchOutcome, WebFetchFailure>;
}
pub struct RustSearchProvider {
    client: reqwest::Client,
}
impl RustSearchProvider {
    pub fn new() -> Result<Self, WebFetchFailure> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(25))
            .user_agent(USER_AGENT)
            // Search endpoints are fixed trust boundaries. Do not forward
            // method bodies or the Brave credential across redirects.
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| WebFetchFailure::unavailable())?;
        Ok(Self { client })
    }

    async fn get_candidates(
        &self,
        input: &SearchInput,
        deadline: Duration,
        cancellation: &WebFetchCancel,
    ) -> Result<Vec<RawHit>, WebFetchFailure> {
        // Same order as the TypeScript provider: Web (bootstrap + preload
        // script) first, then HTML POST, then Lite POST, then Brave.
        // Fall through only on challenge / parse-change / retryable
        // upstream errors.
        let mut observed_challenge: Option<WebFetchFailure> = None;
        match self.ddg_web(input, deadline, cancellation).await {
            Ok(hits) => return Ok(hits),
            Err(failure) => {
                if is_challenge(&failure) {
                    observed_challenge = Some(failure.clone());
                } else if !can_try_alternate(&failure) {
                    return Err(failure);
                }
            }
        }
        let mut last_failure = WebFetchFailure::unavailable();
        for (endpoint, parser) in [
            (DDG_HTML_ENDPOINT, parse_ddg_html as fn(&str) -> _),
            (DDG_LITE_ENDPOINT, parse_ddg_lite as fn(&str) -> _),
        ] {
            if cancellation.is_cancelled() {
                return Err(WebFetchFailure::cancelled());
            }
            let attempt = match self
                .ddg_post(endpoint, &input.query, deadline, cancellation)
                .await
            {
                Ok(body) => parser(&body),
                Err(failure) => Err(failure),
            };
            match attempt {
                Ok(hits) => return Ok(hits),
                Err(failure) => {
                    if is_challenge(&failure) {
                        observed_challenge = Some(failure.clone());
                    } else if !can_try_alternate(&failure) {
                        return Err(failure);
                    }
                    last_failure = failure;
                }
            }
        }
        if cancellation.is_cancelled() {
            return Err(WebFetchFailure::cancelled());
        }
        // Brave fallback is credential-gated and only follows a retryable
        // DuckDuckGo failure, matching the TypeScript provider.
        if last_failure.retryable
            && std::env::var("BRAVE_SEARCH_API_KEY")
                .map(|key| !key.trim().is_empty())
                .unwrap_or(false)
        {
            return self.brave_search(input, deadline, cancellation).await;
        }
        Err(prefer_challenge(last_failure, observed_challenge))
    }

    /// DuckDuckGo Web path: bootstrap GET → VQD + `/d.js` preload URL →
    /// result script GET → `DDG.pageLayout.load("d", …)` payload parse.
    async fn ddg_web(
        &self,
        input: &SearchInput,
        deadline: Duration,
        cancellation: &WebFetchCancel,
    ) -> Result<Vec<RawHit>, WebFetchFailure> {
        let request = self
            .client
            .get(DDG_WEB_ENDPOINT)
            .query(&[("q", input.query.as_str()), ("ia", "web"), ("kp", "-1")])
            .header("Accept", "text/html,application/xhtml+xml")
            .header("Accept-Language", "en-US,en;q=0.9")
            .timeout(deadline.min(Duration::from_secs(20)));
        let bootstrap = send_bounded(request, cancellation, ExpectedBody::Html).await?;
        assert_not_challenge(&bootstrap)?;
        let preload = extract_preload_url(&bootstrap)?;
        let request = self
            .client
            .get(preload)
            .header(
                "Accept",
                "application/javascript,text/javascript;q=0.9,*/*;q=0.1",
            )
            .header("Accept-Language", "en-US,en;q=0.9")
            .timeout(deadline.min(Duration::from_secs(20)));
        let script = send_bounded(request, cancellation, ExpectedBody::Script).await?;
        parse_ddg_web(&script, input.limit)
    }

    async fn ddg_post(
        &self,
        endpoint: &str,
        query: &str,
        deadline: Duration,
        cancellation: &WebFetchCancel,
    ) -> Result<String, WebFetchFailure> {
        // Same form fields as the TypeScript provider (q + kp safe-search).
        let params = [("q", query), ("kp", "-1")];
        let request = self
            .client
            .post(endpoint)
            .form(&params)
            .header("Accept", "text/html,application/xhtml+xml")
            .header("Accept-Language", "en-US,en;q=0.9")
            .timeout(deadline.min(Duration::from_secs(20)));
        let body = send_bounded(request, cancellation, ExpectedBody::Html).await?;
        assert_not_challenge(&body)?;
        Ok(body)
    }

    async fn brave_search(
        &self,
        input: &SearchInput,
        deadline: Duration,
        cancellation: &WebFetchCancel,
    ) -> Result<Vec<RawHit>, WebFetchFailure> {
        let key = std::env::var("BRAVE_SEARCH_API_KEY").unwrap_or_default();
        if key.trim().is_empty() {
            // No credential: never send a request.
            return Err(WebFetchFailure::unavailable());
        }
        let count = input.limit.to_string();
        let request = self
            .client
            .get(BRAVE_ENDPOINT)
            .query(&[("q", input.query.as_str()), ("count", count.as_str())])
            .header("Accept", "application/json")
            .header("X-Subscription-Token", key)
            .timeout(deadline.min(Duration::from_secs(20)));
        let body = send_bounded(request, cancellation, ExpectedBody::Json).await?;
        parse_brave_json(&body)
    }
}
impl Default for RustSearchProvider {
    fn default() -> Self {
        Self::new().expect("search HTTP client builds")
    }
}
#[async_trait]
impl SearchProvider for RustSearchProvider {
    async fn search(
        &self,
        input: SearchInput,
        deadline: Duration,
        cancellation: WebFetchCancel,
    ) -> Result<SearchOutcome, WebFetchFailure> {
        if cancellation.is_cancelled() {
            return Err(WebFetchFailure::cancelled());
        }
        let outcome = tokio::select! {
            biased;
            _ = cancellation.cancelled() => return Err(WebFetchFailure::cancelled()),
            _ = tokio::time::sleep(deadline) => return Err(WebFetchFailure::timeout()),
            result = self.get_candidates(&input, deadline, &cancellation) => result?,
        };
        Ok(filter_and_project(outcome, &input))
    }
}
#[derive(Clone, Copy)]
enum ExpectedBody {
    Html,
    Script,
    Json,
}
impl ExpectedBody {
    fn accepts(self, value: &str) -> bool {
        let value = value.to_ascii_lowercase();
        match self {
            Self::Html => {
                value.starts_with("text/html") || value.starts_with("application/xhtml+xml")
            }
            Self::Script => {
                value.starts_with("application/javascript")
                    || value.starts_with("text/javascript")
                    || value.starts_with("application/x-javascript")
            }
            Self::Json => value.starts_with("application/json"),
        }
    }
}
async fn read_bounded(
    response: reqwest::Response,
    cancellation: &WebFetchCancel,
    expected: ExpectedBody,
) -> Result<String, WebFetchFailure> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
    {
        return Err(WebFetchFailure::new(
            "RESPONSE_TOO_LARGE",
            "The search response was too large.",
            true,
        ));
    }
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(WebFetchFailure::unavailable)?;
    if !expected.accepts(content_type) {
        return Err(WebFetchFailure::new(
            "PARSE_CHANGED",
            "The search provider changed its response format.",
            true,
        ));
    }
    let mut bytes = Vec::with_capacity(
        response
            .content_length()
            .unwrap_or(8_192)
            .min(MAX_RESPONSE_BYTES as u64) as usize,
    );
    let mut stream = response.bytes_stream();
    loop {
        let chunk = tokio::select! {
            biased;
            _ = cancellation.cancelled() => return Err(WebFetchFailure::cancelled()),
            chunk = stream.next() => chunk,
        };
        let Some(chunk) = chunk else { break };
        let chunk = chunk.map_err(|_| WebFetchFailure::unavailable())?;
        if bytes.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
            return Err(WebFetchFailure::new(
                "RESPONSE_TOO_LARGE",
                "The search response was too large.",
                true,
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
    String::from_utf8(bytes).map_err(|_| WebFetchFailure::unavailable())
}
/// Send a search HTTP request with cancellation, 429 mapping, and bounded
/// body read. Non-success statuses become retryable `unavailable`.
async fn send_bounded(
    request: reqwest::RequestBuilder,
    cancellation: &WebFetchCancel,
    expected: ExpectedBody,
) -> Result<String, WebFetchFailure> {
    let response = tokio::select! {
        biased;
        _ = cancellation.cancelled() => return Err(WebFetchFailure::cancelled()),
        result = request.send() => result.map_err(|_| WebFetchFailure::unavailable())?,
    };
    if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return Err(WebFetchFailure::new(
            "RATE_LIMITED",
            "The search provider is rate limited.",
            true,
        ));
    }
    if !response.status().is_success() {
        return Err(WebFetchFailure::new(
            "UPSTREAM_HTTP",
            "The search provider returned an unexpected status.",
            true,
        ));
    }
    read_bounded(response, cancellation, expected).await
}
fn is_challenge(failure: &WebFetchFailure) -> bool {
    failure.code == "BOT_CHALLENGE"
}
/// Mirror the TypeScript `canTryAlternate`: only challenge, parse-change,
/// and retryable upstream errors fall through to the next endpoint.
fn can_try_alternate(failure: &WebFetchFailure) -> bool {
    matches!(
        failure.code,
        "BOT_CHALLENGE" | "PARSE_CHANGED" | "UPSTREAM_HTTP" | "RATE_LIMITED"
    ) && failure.retryable
}
/// Prefer an observed challenge over a later generic failure, matching
/// `preferObservedChallenge`.
fn prefer_challenge(
    failure: WebFetchFailure,
    observed: Option<WebFetchFailure>,
) -> WebFetchFailure {
    match observed {
        Some(challenge)
            if failure.retryable
                && matches!(
                    failure.code,
                    "BOT_CHALLENGE" | "PARSE_CHANGED" | "UPSTREAM_HTTP" | "RATE_LIMITED"
                ) =>
        {
            challenge
        }
        _ => failure,
    }
}
/// Extract the `links.duckduckgo.com/d.js` preload URL from the bootstrap
/// page: VQD token + `<script src="…/d.js…">`, validated like TS.
fn extract_preload_url(html: &str) -> Result<String, WebFetchFailure> {
    let parse_changed = || {
        WebFetchFailure::new(
            "PARSE_CHANGED",
            "The search provider changed its response format.",
            true,
        )
    };
    let vqd = extract_vqd(html).ok_or_else(parse_changed)?;
    let _ = vqd;
    let mut cursor = 0;
    let bytes = html.as_bytes();
    while let Some(open) = find(bytes, cursor, b"<script") {
        let tag_end = match find(bytes, open, b">") {
            Some(end) if end - open < 16_384 => end,
            _ => {
                cursor = open + 7;
                continue;
            }
        };
        let tag = &html[open..tag_end];
        if let Some(src) = attr_value(tag, "src") {
            if src.contains("/d.js") && src.len() <= 16_384 {
                let url = url::Url::parse(&src)
                    .or_else(|_| url::Url::parse(&format!("https://duckduckgo.com{src}")))
                    .or_else(|_| url::Url::parse(&format!("https://duckduckgo.com/{src}")))
                    .map_err(|_| parse_changed())?;
                if url.scheme() == "https"
                    && url.host_str() == Some("links.duckduckgo.com")
                    && url.port().is_none()
                    && url.path() == "/d.js"
                    && url.username().is_empty()
                    && url.fragment().is_none()
                {
                    return Ok(url.to_string());
                }
            }
        }
        cursor = tag_end + 1;
    }
    Err(parse_changed())
}
/// Hand-scan `vqd="<digits>-<digits>[-<digits>]"` (VQD_PATTERN parity).
fn extract_vqd(html: &str) -> Option<String> {
    let bytes = html.as_bytes();
    let mut cursor = 0;
    while let Some(pos) = find(bytes, cursor, b"vqd") {
        let mut i = pos + 3;
        while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
            i += 1;
        }
        if bytes.get(i) != Some(&b'=') {
            cursor = pos + 3;
            continue;
        }
        i += 1;
        while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
            i += 1;
        }
        let quote = *bytes.get(i)?;
        if quote != b'"' && quote != b'\'' {
            cursor = pos + 3;
            continue;
        }
        i += 1;
        let start = i;
        while i < bytes.len() && bytes[i] != quote {
            i += 1;
        }
        if i >= bytes.len() || i - start > 64 {
            cursor = pos + 3;
            continue;
        }
        let token = &html[start..i];
        let parts: Vec<&str> = token.split('-').collect();
        if (parts.len() == 2 || parts.len() == 3)
            && parts
                .iter()
                .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
        {
            return Some(token.to_string());
        }
        cursor = pos + 3;
    }
    None
}
/// Parse the `DDG.pageLayout.load("d", […])` payload: entries with string
/// `t` (title), `a` (abstract), `u` (url); `"n"` entries skipped.
fn parse_ddg_web(body: &str, limit: usize) -> Result<Vec<RawHit>, WebFetchFailure> {
    let parse_changed = || {
        WebFetchFailure::new(
            "PARSE_CHANGED",
            "The search provider changed its response format.",
            true,
        )
    };
    assert_not_challenge(body)?;
    if body.len() > MAX_RESPONSE_BYTES {
        return Err(WebFetchFailure::new(
            "RESPONSE_TOO_LARGE",
            "The search response was too large.",
            true,
        ));
    }
    let marker = find(body.as_bytes(), 0, b"DDG.pageLayout.load(").ok_or_else(parse_changed)?;
    // Expect `"d"` (or `'d'`) as the first argument after the marker.
    let after = &body[marker..];
    let d_pos = after
        .find("\"d\"")
        .or_else(|| after.find("'d'"))
        .ok_or_else(parse_changed)?;
    let array_start = after[d_pos..]
        .find('[')
        .map(|pos| marker + d_pos + pos)
        .ok_or_else(parse_changed)?;
    let array_end = match_bracket(body, array_start).ok_or_else(parse_changed)?;
    let payload: serde_json::Value =
        serde_json::from_str(&body[array_start..=array_end]).map_err(|_| parse_changed())?;
    let entries = payload.as_array().ok_or_else(parse_changed)?;
    if entries.len() > MAX_CANDIDATES {
        return Err(WebFetchFailure::new(
            "RESPONSE_TOO_LARGE",
            "The search provider returned too many results.",
            true,
        ));
    }
    let mut hits = Vec::new();
    let mut seen = HashSet::new();
    for entry in entries {
        let Some(object) = entry.as_object() else {
            continue;
        };
        if object.contains_key("n") {
            continue;
        }
        let (Some(title), Some(snippet), Some(url)) = (
            object.get("t").and_then(|v| v.as_str()),
            object.get("a").and_then(|v| v.as_str()),
            object.get("u").and_then(|v| v.as_str()),
        ) else {
            continue;
        };
        let Some(normalized) = normalize_result_url(url) else {
            continue;
        };
        if !seen.insert(normalized.clone()) {
            continue;
        }
        let title = compact_text(&strip_tags(title), 200);
        let snippet = compact_text(&strip_tags(snippet), 500);
        if title.is_empty() {
            continue;
        }
        hits.push(RawHit {
            provider: "duckduckgo",
            title,
            url: normalized,
            snippet,
        });
        if hits.len() >= limit.max(1) {
            break;
        }
    }
    Ok(hits)
}
