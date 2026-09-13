use regex::Regex;
use std::sync::OnceLock;

pub(super) fn redact(value: &str) -> String {
    static URL: OnceLock<Regex> = OnceLock::new();
    let urls =
        URL.get_or_init(|| Regex::new(r#"https?://[^\s<>\"']+"#).expect("static URL pattern"));
    urls.replace_all(value, |captures: &regex::Captures<'_>| {
        let Ok(mut url) = url::Url::parse(&captures[0]) else {
            return "[INVALID_URL]".to_string();
        };
        if !url.username().is_empty() || url.password().is_some() {
            let _ = url.set_username("");
            let _ = url.set_password(None);
        }
        url.to_string()
    })
    .into_owned()
}
