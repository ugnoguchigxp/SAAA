use super::*;

#[test]
fn episode_fetch_and_time_filters_preserve_identity_without_model_scope_overrides() {
    assert!(parse_arguments("fetch_episode", r#"{"id":"episode","sourceKey":"version"}"#).is_ok());
    assert!(parse_arguments(
        "fetch_episode",
        r#"{"id":"episode","sourceKey":"version","personalScopes":["foreign"]}"#
    )
    .is_err());
    assert!(parse_arguments(
        "search_episodes",
        r#"{"query":"京都","eventFrom":"2024-01","eventUntil":"2025"}"#
    )
    .is_ok());
    let remote = json!({"content":[{"type":"text","text":json!({"items":[{"id":"episode","sourceKey":"version","sourceContract":{"contractVersion":1,"sources":[]},"eventTime":{"start":"2024","precision":"year"}}]}).to_string()}]});
    let value: Value =
        serde_json::from_str(&compact_result("search_episodes", &remote).unwrap()).unwrap();
    assert_eq!(value["items"][0]["id"], "episode");
    assert_eq!(value["items"][0]["eventTime"]["precision"], "year");
}
