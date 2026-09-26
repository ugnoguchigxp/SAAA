//! Bounded HTML-first retrieval. No subresources or page scripts are requested.
use std::{net::IpAddr, sync::OnceLock, time::Duration};

use futures_util::StreamExt;
use url::Url;

#[path = "static_content/project.rs"]
mod project;

pub(super) fn project_visible_text(
    text: &str,
    query: Option<&str>,
    limit: usize,
) -> (String, bool, bool) {
    let projection = project::plain(text, query, limit);
    (projection.text, projection.relevant, projection.truncated)
}

use super::{
    content::FetchContentResult,
    contracts::{FetchContentInput, WebFetchFailure},
};

const MAX_HTML_BYTES: usize = 2 * 1024 * 1024;
const BLOCKED_V4: &[&str] = &[
    "0.0.0.0/8",
    "10.0.0.0/8",
    "100.64.0.0/10",
    "127.0.0.0/8",
    "169.254.0.0/16",
    "172.16.0.0/12",
    "192.0.0.0/24",
    "192.0.2.0/24",
    "192.88.99.0/24",
    "192.168.0.0/16",
    "198.18.0.0/15",
    "198.51.100.0/24",
    "203.0.113.0/24",
    "224.0.0.0/4",
    "240.0.0.0/4",
];
const BLOCKED_V6: &[&str] = &[
    "::/128",
    "::1/128",
    "::/96",
    "::ffff:0:0/96",
    "64:ff9b::/96",
    "64:ff9b:1::/48",
    "100::/64",
    "2001::/23",
    "2001:db8::/32",
    "2002::/16",
    "3fff::/20",
    "5f00::/16",
    "fc00::/7",
    "fe80::/10",
    "fec0::/10",
    "ff00::/8",
];

fn public_url(raw: &str) -> Result<Url, WebFetchFailure> {
    let mut url = Url::parse(raw).map_err(|_| WebFetchFailure::unsafe_url())?;
    let host = url.host_str().ok_or_else(WebFetchFailure::unsafe_url)?;
    if raw.len() > 2_048
        || !matches!(url.scheme(), "http" | "https")
        || url.username() != ""
        || url.password().is_some()
        || url
            .port()
            .is_some_and(|port| !matches!((url.scheme(), port), ("http", 80) | ("https", 443)))
        || matches!(url.host(), Some(url::Host::Ipv4(_) | url::Host::Ipv6(_)))
        || host == "localhost"
        || host.ends_with(".localhost")
        || host.ends_with(".local")
        || host.ends_with(".internal")
        || host.ends_with(".home.arpa")
        || host == "home.arpa"
    {
        return Err(WebFetchFailure::unsafe_url());
    }
    url.set_fragment(None);
    Ok(url)
}

