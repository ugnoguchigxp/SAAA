use serde_json::Value;

#[derive(Debug)]
pub(crate) enum CalError {
    Precondition,
    Gone,
    Unreachable,
    Auth,
    NotFound,
    Other(()),
}

#[derive(Clone, Debug)]
pub(crate) struct RemoteEvent {
    pub(crate) id: String,
    pub(crate) etag: String,
    pub(crate) status: String,
    pub(crate) summary: Option<String>,
    pub(crate) start: Option<i64>,
    pub(crate) end: Option<i64>,
    pub(crate) entry_id: Option<String>,
    pub(crate) rev: Option<i64>,
    pub(crate) hash: Option<String>,
}

impl RemoteEvent {
    pub(crate) fn from_json(value: &Value) -> Option<Self> {
        let private = value
            .pointer("/extendedProperties/private")
            .cloned()
            .unwrap_or(Value::Null);
        Some(Self {
            id: value.get("id")?.as_str()?.to_string(),
            etag: value
                .get("etag")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            status: value
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("confirmed")
                .to_string(),
            summary: value
                .get("summary")
                .and_then(Value::as_str)
                .map(str::to_string),
            start: parse_time(value.pointer("/start/dateTime")),
            end: parse_time(value.pointer("/end/dateTime")),
            entry_id: private
                .get("saaa_entry_id")
                .and_then(Value::as_str)
                .map(str::to_string),
            rev: private
                .get("saaa_rev")
                .and_then(Value::as_str)
                .and_then(|value| value.parse().ok()),
            hash: private
                .get("saaa_hash")
                .and_then(Value::as_str)
                .map(str::to_string),
        })
    }
}

fn parse_time(value: Option<&Value>) -> Option<i64> {
    let text = value.and_then(Value::as_str)?;
    chrono::DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|time| time.timestamp_millis())
}

#[derive(Clone)]
pub(crate) struct Client {
    pub(crate) base_url: String,
    pub(crate) token: String,
}

impl Client {
    async fn send(
        &self,
        method: reqwest::Method,
        path: &str,
        etag: Option<&str>,
        body: Option<&Value>,
    ) -> Result<(u16, Value), CalError> {
        let mut attempt = 0;
        loop {
            let url = format!("{}{path}", self.base_url);
            let mut request = reqwest::Client::new()
                .request(method.clone(), url)
                .bearer_auth(&self.token)
                .header("content-type", "application/json");
            if let Some(etag) = etag {
                request = request.header("If-Match", etag);
            }
            if let Some(body) = body {
                request = request.json(body);
            }
            let response = request.send().await.map_err(|_| CalError::Unreachable)?;
            let status = response.status().as_u16();
            if matches!(status, 429 | 500 | 502 | 503) && attempt < 3 {
                attempt += 1;
                tokio::time::sleep(std::time::Duration::from_millis(250 * (1 << attempt))).await;
                continue;
            }
            if status == 412 {
                return Err(CalError::Precondition);
            }
            if status == 410 {
                return Err(CalError::Gone);
            }
            if status == 401 || status == 403 {
                return Err(CalError::Auth);
            }
            if status == 404 {
                return Err(CalError::NotFound);
            }
            let value = response.json::<Value>().await.unwrap_or(Value::Null);
            if !(200..300).contains(&status) {
                return Err(CalError::Other(()));
            }
            return Ok((status, value));
        }
    }

    pub(crate) async fn insert(
        &self,
        calendar_id: &str,
        body: &Value,
    ) -> Result<RemoteEvent, CalError> {
        let path = event_collection_path(calendar_id, None);
        let (_status, value) = self
            .send(reqwest::Method::POST, &path, None, Some(body))
            .await?;
        RemoteEvent::from_json(&value).ok_or(CalError::Other(()))
    }

    pub(crate) async fn patch(
        &self,
        calendar_id: &str,
        event_id: &str,
        etag: &str,
        body: &Value,
    ) -> Result<RemoteEvent, CalError> {
        let path = event_item_path(calendar_id, event_id);
        let (_status, value) = self
            .send(reqwest::Method::PATCH, &path, Some(etag), Some(body))
            .await?;
        RemoteEvent::from_json(&value).ok_or(CalError::Other(()))
    }

    pub(crate) async fn delete(
        &self,
        calendar_id: &str,
        event_id: &str,
        etag: Option<&str>,
    ) -> Result<(), CalError> {
        let path = event_item_path(calendar_id, event_id);
        match self.send(reqwest::Method::DELETE, &path, etag, None).await {
            Ok(_) | Err(CalError::NotFound) => Ok(()),
            Err(error) => Err(error),
        }
    }

    pub(crate) async fn list(
        &self,
        calendar_id: &str,
        sync_token: Option<&str>,
    ) -> Result<(Vec<RemoteEvent>, Option<String>), CalError> {
        let path = match sync_token {
            Some(token) => format!(
                "{}?syncToken={}",
                event_collection_path(calendar_id, None),
                urlencoding(token)
            ),
            None => format!(
                "{}?showDeleted=true",
                event_collection_path(calendar_id, None)
            ),
        };
        let (_status, value) = self.send(reqwest::Method::GET, &path, None, None).await?;
        let items = value
            .get("items")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let events = items.iter().filter_map(RemoteEvent::from_json).collect();
        let next = value
            .get("nextSyncToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        Ok((events, next))
    }

    pub(crate) async fn find_by_entry(
        &self,
        calendar_id: &str,
        entry_id: &str,
    ) -> Result<Option<RemoteEvent>, CalError> {
        let path = event_collection_path(calendar_id, Some(entry_id));
        let (_status, value) = self.send(reqwest::Method::GET, &path, None, None).await?;
        let items = value
            .get("items")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        Ok(items.first().and_then(RemoteEvent::from_json))
    }
}

fn event_collection_path(calendar_id: &str, entry_id: Option<&str>) -> String {
    let base = format!("/calendars/{}/events", urlencoding(calendar_id));
    match entry_id {
        Some(entry_id) => format!(
            "{base}?privateExtendedProperty=saaa_entry_id%3D{}",
            urlencoding(entry_id)
        ),
        None => base,
    }
}

fn event_item_path(calendar_id: &str, event_id: &str) -> String {
    format!(
        "/calendars/{}/events/{}",
        urlencoding(calendar_id),
        urlencoding(event_id)
    )
}

fn urlencoding(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sl_12_maps_http_codes() {
        assert!(matches!(status_error(412), CalError::Precondition));
        assert!(matches!(status_error(410), CalError::Gone));
        assert!(matches!(status_error(401), CalError::Auth));
    }

    fn status_error(status: u16) -> CalError {
        match status {
            412 => CalError::Precondition,
            410 => CalError::Gone,
            401 | 403 => CalError::Auth,
            404 => CalError::NotFound,
            _ => CalError::Other(()),
        }
    }
}
