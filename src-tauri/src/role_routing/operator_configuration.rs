//! Explicit operator-only enablement through the same validation and snapshot path as Settings.
pub fn enable(database: &str) -> Result<String, String> {
    let path = std::path::Path::new(database);
    if !path.is_file() {
        return Err("Operator database does not exist".into());
    }
    let writer = crate::persistence::SqliteWriter::open(path).map_err(|error| error.to_string())?;
    writer.write(|connection| {
    let mut documents = crate::persistence::list_settings_documents(connection)?
        .into_iter()
        .map(|d| crate::SaveSettingsDocumentInput {
            namespace: d.namespace,
            key: d.key,
            schema_version: d.schema_version,
            value_json: d.value_json,
        })
        .collect::<Vec<_>>();
    let providers = crate::persistence::load_model_providers(connection)?.providers;
    let frontend_provider = providers
        .iter()
        .find(|provider| provider.id() == crate::DYNAMIC_LAN_PROVIDER_ID && provider.enabled())
        .ok_or("No enabled LAN reasoning provider is registered")?;
    let reasoner_provider = frontend_provider;
    let document = documents
        .iter_mut()
        .find(|d| d.namespace == "routing.roles" && d.key == "default")
        .ok_or("Role-routing settings missing")?;
    let value = &mut document.value_json;
    // Bootstrap only the pristine empty policy; never replace an operator's actor assignments.
    if value["actors"].as_array().is_some_and(Vec::is_empty)
        && value["recipes"].as_array().is_some_and(Vec::is_empty)
    {
        value["actors"] = serde_json::json!([{"id":"local-reasoner","label":"LAN reasoning provider",
            "transport":"provider","providerId":reasoner_provider.id(),"model":null,"aliases":[],
            "location":"local","resourceGroup":"local-inference","maxInputBytes":65536,
            "capabilities":["reason","tools"]}]);
        value["roles"]["reasoner"] = serde_json::json!("local-reasoner");
        value["recipes"] = serde_json::json!([{"id":"reasoner-response","action":"respond","roles":["reasoner"],"enabled":true}]);
    }
    for (role, capability, provider_id) in [("reasoner", "reason", reasoner_provider.id())] {
        let actor_id = value["roles"][role]
            .as_str()
            .ok_or_else(|| format!("Role-routing {role} is not configured"))?;
        let actor = value["actors"]
            .as_array()
            .and_then(|actors| actors.iter().find(|actor| actor["id"] == actor_id))
            .ok_or_else(|| format!("Role-routing {role} actor is missing"))?;
        let has_capability = actor["capabilities"]
            .as_array()
            .is_some_and(|items| items.iter().any(|item| item == capability));
        if actor["transport"] != "provider" || actor["providerId"] != provider_id || !has_capability
        {
            return Err(format!(
                "Role-routing {role} actor has an incompatible LAN provider binding"
            ));
        }
    }
    // The live LAN path is normally ~2.5 s. Eight seconds avoids turning ordinary jitter into a
    // false configuration failure while preserving a finite, operator-visible deadline.
    value["limits"]["frontendTimeoutMs"] = serde_json::json!(8_000);
    value["enabled"] = serde_json::json!(true);
    crate::persistence::save_settings_documents_to_connection(connection, &documents)?;
    let policy = crate::persistence::load_role_routing_settings(connection)?;
    Ok(format!(
        "enabled={}; actors={}; reasoner={}; frontend={}",
        policy.enabled,
        policy.actors.len(),
        policy.roles.reasoner.as_deref().unwrap_or("unconfigured"),
        policy.roles.frontend.as_deref().unwrap_or("unconfigured")
    ))
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn operator_enable_does_not_create_a_database() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("operator.sqlite3");
        let target = path.to_str().unwrap();
        assert_eq!(
            super::enable(target).unwrap_err(),
            "Operator database does not exist"
        );
        assert!(!path.exists());
    }
}
