use super::*;

#[tokio::test]
async fn warm_system_one_and_cold_image_contract_are_kept_separate() {
    let mut response: Value = serde_json::from_str(&saaa_selector_catalog()).unwrap();
    response["requestedProfile"] = json!("SAAA-w-Image");
    let profile = &mut response["profiles"][0];
    let providers = profile["providers"].as_array_mut().unwrap();
    providers.retain(|provider| provider["name"] != "backchannel");
    providers.push(json!({"name":"systemone","capability":"system.one","protocol":"larm.system-one.v1","endpoint":"/v1/systemone","model":"laya-fixture"}));
    let service = json!({"name":"image","capability":"media.image.generate","protocol":"larm.image-generation.v1","endpoint":"/public/discovered-image","model":"image-fixture","startupPolicy":{"minWarmInstances":0,"idleTtlSeconds":120}});
    profile["services"] = json!([service]);
    let (address, server, captured) = spawn_json_server(|_| vec![Some(response.to_string())]);
    let catalog = saaa_larm_session::catalog::fetch(
        &reqwest::Client::new(),
        &Url::parse(&format!("http://{address}/")).unwrap(),
        "catalog-token",
        "SAAA-w-Image",
    )
    .await
    .unwrap();
    let selected = selected_llm_from_catalog(&catalog).unwrap();
    assert!(selected.catalog_services[0].starts_on_request());
    let (created, expires) = test_timestamps();
    let mut state = connection_state_json("aconn_test", "ready", AUDIENCE, &created, &expires);
    state["profile"] = json!("SAAA-w-Image");
    state["services"] = json!([service]);
    state["services"][0]["readiness"] = json!("stopped");
    let providers = state["providers"].as_array_mut().unwrap();
    providers.retain(|provider| provider["name"] != "backchannel");
    providers.push(json!({"name":"systemone","capability":"system.one","protocol":"larm.system-one.v1","endpoint":"/v1/systemone","model":"laya-fixture","readiness":"ready","claimable":true}));
    let state_full: ConnectionState = serde_json::from_value(state.clone()).unwrap();
    validate_state_shape(&state_full, AUDIENCE, &selected).unwrap();
    state["providers"]
        .as_array_mut()
        .unwrap()
        .retain(|provider| provider["name"] == "llm");
    let state_llm: ConnectionState = serde_json::from_value(state).unwrap();
    validate_state_shape(&state_llm, AUDIENCE, &selected).unwrap();
    server.join().unwrap();
    assert_eq!(captured.try_iter().count(), 1);
}
