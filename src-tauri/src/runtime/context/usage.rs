use rusqlite::{params, Connection};
use serde_json::Value;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ProviderUsage {
    pub(crate) input_tokens: Option<u64>,
    pub(crate) cache_read_tokens: Option<u64>,
    pub(crate) cache_write_tokens: Option<u64>,
    pub(crate) output_tokens: Option<u64>,
    pub(crate) reasoning_tokens: Option<u64>,
    pub(crate) raw: Option<Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UsageSource {
    Provider,
    Missing,
    Disconnected,
}

impl UsageSource {
    fn as_str(self) -> &'static str {
        match self {
            Self::Provider => "provider",
            Self::Missing => "missing",
            Self::Disconnected => "disconnected",
        }
    }
}

pub(crate) struct UsageTimings {
    pub(crate) ttft_ms: Option<u64>,
    pub(crate) first_visible_ms: Option<u64>,
    pub(crate) completed_ms: Option<u64>,
}

pub(crate) fn parse_openai_usage(value: &Value) -> ProviderUsage {
    let usage = value
        .get("usage")
        .filter(|item| item.is_object())
        .unwrap_or(value);
    ProviderUsage {
        input_tokens: u64_at(usage, "prompt_tokens"),
        cache_read_tokens: usage
            .get("prompt_tokens_details")
            .and_then(|details| u64_at(details, "cached_tokens")),
        cache_write_tokens: None,
        output_tokens: u64_at(usage, "completion_tokens"),
        reasoning_tokens: usage
            .get("completion_tokens_details")
            .and_then(|details| u64_at(details, "reasoning_tokens")),
        raw: Some(usage.clone()),
    }
}

fn u64_at(value: &Value, key: &str) -> Option<u64> {
    value.get(key).and_then(Value::as_u64)
}

fn i64_opt(value: Option<u64>) -> Option<i64> {
    value.and_then(|token| i64::try_from(token).ok())
}

pub(crate) fn record(
    connection: &Connection,
    generation_id: &str,
    provider_id: &str,
    model: Option<&str>,
    usage: &ProviderUsage,
    source: UsageSource,
    wire_bytes: usize,
    timings: &UsageTimings,
) -> Result<(), String> {
    let raw = usage
        .raw
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .map_err(|error| error.to_string())?;
    connection
        .execute(
            "INSERT INTO generation_usage(
               generation_id, provider_id, model, input_tokens, cache_read_tokens,
               cache_write_tokens, output_tokens, reasoning_tokens, usage_source,
               raw_usage_json, wire_bytes, prefix_match_bytes, ttft_ms, first_visible_ms,
               completed_ms, recorded_at
             ) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,NULL,?12,?13,?14,?15)",
            params![
                generation_id,
                provider_id,
                model,
                i64_opt(usage.input_tokens),
                i64_opt(usage.cache_read_tokens),
                i64_opt(usage.cache_write_tokens),
                i64_opt(usage.output_tokens),
                i64_opt(usage.reasoning_tokens),
                source.as_str(),
                raw,
                i64::try_from(wire_bytes).unwrap_or(i64::MAX),
                i64_opt(timings.ttft_ms),
                i64_opt(timings.first_visible_ms),
                i64_opt(timings.completed_ms),
                crate::schedule::tick::now_ms(),
            ],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ProviderUsageSummary {
    pub(crate) provider_id: String,
    pub(crate) generations: i64,
    pub(crate) input_tokens: i64,
    pub(crate) cache_read_tokens: i64,
    pub(crate) cache_read_ratio: Option<f64>,
    pub(crate) usage_missing: i64,
}

