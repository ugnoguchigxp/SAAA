//! Local deterministic provider server, isolated from real-model acceptance.
#[path = "request_options.rs"]
mod request_options;
use super::*;

pub(super) async fn serve(
    State((fixture, base)): State<(Arc<Fixture>, String)>,
    request: Request,
) -> Response {
    let path = request.uri().path().to_string();
    let method = request.method().clone();
    fixture
        .calls
        .lock()
        .expect("fixture calls lock")
        .push(format!("{method} {path}"));
    if path == "/v3/agent-profiles" {
        return Json(json!({"contractVersion":"agent-connection.v3","catalogRevision":"queue-e2e",
            "audiences":["saaa-desktop"],
            "requestedProfile":"SAAA","profiles":[{"id":"saaa-conversation-ornith15",
            "providers":[declaration("asr"),declaration("llm"),declaration("tts"),declaration("embedding")],"services":[]}]})).into_response();
    }
    if path.starts_with("/v1/agent-connections") {
        if request
            .headers()
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            != Some("Bearer fixture-control-token")
        {
            return StatusCode::UNAUTHORIZED.into_response();
        }
        if method == Method::DELETE {
            return StatusCode::NO_CONTENT.into_response();
        }
        if path.ends_with("/claim") {
            return Json(claim_value(&base)).into_response();
        }
        if method == Method::POST {
            return (StatusCode::CREATED, Json(state_value())).into_response();
        }
        return Json(state_value()).into_response();
    }
    if path.ends_with("/health") {
        return Json(json!({"ready":true,"acceptingRequests":true,
            "capacity":{"maxConcurrentRequests":4,"activeRequests":0,"maxQueuedRequests":4,
            "queueDepth":0,"queueTimeoutMs":1000,"retryAfterMs":0,"completionGuaranteed":false},
            "probe":{"validated":true,"protocol":if path.starts_with("/asr/") {"openai.audio-transcriptions.v1"} else if path.starts_with("/tts/") {"openai.audio-speech.v1"} else if path.starts_with("/embedding/") {"larm.embedding.v1"} else {"openai.chat-completions.v1"}}})).into_response();
    }
    if path == "/asr/v1/audio/transcriptions" {
        let bytes = to_bytes(request.into_body(), 1_000_000)
            .await
            .expect("ASR fixture body");
        if !bytes.windows(4).any(|part| part == b"RIFF") {
            return StatusCode::BAD_REQUEST.into_response();
        }
        if fixture.asr_no_speech.load(Ordering::SeqCst) {
            return Json(json!({"text":"こんにちは。","language":"ja",
                "segments":[{"no_speech_prob":0.95}]}))
            .into_response();
        }
        return Json(json!({"text":"今日の事実を調べて","language":"ja"})).into_response();
    }
    if path == "/rejected/v1/chat/completions" {
        return if fixture.cloud_auth_rejection.load(Ordering::SeqCst) {
            StatusCode::UNAUTHORIZED
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        }
        .into_response();
    }
    if path.ends_with("/v1/chat/completions") || path.ends_with("/v1/messages") {
        if fixture.cloud_deadline.load(Ordering::SeqCst) {
            tokio::time::sleep(std::time::Duration::from_millis(400)).await;
        }
        let bytes = to_bytes(request.into_body(), 1_000_000)
            .await
            .expect("LLM fixture body");
        let mut body: Value = serde_json::from_slice(&bytes).expect("LLM fixture JSON");
        let native = path.ends_with("/v1/messages");
        if native {
            request_options::normalize_native(&mut body);
        }
        if path.starts_with("/llm/") {
            fixture
                .requests
                .lock()
                .expect("fixture request lock")
                .push(body.clone());
        }
        if let Some(response) = context_trial::transport_response(&fixture, &path, &body) {
            return response;
        }
        let serialized = body.to_string();
        let content = if let Some(content) = context_trial::respond(&path, &body) {
            content
        } else if let Some(content) = worker::respond(&fixture, &body) {
            content
        } else if let Some(content) = dictionary::respond(&path, &body) {
            content
        } else if let Some(content) = cancellation::respond(&fixture, &path, &body).await {
            content
        } else if path.starts_with("/llm/") {
            if fixture.authentication_failure.load(Ordering::SeqCst) {
                *fixture.llm_calls.lock().expect("LLM fixture count") += 1;
                return StatusCode::UNAUTHORIZED.into_response();
            }
            let first_call = *fixture.llm_calls.lock().expect("LLM fixture count") == 0;
            if first_call && fixture.slow_ornith.load(Ordering::SeqCst) {
                tokio::time::sleep(std::time::Duration::from_millis(11_000)).await;
            }
            let mut count = fixture.llm_calls.lock().expect("LLM fixture count");
            if fixture.cloud_options.load(Ordering::SeqCst) {
                request_options::validate(&body);
            } else {
                if *count == 0 {
                    assert_eq!(
                        body["max_tokens"], 4_096,
                        "initial Ornith call keeps its output reserve"
                    );
                } else {
                    assert_eq!(
                        body["max_tokens"], 4_096,
                        "tool follow-up keeps the advertised output reserve"
                    );
                    if serialized.contains("TOOL_RESULT:") {
                        assert_ne!(body["chat_template_kwargs"]["enable_thinking"], false);
                    }
                }
            }
            if *count > 0 && fixture.fail_after_search.load(Ordering::SeqCst) {
                *count += 1;
                if body["stream"] == true {
                    let event = json!({"model":body["model"],"choices":[{"index":0,"delta":{},"finish_reason":"length"}]});
                    return (
                        [(header::CONTENT_TYPE, "text/event-stream")],
                        format!("data: {event}\n\ndata: [DONE]\n\n"),
                    )
                        .into_response();
                }
                return Json(
                    json!({"choices":[{"message":{"content":""},"finish_reason":"length"}]}),
                )
                .into_response();
            }
            assert!(
                serialized.contains(&chrono::Local::now().format("%Y-%m-%d").to_string()),
                "runtime date reaches Ornith"
            );
            let current_user_text = body["messages"]
                .as_array()
                .and_then(|messages| {
                    messages
                        .iter()
                        .rev()
                        .find(|message| message["role"] == "user")
                })
                .and_then(|message| message["content"].as_str())
                .unwrap_or_default();
            let result = if current_user_text == "こんにちは" {
                json!({"action":"answer","content":"こんにちは。","sources":[]}).to_string()
            } else {
                match *count {
                    0 => json!({"action":"web_search","query":"fixture fact"}).to_string(),
                    1 => {
                        assert!(
                            body["messages"]
                                .as_array()
                                .unwrap()
                                .iter()
                                .any(|m| m["role"] == "assistant"
                                    && m["content"]
                                        .as_str()
                                        .is_some_and(|s| s.contains("web_search"))),
                            "search action is retained in dialogue"
                        );
                        json!({"action":"fetch_content","url":"https://example.invalid/other"})
                            .to_string()
                    }
                    2 => {
                        assert!(serialized.contains("FETCH_FAILED"));
                        json!({"action":"fetch_content","url":"https://example.invalid/report"})
                            .to_string()
                    }
                    _ => {
                        let fetched = body["messages"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .rev()
                            .filter_map(|m| m["content"].as_str())
                            .find(|s| s.starts_with("[TOOL_RESULT: fetch_content"))
                            .unwrap();
                        let document: Value =
                            serde_json::from_str(fetched.split_once('\n').unwrap().1)
                                .expect("tool JSON must not be cut in half");
                        assert!(document["document"]["text"]
                            .as_str()
                            .unwrap()
                            .contains("確認済みの事実は42"));
                        let answer = if fixture.invalid_reply.load(Ordering::SeqCst) {
                            "保存してはいけない回答<think>制御文</think>"
                        } else {
                            "資料では確認済みの事実は42です。"
                        };
                        json!({"action":"answer","content":answer,"sources":["https://example.invalid/report"]}).to_string()
                    }
                }
            };
            *count += 1;
            result
        } else if serialized.contains("会話の入口") {
            assert_eq!(body["max_tokens"], 4_096);
            json!({"route":"think","reply":null}).to_string()
        } else if serialized.contains("挨拶に自然な日本語") {
            "こんにちは。".into()
        } else {
            assert_eq!(body["max_tokens"], 4_096);
            assert!(serialized.contains("ORNITH_RESULT"));
            assert!(
                !serialized.contains("古い天気は雨です"),
                "Qwen speaker must not read stale conversation replies"
            );
            if fixture.authentication_failure.load(Ordering::SeqCst) {
                assert!(serialized.contains("Provider authentication failed"));
                "接続の認証に失敗したため回答できませんでした。".into()
            } else if fixture.invalid_reply.load(Ordering::SeqCst) {
                "保存してはいけない回答<think>制御文</think>".into()
            } else if fixture.fail_after_search.load(Ordering::SeqCst) {
                assert!(
                    serialized.contains("結果を整理する段階で失敗"),
                    "Qwen must read the Ornith failure"
                );
                "調査結果を確定できませんでした。".into()
            } else {
                assert!(
                    serialized.contains("確認済みの事実は42"),
                    "Qwen must read Ornith result"
                );
                "調査結果は42です。補足です。これ以上の説明は不要です。".into()
            }
        };
        if native {
            return Json(json!({"id":"msg_fixture","type":"message","role":"assistant","model":body["model"],"stop_reason":"end_turn","content":[{"type":"text","text":content}],"usage":{"input_tokens":20,"output_tokens":30}})).into_response();
        }
        if body["stream"] == true {
            let model = body["model"].as_str().unwrap_or("fixture-ornith");
            if let Some(split_at) = content.find("資料では確認済みの事実は42です。")
            {
                let split_at = split_at + "資料では確認済みの事実は42です。".len();
                let first_event = json!({"model":model,"choices":[{"index":0,"delta":{"content":&content[..split_at]},"finish_reason":null}]});
                let second_event = json!({"model":model,"choices":[{"index":0,"delta":{"content":&content[split_at..]},"finish_reason":"stop"}]});
                let first = format!("data: {first_event}\n\n");
                let second = format!("data: {second_event}\n\ndata: [DONE]\n\n");
                let before_done = fixture.clone();
                let chunks = futures_util::stream::once(async move {
                    Ok::<_, std::io::Error>(axum::body::Bytes::from(first))
                })
                .chain(futures_util::stream::once(async move {
                    let first_speech =
                        tokio::time::timeout(std::time::Duration::from_secs(2), async {
                            loop {
                                if before_done
                                    .spoken
                                    .lock()
                                    .expect("fixture spoken lock")
                                    .iter()
                                    .any(|text| text == "資料では確認済みの事実は42です。")
                                {
                                    break;
                                }
                                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                            }
                        })
                        .await
                        .is_ok();
                    before_done
                        .speech_before_done
                        .store(first_speech, Ordering::SeqCst);
                    Ok::<_, std::io::Error>(axum::body::Bytes::from(second))
                }));
                return (
                    [(header::CONTENT_TYPE, "text/event-stream")],
                    axum::body::Body::from_stream(chunks),
                )
                    .into_response();
            }
            let event = json!({"model":model,"choices":[{"index":0,"delta":{"content":content},"finish_reason":"stop"}]});
            let stream = format!("data: {event}\n\ndata: [DONE]\n\n");
            return ([(header::CONTENT_TYPE, "text/event-stream")], stream).into_response();
        }
        return Json(json!({"choices":[{"message":{"content":content},"finish_reason":"stop"}]}))
            .into_response();
    }
    StatusCode::NOT_FOUND.into_response()
}
