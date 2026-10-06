use super::*;

/// Last media HTTP response, for opt-in connection-test diagnostics. Never includes headers.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaHttpResponse {
    pub received_at: String,
    pub status: u16,
    pub body: String,
}

impl MediaClient {
    pub fn last_http_response(&self) -> Option<MediaHttpResponse> {
        self.last_http_response.lock().unwrap().clone()
    }

    pub(super) fn clear_http_response(&self) {
        *self.last_http_response.lock().unwrap() = None;
    }

    pub(super) fn record_http_response(&self, status: u16, body: &[u8]) {
        let mut body = String::from_utf8_lossy(body).into_owned();
        if !self.token.is_empty() {
            body = body.replace(self.token.as_str(), "[REDACTED]");
            let escaped = serde_json::to_string(self.token.as_str()).unwrap();
            body = body.replace(&escaped[1..escaped.len() - 1], "[REDACTED]");
        }
        // Redact before truncation so even a token crossing the boundary cannot leak.
        if body.chars().count() > 16_384 {
            body = body.chars().take(16_384).collect::<String>() + " [truncated]";
        }
        *self.last_http_response.lock().unwrap() = Some(MediaHttpResponse {
            received_at: chrono::Utc::now().to_rfc3339(),
            status,
            body,
        });
    }
}
