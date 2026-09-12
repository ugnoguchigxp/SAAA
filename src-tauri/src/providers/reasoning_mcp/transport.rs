use super::*;
use futures_util::StreamExt;
impl Client {
    fn post(&self, session: Option<&str>, body: Value) -> reqwest::RequestBuilder {
        let mut request = self
            .http
            .post(self.url.clone())
            .bearer_auth(&self.token)
            .header("Accept", "application/json, text/event-stream")
            .header("MCP-Protocol-Version", PROTOCOL)
            .json(&body);
        if let Some(session) = session {
            request = request.header("Mcp-Session-Id", session);
        }
        request
    }
    pub(super) async fn notify(
        &self,
        session: Option<&str>,
        method: &str,
        params: Value,
    ) -> Result<(), String> {
        let response = self
            .post(
                session,
                json!({"jsonrpc":"2.0","method":method,"params":params}),
            )
            .send()
            .await
            .map_err(|_| "MCP notification failed")?;
        if !response.status().is_success() {
            return Err("MCP notification rejected".into());
        }
        Ok(())
    }
    pub(super) async fn rpc(
        &self,
        session: Option<&str>,
        body: Value,
    ) -> Result<(Value, Option<String>), String> {
        let expected_id = body["id"].clone();
        let response = self
            .post(session, body)
            .send()
            .await
            .map_err(|_| "Reasoning MCP unavailable")?;
        if !response.status().is_success() {
            return Err("Reasoning MCP rejected request".into());
        }
        let session = response
            .headers()
            .get("Mcp-Session-Id")
            .and_then(|s| s.to_str().ok())
            .map(str::to_string);
        if session.as_ref().is_some_and(|v| v.len() > 256) {
            return Err("Invalid MCP session".into());
        }
        let sse = response
            .headers()
            .get("content-type")
            .and_then(|s| s.to_str().ok())
            .is_some_and(|s| s.starts_with("text/event-stream"));
        let mut stream = response.bytes_stream();
        let mut bytes = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| "Reasoning MCP disconnected")?;
            if bytes.len() + chunk.len() > 64 * 1024 {
                return Err("Reasoning MCP response too large".into());
            }
            bytes.extend_from_slice(&chunk);
            if sse {
                if let Some(value) = sse_result(&bytes, &expected_id)? {
                    return Ok((value, session));
                }
            }
        }
        if sse {
            return Err("Reasoning MCP result missing".into());
        }
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| "Invalid MCP JSON")?;
        if value["jsonrpc"] != "2.0" || value["id"] != expected_id {
            return Err("MCP response ID mismatch".into());
        }
        Ok((value, session))
    }
}
fn sse_result(bytes: &[u8], expected: &Value) -> Result<Option<Value>, String> {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return Ok(None);
    };
    let text = text.replace("\r\n", "\n");
    // Only complete frames may produce a result, never a partial JSON object.
    for frame in text.split("\n\n").take(text.matches("\n\n").count()) {
        let data = frame
            .lines()
            .filter_map(|l| l.strip_prefix("data:").map(str::trim_start))
            .collect::<Vec<_>>()
            .join("\n");
        if data.is_empty() {
            continue;
        }
        let value: Value = serde_json::from_str(&data).map_err(|_| "Invalid MCP event")?;
        if value["jsonrpc"] == "2.0"
            && value["id"] == *expected
            && (value.get("result").is_some() || value.get("error").is_some())
        {
            return Ok(Some(value));
        }
    }
    Ok(None)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sse_requires_complete_matching_frame() {
        let value = b"data: {\"jsonrpc\":\"2.0\",\"id\":\"r1\",\"result\":{}}\n\n";
        assert!(sse_result(&value[..value.len() - 1], &json!("r1"))
            .unwrap()
            .is_none());
        assert!(sse_result(value, &json!("r1")).unwrap().is_some());
        assert!(sse_result(value, &json!("other")).unwrap().is_none());
    }
}
