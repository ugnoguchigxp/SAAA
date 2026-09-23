use tauri::webview::{NewWindowResponse, WebviewBuilder};
use tauri::{AppHandle, LogicalPosition, LogicalSize, Manager, WebviewUrl};
use url::Url;

use crate::AppState;

pub(super) fn mount(
    app: &AppHandle,
    state: &AppState,
    conversation_id: &str,
    source_url: &str,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
) -> Result<String, String> {
    super::host::validate_bounds(x, y, width, height).map_err(str::to_string)?;
    let url = authorized_url(state, conversation_id, source_url)?;
    let window = app.get_window("main").ok_or("source-window-unavailable")?;
    let label = format!("source-website-{}", uuid::Uuid::new_v4());
    let allowed_origin = url.origin().ascii_serialization();
    let builder = WebviewBuilder::new(label.clone(), WebviewUrl::External(url))
        .initialization_script(super::policy::DEVICE_DENY_SCRIPT)
        .incognito(true)
        .focused(false)
        .disable_drag_drop_handler()
        .on_navigation(move |candidate| allow_source_navigation(&allowed_origin, candidate))
        .on_download(|_, _| false)
        .on_new_window(|_, _| NewWindowResponse::Deny);
    window
        .add_child(
            builder,
            LogicalPosition::new(x, y),
            LogicalSize::new(width, height),
        )
        .map_err(|_| "source-window-unavailable".to_string())?;
    Ok(label)
}

pub(super) fn open_in_browser(
    state: &AppState,
    conversation_id: &str,
    source_url: &str,
) -> Result<(), String> {
    let url = authorized_url(state, conversation_id, source_url)?;
    open::that(url.as_str()).map_err(|_| "source-browser-unavailable".to_string())
}

pub(super) fn scroll(app: &AppHandle, label: &str) -> Result<(), String> {
    if !label.starts_with("source-website-") || uuid::Uuid::parse_str(&label[15..]).is_err() {
        return Err("source-webview-invalid".into());
    }
    let webview = app.get_webview(label).ok_or("source-webview-unavailable")?;
    webview
        .eval("window.scrollBy(0, Math.max(240, window.innerHeight * 0.6))")
        .map_err(|_| "source-webview-scroll-failed".into())
}

fn authorized_url(
    state: &AppState,
    conversation_id: &str,
    source_url: &str,
) -> Result<Url, String> {
    crate::validate_identifier(conversation_id, "conversation id")?;
    let url = Url::parse(source_url).map_err(|_| "source-url-invalid".to_string())?;
    if !matches!(url.scheme(), "http" | "https") || source_url.len() > 2_048 {
        return Err("source-url-invalid".into());
    }
    if !crate::runtime::web_fetch::search::is_allowed_result_url(source_url) {
        return Err("source-url-not-public".into());
    }
    let principal = crate::tool_selection::service::ensure_principal(&state.sqlite_writer)
        .map_err(|error| error.to_string())?;
    let found = state.sqlite_readers.read(|connection| {
        let auth = crate::records::auth::Authorization {
            principal_id: principal,
            conversation_id: conversation_id.to_string(),
            allowed_scope_keys: Vec::new(),
        };
        super::source::read_source(connection, &auth, source_url)
    })?;
    if found.is_none() && !assistant_message_contains_url(state, conversation_id, &url) {
        return Err("source-not-saved".into());
    }
    Ok(url)
}

fn assistant_message_contains_url(state: &AppState, conversation_id: &str, url: &Url) -> bool {
    state
        .sqlite_readers
        .read(|connection| {
            let mut statement = connection
                .prepare(
                    "SELECT content FROM conversation_messages WHERE conversation_id=?1 AND role='assistant'",
                )
                .map_err(|error| error.to_string())?;
            let rows = statement
                .query_map([conversation_id], |row| row.get::<_, String>(0))
                .map_err(|error| error.to_string())?;
            let mut normalized = url.clone();
            normalized.set_fragment(None);
            let target = normalized.to_string();
            for content in rows.flatten() {
                if extract_answer_urls(&content).iter().any(|item| item == &target) {
                    return Ok(true);
                }
            }
            Ok(false)
        })
        .unwrap_or(false)
}

pub(crate) fn extract_answer_urls(markdown: &str) -> Vec<String> {
    let mut text = String::new();
    let mut rest = markdown;
    while let Some(start) = rest.find("```") {
        text.push_str(&rest[..start]);
        text.push(' ');
        rest = &rest[start + 3..];
        if let Some(end) = rest.find("```") {
            rest = &rest[end + 3..];
        } else {
            rest = "";
        }
    }
    text.push_str(rest);
    let mut visible = String::new();
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '!' && chars.peek() == Some(&'[') {
            for next in chars.by_ref() {
                if next == ')' {
                    break;
                }
            }
            visible.push(' ');
            continue;
        }
        if ch == '`' {
            for next in chars.by_ref() {
                if next == '`' {
                    break;
                }
            }
            visible.push(' ');
            continue;
        }
        visible.push(ch);
    }
    let mut urls = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    let mut starts = visible
        .match_indices("http://")
        .chain(visible.match_indices("https://"))
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    starts.sort_unstable();
    for index in starts {
        let slice = &visible[index..];
        let end = slice
            .find(|ch: char| ch.is_whitespace() || matches!(ch, '<' | ')' | ']' | '>' | '"'))
            .unwrap_or(slice.len());
        let raw = slice[..end].trim_end_matches(['.', ',', ';', ':', '!', '?', ')']);
        if let Ok(parsed) = Url::parse(raw) {
            if matches!(parsed.scheme(), "http" | "https") {
                let mut clean = parsed;
                clean.set_fragment(None);
                let href = clean.to_string();
                if seen.insert(href.clone()) {
                    urls.push(href);
                }
            }
        }
    }
    urls
}

fn allow_source_navigation(allowed_origin: &str, candidate: &Url) -> bool {
    matches!(candidate.scheme(), "http" | "https")
        && candidate.origin().ascii_serialization() == allowed_origin
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_view_stays_on_the_cited_website() {
        let origin = Url::parse("https://tenki.jp/forecast")
            .unwrap()
            .origin()
            .ascii_serialization();
        assert!(allow_source_navigation(
            &origin,
            &Url::parse("https://tenki.jp/forecast/3/17").unwrap(),
        ));
        assert!(!allow_source_navigation(
            &origin,
            &Url::parse("https://advertiser.example/landing").unwrap(),
        ));
        assert!(!allow_source_navigation(
            &origin,
            &Url::parse("http://tenki.jp/forecast").unwrap(),
        ));
        assert!(!allow_source_navigation(
            &origin,
            &Url::parse("https://tenki.jp:8443/forecast").unwrap(),
        ));
        assert!(!allow_source_navigation(
            &origin,
            &Url::parse("file:///tmp/private").unwrap(),
        ));
        let urls = extract_answer_urls(
            "See [News](https://example.com/a) and https://example.com/b.\n```\nhttps://skip.example/c\n```\n`https://code.example/e`\n![pic](https://images.example/d.png)",
        );
        assert_eq!(
            urls,
            vec![
                "https://example.com/a".to_string(),
                "https://example.com/b".to_string()
            ]
        );
        assert_eq!(
            extract_answer_urls("https://first.example/a then [Second](http://second.example/b)"),
            vec![
                "https://first.example/a".to_string(),
                "http://second.example/b".to_string()
            ]
        );
    }
}
