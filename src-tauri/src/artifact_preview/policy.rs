use url::Url;

pub(crate) fn allow_navigation(bound_token: &str, candidate: &str) -> Result<(), &'static str> {
    let Ok(url) = Url::parse(candidate) else {
        return Err("navigation-denied");
    };
    if matches!(url.scheme(), "file" | "data" | "blob" | "about") {
        return Err("navigation-denied");
    }
    same_preview(bound_token, &url)
}

fn same_preview(bound_token: &str, url: &Url) -> Result<(), &'static str> {
    match super::protocol::token_from_url(url) {
        Some(token) if token == bound_token => Ok(()),
        _ => Err("navigation-denied"),
    }
}

pub(crate) fn allow_popup(_candidate: &str) -> Result<(), &'static str> {
    Err("popup-denied")
}

pub(crate) fn allow_download(_candidate: &str) -> Result<(), &'static str> {
    Err("download-denied")
}

#[cfg(test)]
pub(crate) fn allow_permission(_name: &str) -> Result<(), &'static str> {
    Err("permission-denied")
}

pub(crate) const DEVICE_DENY_SCRIPT: &str = r#"
(() => {
  const denied = () => Promise.reject(new DOMException("permission-denied", "NotAllowedError"));
  const media = {
    getUserMedia: denied,
    getDisplayMedia: denied,
    enumerateDevices: () => Promise.resolve([]),
  };
  const lock = (target, key, value) => {
    try {
      Object.defineProperty(target, key, { configurable: false, value });
    } catch (_) {}
  };
  lock(navigator, "mediaDevices", media);
  lock(navigator, "geolocation", undefined);
  lock(navigator, "clipboard", undefined);
  if (typeof Notification !== "undefined") {
    lock(Notification, "requestPermission", () => Promise.resolve("denied"));
  }
})();
"#;

pub(crate) fn sanitize_reason(reason: &str) -> &'static str {
    match reason {
        "navigation-denied" => "navigation-denied",
        "popup-denied" => "popup-denied",
        "download-denied" => "download-denied",
        "permission-denied" => "permission-denied",
        "token-unknown" => "token-unknown",
        "token-expired" => "token-expired",
        "path-invalid" => "path-invalid",
        "method-not-allowed" => "method-not-allowed",
        _ => "policy-denied",
    }
}
