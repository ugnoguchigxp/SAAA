use super::auth::{pkce_pair, store_refresh, unsupported};
use crate::schedule::runtime;
use crate::AppState;
use serde_json::Value;
use std::sync::Mutex;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const SCOPE: &str = "https://www.googleapis.com/auth/calendar";
const AUTH: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN: &str = "https://oauth2.googleapis.com/token";
static PENDING: Mutex<Option<String>> = Mutex::new(None);

#[cfg(test)]
pub(crate) fn pending_redirect() -> Option<String> {
    PENDING.lock().ok().and_then(|slot| slot.clone())
}

pub(crate) fn authorize_url(
    client_id: &str,
    redirect: &str,
    challenge: &str,
    state: &str,
) -> String {
    format!(
        "{AUTH}?client_id={}&redirect_uri={}&response_type=code&scope={}&code_challenge={}&code_challenge_method=S256&state={}&access_type=offline&prompt=consent",
        urlencoding(client_id),
        urlencoding(redirect),
        urlencoding(SCOPE),
        urlencoding(challenge),
        urlencoding(state)
    )
}

pub(crate) fn parse_callback(target: &str, expected_state: &str) -> Result<String, String> {
    let query = target.split_once('?').map(|part| part.1).unwrap_or(target);
    let mut code = None;
    let mut state = None;
    let mut error = None;
    for pair in query.split('&') {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        match key {
            "code" => code = Some(urldecoding(value)),
            "state" => state = Some(urldecoding(value)),
            "error" => error = Some(urldecoding(value)),
            _ => {}
        }
    }
    if error.is_some() {
        return Err("oauth-denied".into());
    }
    if state.as_deref() != Some(expected_state) {
        return Err("oauth-state".into());
    }
    code.filter(|value| !value.is_empty())
        .ok_or_else(|| "oauth-code".into())
}

pub(crate) async fn connect(state: &AppState, client_id: Option<String>) -> Result<(), String> {
    unsupported()?;
    let client_id = resolve_client_id(state, client_id)?;
    let (verifier, challenge) = pkce_pair();
    let csrf = uuid::Uuid::new_v4().simple().to_string();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|_| "oauth-bind".to_string())?;
    let port = listener
        .local_addr()
        .map_err(|_| "oauth-bind".to_string())?
        .port();
    let redirect = format!("http://127.0.0.1:{port}/");
    let url = authorize_url(&client_id, &redirect, &challenge, &csrf);
    if let Ok(mut slot) = PENDING.lock() {
        *slot = Some(url.clone());
    }
    if !cfg!(test) {
        open_browser(&url)?;
    }
    let seconds = if cfg!(test) { 8 } else { 180 };
    let code = tokio::time::timeout(
        std::time::Duration::from_secs(seconds),
        wait_code(listener, &csrf),
    )
    .await
    .map_err(|_| "oauth-timeout".to_string())??;
    if let Ok(mut slot) = PENDING.lock() {
        *slot = None;
    }
    let (refresh, access) = exchange(
        &token_endpoint(&state.schedule.http_base()),
        &client_id,
        &redirect,
        &verifier,
        &code,
    )
    .await?;
    store_refresh(&state.schedule, &refresh)?;
    state.schedule.set_access(&access);
    Ok(())
}

pub(crate) fn ensure_access(state: &AppState) -> Option<String> {
    if let Some(token) = state.schedule.access() {
        return Some(token);
    }
    let refresh = super::auth::load_refresh(&state.schedule).ok().flatten()?;
    let client_id = resolve_client_id(state, None).ok()?;
    let endpoint = token_endpoint(&state.schedule.http_base());
    std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .ok()?
            .block_on(refresh_token(&endpoint, &client_id, &refresh))
            .ok()
    })
    .join()
    .ok()
    .flatten()
    .inspect(|access| state.schedule.set_access(access))
}

