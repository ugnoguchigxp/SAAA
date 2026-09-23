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
    let allowed_host = url
        .host_str()
        .ok_or("source-url-invalid")?
        .to_ascii_lowercase();
    let builder = WebviewBuilder::new(label.clone(), WebviewUrl::External(url))
        .initialization_script(super::policy::DEVICE_DENY_SCRIPT)
        .incognito(true)
        .focused(false)
        .disable_drag_drop_handler()
        .on_navigation(move |candidate| allow_source_navigation(&allowed_host, candidate))
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
    if found.is_none() {
        return Err("source-not-saved".into());
    }
    Ok(url)
}

fn allow_source_navigation(allowed_host: &str, candidate: &Url) -> bool {
    matches!(candidate.scheme(), "http" | "https")
        && candidate
            .host_str()
            .is_some_and(|host| host.eq_ignore_ascii_case(allowed_host))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_view_stays_on_the_cited_website() {
        assert!(allow_source_navigation(
            "tenki.jp",
            &Url::parse("https://tenki.jp/forecast/3/17").unwrap(),
        ));
        assert!(!allow_source_navigation(
            "tenki.jp",
            &Url::parse("https://advertiser.example/landing").unwrap(),
        ));
        assert!(!allow_source_navigation(
            "tenki.jp",
            &Url::parse("file:///tmp/private").unwrap(),
        ));
    }
}
