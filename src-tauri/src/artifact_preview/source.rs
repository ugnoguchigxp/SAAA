use rusqlite::{params_from_iter, types::Value as SqlValue, Connection};
use serde::Serialize;

use crate::records::{auth::Authorization, read};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SourceArtifact {
    pub url: String,
    pub text: String,
    pub observed_at: i64,
    pub truncated: bool,
}

pub(crate) fn read_source(
    connection: &Connection,
    auth: &Authorization,
    url: &str,
) -> Result<Option<SourceArtifact>, String> {
    let parsed = url::Url::parse(url).map_err(|_| "source-url-invalid")?;
    if !matches!(parsed.scheme(), "http" | "https") || url.len() > 2_048 {
        return Err("source-url-invalid".into());
    }
    let (filter, filter_values) = auth.sql_filter("r");
    let sql = format!(
        "SELECT r.id,r.observed_at,r.kind FROM records r WHERE {filter} \
         AND r.kind IN ('web_fetch','web_search_result') \
         AND r.capture_state='complete' AND json_extract(r.locator_json,'$.url')=? \
         ORDER BY CASE r.kind WHEN 'web_fetch' THEN 0 ELSE 1 END,r.recorded_at DESC LIMIT 1"
    );
    let mut values = filter_values;
    values.push(SqlValue::Text(url.to_string()));
    let mut statement = connection
        .prepare(&sql)
        .map_err(|error| error.to_string())?;
    let mut rows = statement
        .query(params_from_iter(values.iter()))
        .map_err(|error| error.to_string())?;
    let Some(row) = rows.next().map_err(|error| error.to_string())? else {
        return Ok(None);
    };
    let id: String = row.get(0).map_err(|error| error.to_string())?;
    let observed_at: i64 = row.get(1).map_err(|error| error.to_string())?;
    let kind: String = row.get(2).map_err(|error| error.to_string())?;
    drop(rows);
    drop(statement);
    let mut text = String::new();
    let mut start = 0;
    let truncated = loop {
        let Some(part) = read::read_range(
            connection,
            auth,
            &id,
            "readable_text",
            read::ReadRange {
                start,
                max_bytes: 8_192,
            },
        )?
        else {
            return Ok(None);
        };
        text.push_str(&part.text);
        start = part.actual_end;
        if !part.truncated || text.len() >= 20_000 || part.actual_end == part.actual_start {
            break part.truncated;
        }
    };
    if kind == "web_search_result" {
        let hit: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
        text = [hit["title"].as_str(), hit["snippet"].as_str()]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join("\n\n");
    }
    Ok(Some(SourceArtifact {
        url: url.to_string(),
        text,
        observed_at,
        truncated,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::records::{
        write::{commit, NewRecord},
        Origin, RecordKind,
    };

    #[test]
    fn source_reader_prefers_saved_page_and_is_conversation_scoped() {
        let connection = Connection::open_in_memory().unwrap();
        crate::persistence::schema::initialize_database(&connection).unwrap();
        let auth = Authorization {
            principal_id: "principal".into(),
            conversation_id: "conversation".into(),
            allowed_scope_keys: Vec::new(),
        };
        for (kind, text) in [
            (
                RecordKind::WebSearchResult,
                r#"{"title":"Hit","snippet":"Search excerpt"}"#,
            ),
            (RecordKind::WebFetch, "Saved page text"),
        ] {
            commit(
                &connection,
                NewRecord {
                    kind,
                    origin: Origin::ExternalObservation,
                    principal_id: &auth.principal_id,
                    conversation_id: &auth.conversation_id,
                    run_id: None,
                    turn_id: None,
                    parent_execution_id: None,
                    rank: None,
                    observed_at: 123,
                    locator: serde_json::json!({"url":"https://example.com/source"}),
                    scope_keys: &[],
                },
                text.as_bytes(),
                Some(text),
            )
            .unwrap();
        }
        let result = read_source(&connection, &auth, "https://example.com/source")
            .unwrap()
            .unwrap();
        assert_eq!(result.text, "Saved page text");
        assert!(read_source(
            &connection,
            &Authorization {
                conversation_id: "other".into(),
                ..auth.clone()
            },
            "https://example.com/source"
        )
        .unwrap()
        .is_none());
        assert!(read_source(&connection, &auth, "file:///tmp/source").is_err());
    }
}
