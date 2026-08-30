use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};

use crate::{now_iso, DYNAMIC_LAN_PROVIDER_ID};

pub(crate) fn migrate_dynamic_lan_provider_identity(
    connection: &Connection,
) -> rusqlite::Result<()> {
    const REGRESSED_PROVIDER_ID: &str = "dynamic-lan";
    let current: Option<(String, String)> = connection
        .query_row(
            "SELECT providers.value_json, routing.value_json
             FROM settings_documents AS providers
             JOIN settings_documents AS routing
               ON routing.namespace='routing.tasks' AND routing.key='default'
             WHERE providers.namespace='providers.model' AND providers.key='default'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let Some((providers_text, routing_text)) = current else {
        return Ok(());
    };
    let mut providers: Value = serde_json::from_str(&providers_text).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(error))
    })?;
    let mut routing: Value = serde_json::from_str(&routing_text).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, Box::new(error))
    })?;
    let Some(items) = providers.get_mut("providers").and_then(Value::as_array_mut) else {
        return Ok(());
    };
    if items
        .iter()
        .any(|item| item.get("id").and_then(Value::as_str) == Some(DYNAMIC_LAN_PROVIDER_ID))
    {
        return Ok(());
    }
    let Some(provider) = items.iter_mut().find(|item| {
        item.get("id").and_then(Value::as_str) == Some(REGRESSED_PROVIDER_ID)
            && item.get("kind").and_then(Value::as_str) == Some("dynamic-lan")
    }) else {
        return Ok(());
    };
    provider["id"] = json!(DYNAMIC_LAN_PROVIDER_ID);
    if routing.pointer("/conversationRespond/primaryProviderId")
        == Some(&json!(REGRESSED_PROVIDER_ID))
    {
        routing["conversationRespond"]["primaryProviderId"] = json!(DYNAMIC_LAN_PROVIDER_ID);
    }
    if let Some(fallbacks) = routing
        .pointer_mut("/conversationRespond/fallbackProviderIds")
        .and_then(Value::as_array_mut)
    {
        for fallback in fallbacks {
            if fallback.as_str() == Some(REGRESSED_PROVIDER_ID) {
                *fallback = json!(DYNAMIC_LAN_PROVIDER_ID);
            }
        }
    }
    let updated_at = now_iso();
    connection.execute(
        "UPDATE settings_documents SET value_json=?1, updated_at=?2
         WHERE namespace='providers.model' AND key='default'",
        params![providers.to_string(), updated_at],
    )?;
    connection.execute(
        "UPDATE settings_documents SET value_json=?1, updated_at=?2
         WHERE namespace='routing.tasks' AND key='default'",
        params![routing.to_string(), updated_at],
    )?;
    Ok(())
}