pub(crate) fn summary(connection: &Connection) -> Result<Vec<ProviderUsageSummary>, String> {
    let mut statement = connection
        .prepare(
            "SELECT provider_id,
                    COUNT(*),
                    COALESCE(SUM(input_tokens), 0),
                    COALESCE(SUM(cache_read_tokens), 0),
                    SUM(CASE WHEN usage_source = 'missing' THEN 1 ELSE 0 END)
             FROM generation_usage
             GROUP BY provider_id
             ORDER BY provider_id",
        )
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map([], |row| {
            let input: i64 = row.get(2)?;
            let cache: i64 = row.get(3)?;
            Ok(ProviderUsageSummary {
                provider_id: row.get(0)?,
                generations: row.get(1)?,
                input_tokens: input,
                cache_read_tokens: cache,
                cache_read_ratio: (input > 0).then_some(cache as f64 / input as f64),
                usage_missing: row.get(4)?,
            })
        })
        .map_err(|error| error.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn generation(connection: &Connection, id: &str, provider: &str) {
        crate::persistence::schema::initialize_database(connection).unwrap();
        connection
            .execute(
                "INSERT INTO runtime_runs(id,conversation_id,route_kind,status,started_at)
                 VALUES(?1,?2,'conversation.respond','running','1')",
                rusqlite::params![id, crate::PRIMARY_CONVERSATION_ID],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO context_generations(
                   id,run_id,provider_id,ordinal,purpose,envelope_digest,request_digest,
                   projected_bytes,current_instruction_count,health_status,status,started_at
                 ) VALUES(?1,?1,?2,1,'reasoning',?3,?3,10,1,'green','dispatched','1')",
                rusqlite::params![id, provider, "a".repeat(64)],
            )
            .unwrap();
    }

    #[test]
    fn cw_11_parse_openai_usage_reads_cached_tokens() {
        let parsed = parse_openai_usage(&json!({
            "prompt_tokens": 20,
            "prompt_tokens_details": {"cached_tokens": 8},
            "completion_tokens": 3,
            "completion_tokens_details": {"reasoning_tokens": 1}
        }));
        assert_eq!(parsed.input_tokens, Some(20));
        assert_eq!(parsed.cache_read_tokens, Some(8));
        assert_eq!(parsed.output_tokens, Some(3));
        assert_eq!(parsed.reasoning_tokens, Some(1));
    }

    #[test]
    fn cw_11_parse_missing_fields_are_none() {
        let parsed = parse_openai_usage(&json!({"prompt_tokens": 4}));
        assert_eq!(parsed.input_tokens, Some(4));
        assert_eq!(parsed.cache_read_tokens, None);
        assert_eq!(parsed.output_tokens, None);
        assert_eq!(parsed.reasoning_tokens, None);
        assert_eq!(parsed.cache_write_tokens, None);
    }

    #[test]
    fn cw_11_record_inserts_row() {
        let connection = Connection::open_in_memory().unwrap();
        generation(&connection, "gen_usage", "openai");
        let usage = parse_openai_usage(&json!({
            "prompt_tokens": 11,
            "prompt_tokens_details": {"cached_tokens": 2},
            "completion_tokens": 5
        }));
        record(
            &connection,
            "gen_usage",
            "openai",
            Some("fixture"),
            &usage,
            UsageSource::Provider,
            128,
            &UsageTimings {
                ttft_ms: Some(9),
                first_visible_ms: Some(9),
                completed_ms: Some(40),
            },
        )
        .unwrap();
        let (input, cache, source): (i64, i64, String) = connection
            .query_row(
                "SELECT input_tokens, cache_read_tokens, usage_source FROM generation_usage WHERE generation_id='gen_usage'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!((input, cache, source.as_str()), (11, 2, "provider"));
    }

    #[test]
    fn cw_14_summary_ratio_is_null_when_no_input() {
        let connection = Connection::open_in_memory().unwrap();
        generation(&connection, "gen_missing", "openai");
        record(
            &connection,
            "gen_missing",
            "openai",
            None,
            &ProviderUsage::default(),
            UsageSource::Missing,
            10,
            &UsageTimings {
                ttft_ms: None,
                first_visible_ms: None,
                completed_ms: Some(1),
            },
        )
        .unwrap();
        let rows = summary(&connection).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].input_tokens, 0);
        assert!(rows[0].cache_read_ratio.is_none());
        assert_eq!(rows[0].usage_missing, 1);
    }
}