fn decode_page(bytes: &[u8], content_type: &str) -> String {
    let declared = content_type.split(';').skip(1).find_map(|part| {
        let (key, value) = part.trim().split_once('=')?;
        key.trim()
            .eq_ignore_ascii_case("charset")
            .then(|| value.trim().trim_matches(['\'', '"']))
    });
    let meta = String::from_utf8_lossy(&bytes[..bytes.len().min(4_096)]);
    static META_CHARSET: OnceLock<regex::Regex> = OnceLock::new();
    let meta_declared = META_CHARSET
        .get_or_init(|| {
            regex::Regex::new(r#"(?i)charset\s*=\s*['\"]?([a-z0-9_-]+)"#)
                .expect("static charset pattern")
        })
        .captures(&meta)
        .and_then(|captures| captures.get(1).map(|label| label.as_str().to_string()));
    let encoding = declared
        .and_then(|label| encoding_rs::Encoding::for_label(label.as_bytes()))
        .or_else(|| {
            meta_declared
                .as_deref()
                .and_then(|label| encoding_rs::Encoding::for_label(label.as_bytes()))
        })
        .unwrap_or(encoding_rs::UTF_8);
    let (decoded, _, _) = encoding.decode(bytes);
    decoded.into_owned()
}

fn looks_like_challenge(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    [
        "verify you are human",
        "checking your browser",
        "enable javascript",
        "captcha",
        "access denied",
        "bot detection",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
}

fn blocked_ip(ip: IpAddr) -> bool {
    let ranges = if ip.is_ipv4() { BLOCKED_V4 } else { BLOCKED_V6 };
    ranges.iter().any(|range| {
        range
            .parse::<ipnet::IpNet>()
            .is_ok_and(|network| network.contains(&ip))
    })
}

async fn pinned_client(url: &Url, timeout: Duration) -> Result<reqwest::Client, WebFetchFailure> {
    let host = url.host_str().ok_or_else(WebFetchFailure::unsafe_url)?;
    let port = url
        .port_or_known_default()
        .ok_or_else(WebFetchFailure::unsafe_url)?;
    let addresses = tokio::time::timeout(timeout, tokio::net::lookup_host((host, port)))
        .await
        .map_err(|_| WebFetchFailure::timeout())?
        .map_err(|_| {
            WebFetchFailure::new(
                "DNS_FAILURE",
                "The page hostname could not be resolved.",
                true,
            )
        })?
        .collect::<Vec<_>>();
    if addresses.is_empty()
        || addresses.len() > 64
        || addresses.iter().any(|address| blocked_ip(address.ip()))
    {
        return Err(WebFetchFailure::unsafe_url());
    }
    let mut builder = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(timeout)
        .no_proxy()
        .user_agent("SAAA/1.0 (content retrieval)");
    for address in addresses {
        builder = builder.resolve(host, address);
    }
    builder.build().map_err(|_| WebFetchFailure::unavailable())
}

pub async fn fetch(
    request: &FetchContentInput,
    timeout: Duration,
) -> Result<Option<FetchContentResult>, WebFetchFailure> {
    let mut url = public_url(&request.url)?;
    for _ in 0..=3 {
        let client = pinned_client(&url, timeout).await?;
        let response = client
            .get(url.clone())
            .header(
                reqwest::header::ACCEPT,
                "text/html,application/xhtml+xml;q=0.9,text/plain;q=0.8",
            )
            .send()
            .await
            .map_err(|_| WebFetchFailure::unavailable())?;
        if response.status().is_redirection() {
            let location = response
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|value| value.to_str().ok())
                .ok_or_else(WebFetchFailure::unavailable)?;
            url = public_url(
                url.join(location)
                    .map_err(|_| WebFetchFailure::unsafe_url())?
                    .as_str(),
            )?;
            continue;
        }
        if !response.status().is_success() {
            return Ok(None);
        }
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
            .to_ascii_lowercase();
        if !(content_type.starts_with("text/html")
            || content_type.starts_with("application/xhtml+xml")
            || content_type.starts_with("text/plain"))
        {
            return Ok(None);
        }
        if response
            .content_length()
            .is_some_and(|length| length as usize > MAX_HTML_BYTES)
        {
            return Ok(None);
        }
        let mut bytes = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| WebFetchFailure::unavailable())?;
            if bytes.len() + chunk.len() > MAX_HTML_BYTES {
                return Ok(None);
            }
            bytes.extend_from_slice(&chunk);
        }
        let decoded = decode_page(&bytes, &content_type);
        let is_plain = content_type.starts_with("text/plain");
        let query = request.query.clone();
        let max_characters = request.max_characters;
        let projection = tokio::task::spawn_blocking(move || {
            if is_plain {
                project::plain(&decoded, query.as_deref(), max_characters)
            } else {
                project::html(&decoded, query.as_deref(), max_characters)
            }
        })
        .await
        .map_err(|_| WebFetchFailure::unavailable())?;
        // A title without body text may be a JS shell. Brief, complete pages are valid.
        if !projection.has_body
            || looks_like_challenge(&projection.text)
            || (request.query.is_some() && !projection.relevant)
        {
            return Ok(None);
        }
        let text = projection.text;
        let guard = tauri_plugin_llm_fetch::inspect_plain_text_bounded(
            &text,
            request.max_characters.max(1_000),
        );
        use tauri_plugin_llm_fetch::GuardDecision;
        if matches!(
            guard.decision,
            GuardDecision::Deny | GuardDecision::RequireApproval
        ) {
            return Ok(None);
        }
        let warning_categories = guard
            .warning_categories
            .iter()
            .map(|category| {
                serde_json::to_value(category)
                    .and_then(serde_json::from_value::<String>)
                    .unwrap_or_else(|_| "unknown".into())
            })
            .collect();
        return Ok(Some(FetchContentResult {
            final_url: url.to_string(),
            text,
            fetched_at: chrono::Utc::now().to_rfc3339(),
            truncated: projection.truncated,
            decision: if matches!(guard.decision, GuardDecision::AllowWithWarning) {
                "allow_with_warning"
            } else {
                "allow"
            },
            warning_categories,
            retrieval_status: if request.query.is_some() {
                "relevant"
            } else {
                "partial"
            },
            retrieval_method: "html",
        }));
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_private_destinations() {
        assert!(public_url("https://127.0.0.1/").is_err());
        assert!(public_url("https://[::1]/").is_err());
        assert!(public_url("http://example.com/").is_ok());
        assert!(public_url("http://example.com:8080/").is_err());
        assert!(public_url("https://example.com:443/x").is_ok());
        assert!(blocked_ip("10.0.0.1".parse().unwrap()));
    }

    #[test]
    fn decodes_declared_non_utf8_html() {
        let (encoded, _, _) = encoding_rs::SHIFT_JIS.encode("売上高は460億円");
        assert_eq!(
            decode_page(&encoded, "text/html; charset=Shift_JIS"),
            "売上高は460億円"
        );
    }

    #[test]
    fn challenge_text_requires_browser() {
        assert!(looks_like_challenge(
            "Please verify you are human to continue"
        ));
    }

    #[tokio::test]
    #[ignore = "requires public network access; run explicitly as a live canary"]
    async fn live_nvidia_financial_results_canary() {
        let request = FetchContentInput {
            url: "https://nvidianews.nvidia.com/news/nvidia-announces-financial-results-for-second-quarter-fiscal-2026".into(),
            max_characters: 2_500,
            query: Some("NVIDIA revenue FY2026 Q2".into()),
        };
        let result = fetch(&request, Duration::from_secs(10))
            .await
            .unwrap()
            .expect("HTML evidence");
        assert!(result.text.contains("$46.7 billion"), "{}", result.text);
        assert_eq!(result.retrieval_status, "relevant");
    }
}