fn resolve_client_id(state: &AppState, passed: Option<String>) -> Result<String, String> {
    let passed = passed
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    if let Some(id) = passed {
        validate_client_id(&id)?;
        let _ = state
            .sqlite_writer
            .write(|connection| runtime::set_oauth_client(connection, Some(&id)));
        return Ok(id);
    }
    if let Some(id) = state
        .sqlite_writer
        .read_serialized(runtime::load)
        .ok()
        .and_then(|settings| settings.oauth_client_id)
        .filter(|value| !value.is_empty())
    {
        return Ok(id);
    }
    std::env::var("SAAA_GOOGLE_CALENDAR_CLIENT_ID")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "oauth-client-id".into())
        .and_then(|id| {
            validate_client_id(&id)?;
            Ok(id)
        })
}

fn validate_client_id(id: &str) -> Result<(), String> {
    if (8..=160).contains(&id.len())
        && id.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        })
    {
        Ok(())
    } else {
        Err("oauth-client-id".into())
    }
}

fn token_endpoint(http_base: &str) -> String {
    if http_base.starts_with("https://www.googleapis.com") {
        TOKEN.to_string()
    } else {
        format!("{}/token", http_base.trim_end_matches('/'))
    }
}

fn open_browser(url: &str) -> Result<(), String> {
    if !url.starts_with(AUTH) {
        return Err("oauth-url".into());
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(url)
            .status()
            .map_err(|_| "oauth-open".to_string())?;
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = url;
        Err("Calendar connection requires macOS".into())
    }
}

async fn wait_code(
    listener: tokio::net::TcpListener,
    expected_state: &str,
) -> Result<String, String> {
    let (mut stream, _) = listener
        .accept()
        .await
        .map_err(|_| "oauth-accept".to_string())?;
    let mut bytes = Vec::new();
    let mut buffer = [0; 1024];
    loop {
        let count = stream
            .read(&mut buffer)
            .await
            .map_err(|_| "oauth-read".to_string())?;
        if count == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..count]);
        if bytes.len() > 8_192 {
            return Err("oauth-request".into());
        }
        if bytes.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
    }
    let header = String::from_utf8_lossy(&bytes);
    let target = header
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("");
    let result = parse_callback(target, expected_state);
    let ok = result.is_ok();
    let body = if ok {
        "Calendar connected. You can close this window."
    } else {
        "Calendar connection failed."
    };
    let status = if ok { "200 OK" } else { "400 Bad Request" };
    let _ = stream
        .write_all(
            format!(
                "HTTP/1.1 {status}\r\ncontent-type: text/plain; charset=utf-8\r\nconnection: close\r\ncontent-length: {}\r\n\r\n{body}",
                body.len()
            )
            .as_bytes(),
        )
        .await;
    result
}

async fn exchange(
    endpoint: &str,
    client_id: &str,
    redirect: &str,
    verifier: &str,
    code: &str,
) -> Result<(String, String), String> {
    let body = format!(
        "grant_type=authorization_code&code={}&redirect_uri={}&client_id={}&code_verifier={}",
        urlencoding(code),
        urlencoding(redirect),
        urlencoding(client_id),
        urlencoding(verifier)
    );
    let (refresh, access) = token_pair(endpoint, &body).await?;
    if refresh.is_empty() {
        return Err("oauth-refresh".into());
    }
    Ok((refresh, access))
}

async fn refresh_token(endpoint: &str, client_id: &str, refresh: &str) -> Result<String, String> {
    let body = format!(
        "grant_type=refresh_token&refresh_token={}&client_id={}",
        urlencoding(refresh),
        urlencoding(client_id)
    );
    token_pair(endpoint, &body).await.map(|pair| pair.1)
}

async fn token_pair(endpoint: &str, body: &str) -> Result<(String, String), String> {
    let response = reqwest::Client::new()
        .post(endpoint)
        .header("content-type", "application/x-www-form-urlencoded")
        .body(body.to_string())
        .send()
        .await
        .map_err(|_| "oauth-token".to_string())?;
    if !response.status().is_success() {
        return Err("oauth-token".into());
    }
    let value = response
        .json::<Value>()
        .await
        .map_err(|_| "oauth-token".to_string())?;
    let access = value
        .get("access_token")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "oauth-token".to_string())?;
    let refresh = value
        .get("refresh_token")
        .and_then(Value::as_str)
        .unwrap_or("");
    Ok((refresh.to_string(), access.to_string()))
}

