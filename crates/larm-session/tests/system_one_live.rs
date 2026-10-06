#[tokio::test]
#[ignore = "requires LARM_API_TOKEN and SAAA_LAYA_TEST_ADDRESS"]
async fn system_one_subset_connects_and_releases() {
    use saaa_larm_session::{ProfilePreference, ProfileVariant, Session};
    let base = std::env::var("SAAA_LAYA_TEST_ADDRESS").unwrap();
    let token = std::env::var("LARM_API_TOKEN").unwrap();
    let (_tx, rx) = tokio::sync::watch::channel(false);
    let result = Session::connect_with_profile_credential_key_phase_and_providers(
        &base,
        ProfilePreference::Variant(ProfileVariant::Image),
        token,
        format!("saaa-laya-test-{}", uuid::Uuid::new_v4().simple()),
        rx,
        None,
        Some(vec!["system-one"]),
    )
    .await;
    let session = match result {
        Ok(s) => s,
        Err(e) => {
            let code = e.code;
            if let Some(s) = e.cleanup {
                let _ = s.close().await;
            }
            panic!("{code}")
        }
    };
    assert_eq!(
        session.provider_names().await,
        vec!["system-one".to_string()]
    );
    let acquired = session.acquire("system-one").await;
    let outcome = acquired
        .as_ref()
        .map(|u| u.provider().protocol.clone())
        .map_err(|e| *e);
    drop(acquired);
    session.close().await.unwrap();
    assert_eq!(outcome.unwrap(), "larm.system-one.v1");
}
