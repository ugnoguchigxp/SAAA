use super::*;
pub(super) fn state_provider(name: &str) -> Value {
    let (protocol, endpoint, model) = match name {
        "llm" => (
            "openai.chat-completions.v1",
            "/v1/chat/completions",
            LLM_MODEL,
        ),
        "backchannel" => (
            "openai.chat-completions.v1",
            "/v1/chat/completions",
            "qwen3.5-2b-fast-response",
        ),
        "asr" => (
            "openai.audio-transcriptions.v1",
            "/v1/audio/transcriptions",
            "qwen3-asr-1.7b",
        ),
        "tts" => (
            "openai.audio-speech.v1",
            "/v1/audio/speech",
            "voicevox-core",
        ),
        "embedding" => ("larm.embedding.v1", "/v1/embed", "multilingual-e5-small"),
        _ => ("invalid", "/invalid", "invalid"),
    };
    json!({
        "name": name,
        "capability": if name == "llm" { PROFILE_CAPABILITY } else { "ignored" },
        "route": "route-a",
        "protocol": protocol,
        "endpoint": endpoint,
        "model": model,
        "readiness": "ready",
        "claimable": true
    })
}

pub(in crate::providers::dynamic_lan) fn spawn_json_server(
    build: impl FnOnce(u16) -> Vec<Option<String>>,
) -> (
    std::net::SocketAddr,
    std::thread::JoinHandle<()>,
    mpsc::Receiver<String>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("test listener");
    let address = listener.local_addr().expect("listener address");
    let responses = build(address.port());
    let (captured_tx, captured_rx) = mpsc::channel();
    let server = std::thread::spawn(move || {
        for (index, response) in responses.into_iter().enumerate() {
            let (mut stream, _) = listener.accept().expect("request accepted");
            let request = read_request(&mut stream);
            captured_tx.send(request).expect("request captured");
            match response {
                Some(body) => write_response(
                    &mut stream,
                    if index == 1 { "201 Created" } else { "200 OK" },
                    "application/json",
                    &body,
                ),
                None => write_response(&mut stream, "204 No Content", "", ""),
            }
        }
    });
    (address, server, captured_rx)
}

pub(super) fn ready_health() -> String {
    json!({
        "ready": true,
        "acceptingRequests": true,
        "capacity": {
            "maxConcurrentRequests": 1,
            "activeRequests": 0,
            "maxQueuedRequests": 2,
            "queueDepth": 0,
            "queueTimeoutMs": 5000,
            "retryAfterMs": 100,
            "completionGuaranteed": false
        }
    })
    .to_string()
}
