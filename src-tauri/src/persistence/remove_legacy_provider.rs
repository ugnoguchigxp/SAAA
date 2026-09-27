//! Remove settings for the deleted allocation/WebSocket client without silently
//! selecting a different network destination. Called inside the startup transaction.
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;
use std::collections::HashSet;

pub(super) fn migrate(connection: &Connection) -> rusqlite::Result<()> {
    let Some(mut providers) = read(connection, "providers.model")? else {
        return Ok(());
    };
    let Some(items) = providers.get_mut("providers").and_then(Value::as_array_mut) else {
        return Ok(());
    };
    let removed: HashSet<String> = items
        .iter()
        .filter(|item| item["kind"] == "larm")
        .filter_map(|item| item["id"].as_str().map(str::to_string))
        .collect();
    let before = items.len();
    items.retain(|item| item["kind"] != "larm");
    if before == items.len() {
        return Ok(());
    }
    write(connection, "providers.model", &providers)?;
    if let Some(mut routing) = read(connection, "routing.tasks")? {
        let route = &mut routing["conversationRespond"];
        if route["primaryProviderId"]
            .as_str()
            .is_some_and(|id| removed.contains(id))
        {
            route["primaryProviderId"] = Value::Null;
            // A missing primary must not turn an old fallback into an implicit primary.
            route["fallbackProviderIds"] = serde_json::json!([]);
        } else if let Some(ids) = route["fallbackProviderIds"].as_array_mut() {
            ids.retain(|id| !id.as_str().is_some_and(|id| removed.contains(id)));
        }
        write(connection, "routing.tasks", &routing)?;
    }
    Ok(())
}

fn read(connection: &Connection, namespace: &str) -> rusqlite::Result<Option<Value>> {
    let raw: Option<String> = connection
        .query_row(
            "SELECT value_json FROM settings_documents WHERE namespace=?1 AND key='default'",
            [namespace],
            |row| row.get(0),
        )
        .optional()?;
    raw.map(|raw| {
        serde_json::from_str(&raw).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })
    })
    .transpose()
}
fn write(connection: &Connection, namespace: &str, value: &Value) -> rusqlite::Result<()> {
    connection.execute("UPDATE settings_documents SET value_json=?1, updated_at=?2 WHERE namespace=?3 AND key='default'",
        params![value.to_string(), crate::now_iso(), namespace])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn legacy_only_settings_reopen_without_selecting_a_cloud_fallback() {
        let connection = Connection::open_in_memory().unwrap();
        crate::initialize_database(&connection).unwrap();
        let mut providers = read(&connection, "providers.model").unwrap().unwrap();
        providers["providers"] = json!([{ "kind":"larm", "id":"old-larm" }]);
        write(&connection, "providers.model", &providers).unwrap();
        let mut routing = read(&connection, "routing.tasks").unwrap().unwrap();
        routing["voiceSpeak"]["source"] = json!("harness");
        routing["voiceSpeak"]["providerId"] = Value::Null;
        routing["conversationRespond"]["source"] = json!("provider");
        routing["conversationRespond"]["primaryProviderId"] = json!("old-larm");
        routing["conversationRespond"]["fallbackProviderIds"] = json!(["cloud"]);
        write(&connection, "routing.tasks", &routing).unwrap();
        let mut roles = read(&connection, "routing.roles").unwrap().unwrap();
        roles["enabled"] = json!(false);
        write(&connection, "routing.roles", &roles).unwrap();
        crate::initialize_database(&connection).unwrap();
        assert!(crate::persistence::load_model_providers(&connection)
            .unwrap()
            .providers
            .is_empty());
        let route = crate::persistence::load_routing_settings(&connection)
            .unwrap()
            .conversation_respond;
        assert_eq!(route.source, "provider");
        assert!(route.primary_provider_id.is_none());
        assert!(route.fallback_provider_ids.is_empty());
        let before = read(&connection, "providers.model").unwrap();
        crate::initialize_database(&connection).unwrap();
        assert_eq!(read(&connection, "providers.model").unwrap(), before);
        let state = crate::test_support::app_state(connection);
        let snapshot = crate::persistence::app_commands::get_app_snapshot(&state).unwrap();
        assert!(snapshot.effective_route.provider_id.is_none());
    }

    #[test]
    fn removes_only_legacy_fallbacks_and_preserves_http_provider_configuration() {
        let connection = Connection::open_in_memory().unwrap();
        crate::initialize_database(&connection).unwrap();
        let mut providers = read(&connection, "providers.model").unwrap().unwrap();
        let original = providers["providers"].clone();
        providers["providers"]
            .as_array_mut()
            .unwrap()
            .push(json!({"kind":"larm","id":"old"}));
        write(&connection, "providers.model", &providers).unwrap();
        let mut routing = read(&connection, "routing.tasks").unwrap().unwrap();
        routing["voiceSpeak"]["source"] = json!("harness");
        routing["voiceSpeak"]["providerId"] = Value::Null;
        routing["conversationRespond"]["source"] = json!("provider");
        routing["conversationRespond"]["primaryProviderId"] = json!("custom-primary");
        routing["conversationRespond"]["fallbackProviderIds"] = json!(["custom-fallback", "old"]);
        write(&connection, "routing.tasks", &routing).unwrap();
        migrate(&connection).unwrap();
        assert_eq!(
            read(&connection, "providers.model").unwrap().unwrap()["providers"],
            original
        );
        let route = &read(&connection, "routing.tasks").unwrap().unwrap()["conversationRespond"];
        assert_eq!(route["primaryProviderId"], "custom-primary");
        assert_eq!(route["fallbackProviderIds"], json!(["custom-fallback"]));
    }
}