fn urlencoding(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

fn urldecoding(value: &str) -> String {
    url::form_urlencoded::parse(format!("q={value}").as_bytes())
        .next()
        .map(|(_, decoded)| decoded.into_owned())
        .unwrap_or_else(|| value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::schema::initialize_database;
    use crate::test_support::app_state;
    use rusqlite::Connection;
    use tokio::io::AsyncWriteExt;

    #[test]
    fn sl_11_authorize_url_is_pkce_s256_calendar_scope() {
        let url = authorize_url(
            "id.apps.googleusercontent.com",
            "http://127.0.0.1:9/",
            "ch",
            "st",
        );
        assert!(url.starts_with(AUTH));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("access_type=offline"));
        assert!(url.contains(&urlencoding(SCOPE)));
        assert!(!url.contains("refresh_token"));
    }

    #[test]
    fn sl_11_callback_rejects_state_mismatch() {
        assert!(parse_callback("/?code=a&state=x", "y").is_err());
        assert_eq!(parse_callback("/?code=ok&state=st", "st").unwrap(), "ok");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn sl_11_loopback_exchanges_code_without_sqlite_token() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    break;
                };
                let mut bytes = [0; 2048];
                let count = socket.read(&mut bytes).await.unwrap_or(0);
                let header = String::from_utf8_lossy(&bytes[..count]);
                let body = if header.contains("grant_type=refresh_token") {
                    serde_json::json!({ "access_token": "access-2", "token_type": "Bearer" })
                        .to_string()
                } else {
                    serde_json::json!({
                        "access_token": "access-1",
                        "refresh_token": "refresh-1",
                        "token_type": "Bearer"
                    })
                    .to_string()
                };
                let _ = socket
                    .write_all(
                        format!(
                            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}",
                            body.len()
                        )
                        .as_bytes(),
                    )
                    .await;
            }
        });
        let connection = Connection::open_in_memory().unwrap();
        initialize_database(&connection).unwrap();
        let state = app_state(connection);
        state
            .schedule
            .set_http_base(format!("http://127.0.0.1:{}", address.port()));
        let connected = connect(&state, Some("test.apps.googleusercontent.com".into()));
        let redirected = async {
            let mut url = None;
            for _ in 0..200 {
                url = pending_redirect();
                if url.is_some() {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            let parsed = url::Url::parse(&url.expect("authorize url")).unwrap();
            let csrf = parsed
                .query_pairs()
                .find(|(key, _)| key == "state")
                .map(|(_, value)| value.into_owned())
                .unwrap();
            let port = parsed
                .query_pairs()
                .find(|(key, _)| key == "redirect_uri")
                .and_then(|(_, value)| url::Url::parse(&value).ok()?.port())
                .unwrap();
            reqwest::get(format!("http://127.0.0.1:{port}/?code=c1&state={csrf}"))
                .await
                .unwrap();
        };
        let (result, _) = tokio::join!(connected, redirected);
        result.unwrap();
        assert_eq!(state.schedule.refresh().as_deref(), Some("refresh-1"));
        assert_eq!(state.schedule.access().as_deref(), Some("access-1"));
        let stored = state
            .sqlite_writer
            .read_serialized(crate::schedule::runtime::load)
            .unwrap();
        assert_eq!(
            stored.oauth_client_id.as_deref(),
            Some("test.apps.googleusercontent.com")
        );
        let dump = format!(
            "{}{}{}",
            stored.calendar_id.as_deref().unwrap_or(""),
            stored.last_error.as_deref().unwrap_or(""),
            stored.oauth_client_id.as_deref().unwrap_or("")
        );
        assert!(!dump.contains("refresh-1") && !dump.contains("access-1"));
        state.schedule.set_access("");
        assert_eq!(ensure_access(&state).as_deref(), Some("access-2"));
    }
}
