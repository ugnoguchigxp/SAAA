//! Explicit operator-only enablement through the same validation and snapshot path as Settings.
pub fn enable(database: &str) -> Result<String, String> {
    let mut connection = rusqlite::Connection::open_with_flags(
        database,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
    )
    .map_err(crate::database_error)?;
    connection
        .busy_timeout(std::time::Duration::from_secs(5))
        .map_err(crate::database_error)?;
    let mut documents = crate::persistence::list_settings_documents(&connection)?
        .into_iter()
        .map(|d| crate::SaveSettingsDocumentInput {
            namespace: d.namespace,
            key: d.key,
            schema_version: d.schema_version,
            value_json: d.value_json,
        })
        .collect::<Vec<_>>();
    let provider = crate::persistence::load_model_providers(&connection)?
        .providers
        .into_iter()
        .find(|p| p.id() == crate::DYNAMIC_LAN_PROVIDER_ID && p.enabled())
        .ok_or("No enabled LAN reasoning provider is registered")?;
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
            "transport":"provider","providerId":provider.id(),"model":null,"aliases":[],
            "location":"local","resourceGroup":"local-inference","maxInputBytes":65536,
            "capabilities":["reason","tools"]}]);
        value["roles"]["reasoner"] = serde_json::json!("local-reasoner");
        value["recipes"] = serde_json::json!([{"id":"reasoner-response","action":"respond","roles":["reasoner"],"enabled":true}]);
    }
    if value["roles"]["frontend"].is_null() {
        let actors = value["actors"]
            .as_array_mut()
            .ok_or("Role-routing actors are invalid")?;
        let frontend_id = "local-conversation-frontend";
        if let Some(actor) = actors.iter().find(|actor| actor["id"] == frontend_id) {
            let capabilities = actor["capabilities"].as_array();
            if actor["transport"] != "provider"
                || actor["providerId"] != provider.id()
                || !capabilities
                    .is_some_and(|items| items.iter().any(|item| item == "social_reply"))
            {
                return Err(
                    "Existing conversation frontend actor has an incompatible binding".into(),
                );
            }
        } else {
            actors.push(serde_json::json!({
                "id":frontend_id,
                "label":"Harness conversation frontend",
                "transport":"provider",
                "providerId":provider.id(),
                "model":null,
                "aliases":["LFM"],
                "location":"local",
                "resourceGroup":"harness-backchannel",
                "maxInputBytes":16000,
                "capabilities":["social_reply","classify"]
            }));
        }
        value["roles"]["frontend"] = serde_json::json!(frontend_id);
    }
    for (role, capability) in [("frontend", "social_reply"), ("reasoner", "reason")] {
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
        if actor["transport"] != "provider"
            || actor["providerId"] != provider.id()
            || !has_capability
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
    crate::persistence::save_settings_documents_to_connection(&mut connection, &documents)?;
    let policy = crate::persistence::load_role_routing_settings(&connection)?;
    Ok(format!(
        "enabled={}; actors={}; reasoner={}; frontend={}",
        policy.enabled,
        policy.actors.len(),
        policy.roles.reasoner.as_deref().unwrap_or("unconfigured"),
        policy.roles.frontend.as_deref().unwrap_or("unconfigured")
    ))
}
