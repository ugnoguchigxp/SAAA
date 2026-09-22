//! Explicit opt-in real LAN smoke: always release before asserting request outcomes.
use saaa_larm_session::Session;
use serde_json::json;
use std::time::Duration;
#[tokio::test]
#[ignore = "starts the existing-only SAAA LARM profile on the LAN"]
async fn live_four_provider_session() {
    let base = std::env::var("SAAA_LARM_CONTROL_URL")
        .unwrap_or_else(|_| "http://gnosis.local:9810".into());
    let (_stop, receiver) = tokio::sync::watch::channel(false);
    let session = Session::connect(&base, receiver)
        .await
        .expect("connect and health all four providers");
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(45))
        .build()
        .unwrap();
    let result=async {
        for name in ["llm"] {
            let lease=session.acquire(name).await?; let p=lease.provider();
            let value:serde_json::Value=client.post(p.endpoint("chat/completions")?).bearer_auth(p.token())
                .json(&json!({"model":p.model,"messages":[{"role":"user","content":"Reply with the single word OK."}],"stream":false,"max_tokens":64}))
                .send().await.map_err(|_|"chat transport")?.error_for_status().map_err(|_|"chat status")?
                .json().await.map_err(|_|"chat JSON")?;
            if value["choices"][0]["message"]["content"].as_str().is_none_or(|s|s.is_empty()) { return Err("empty chat content"); }
            eprintln!("{name}: completion received");
        }
        let lease=session.acquire("embedding").await?; let p=lease.provider();
        let value:serde_json::Value=client.post(p.endpoint("embed")?).bearer_auth(p.token())
            .json(&json!({"texts":["接続確認"],"type":"query","normalize":true,"priority":"normal"}))
            .send().await.map_err(|_|"embedding transport")?.error_for_status().map_err(|_|"embedding status")?
            .json().await.map_err(|_|"embedding JSON")?;
        if value["dimension"].as_u64()!=p.embedding_space.map(|space|space.dimension as u64) { return Err("invalid embedding dimension"); }
        drop(lease);
        eprintln!("embedding: vector received");
        let lease=session.acquire("tts").await?; let p=lease.provider();
        let audio=client.post(p.endpoint("audio/speech")?).bearer_auth(p.token())
            .json(&json!({"model":p.model,"input":"接続を確認しました。","voice":"Kasukabe_Tsumugi","response_format":"wav"}))
            .send().await.map_err(|_|"tts transport")?.error_for_status().map_err(|_|"tts status")?
            .bytes().await.map_err(|_|"tts bytes")?;
        if !audio.starts_with(b"RIFF") || audio.len()>16*1024*1024 { return Err("invalid WAV"); }
        drop(lease);
        eprintln!("tts: WAV received");
        let lease=session.acquire("asr").await?; let p=lease.provider();
        let form=reqwest::multipart::Form::new().part("file", reqwest::multipart::Part::bytes(audio.to_vec()).file_name("test.wav").mime_str("audio/wav").unwrap())
            .text("model",p.model.clone()).text("response_format","json");
        let value:serde_json::Value=client.post(p.endpoint("audio/transcriptions")?).bearer_auth(p.token()).multipart(form)
            .send().await.map_err(|_|"asr transport")?.error_for_status().map_err(|_|"asr status")?
            .json().await.map_err(|_|"asr JSON")?;
        if value["text"].as_str().is_none_or(|s|s.trim().is_empty()) { return Err("empty transcription"); }
        eprintln!("asr: transcription received");
        let vectors=session.embed_query(&["接続確認".into()]).await?;
        if vectors.len()!=1 || vectors[0].is_empty() { return Err("empty embedding"); }
        eprintln!("embedding: vector received");
        Ok::<_, &'static str>(())
    }.await;
    let invalidated = match session.acquire("llm").await {
        Ok(lease) => Ok((
            lease.provider().endpoint("chat/completions").unwrap(),
            lease.provider().token().to_string(),
            lease.provider().model.clone(),
        )),
        Err(error) => Err(error),
    };
    session.close().await.expect("release");
    eprintln!("connection: released");
    result.expect("all four providers respond");
    let invalidated = invalidated.expect("lease for revocation check");
    let status=client.post(invalidated.0).bearer_auth(invalidated.1).json(&json!({"model":invalidated.2,"messages":[{"role":"user","content":"OK"}],"stream":false}))
        .send().await.unwrap().status();
    assert_eq!(status.as_u16(), 401, "token must be invalid after release");
}

#[tokio::test]
#[ignore = "reclaims an unreleased desktop connection after a simulated restart"]
async fn live_restart_reclaims_connection() {
    let base = std::env::var("SAAA_LARM_CONTROL_URL")
        .unwrap_or_else(|_| "http://gnosis.local:9810".into());
    let token = std::env::var("LARM_API_TOKEN").expect("LARM_API_TOKEN");
    let key = format!("saaa-voice-live-{}", uuid::Uuid::new_v4().simple());
    let (_first_stop, first_receiver) = tokio::sync::watch::channel(false);
    let first = Session::connect_with_profile_credential_and_key(
        &base,
        saaa_larm_session::DEFAULT_PROFILE,
        token.clone(),
        key.clone(),
        first_receiver,
    )
    .await
    .expect("first desktop process connects");
    let first_allocation = first
        .acquire("llm")
        .await
        .expect("first lease")
        .allocation_id()
        .to_string();
    std::mem::forget(first); // Simulate termination before the release handler runs.

    let (_second_stop, second_receiver) = tokio::sync::watch::channel(false);
    let second = Session::connect_with_profile_credential_and_key(
        &base,
        saaa_larm_session::DEFAULT_PROFILE,
        token,
        key,
        second_receiver,
    )
    .await
    .expect("restarted desktop reclaims the connection");
    let second_allocation = second
        .acquire("llm")
        .await
        .expect("reclaimed lease")
        .allocation_id()
        .to_string();
    assert_eq!(second_allocation, first_allocation);
    second.close().await.expect("reclaimed connection releases");
}
