//! LFM classifies the latest utterance. User-visible text is always host-owned.
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ConversationDecision {
    pub say: Option<String>,
    pub think: bool,
    pub reply_key: Option<&'static str>,
    #[serde(skip, default)]
    pub classifier_failure: Option<&'static str>,
    #[serde(skip, default)]
    pub structured_output_fallback: bool,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FrontdeskClassification {
    route: Route,
    reply_key: ReplyKey,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum Route {
    Delegate,
    SimpleReply,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum ReplyKey {
    None,
    Greeting,
    Acknowledgement,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReasoningNeed {
    think: bool,
}

const INSTRUCTION: &str = "あなたは音声受付の分類器です。ユーザー音声は1.5秒の無音ごとに届き、無音は依頼完了を意味しません。最新発言が完全な挨拶だけならroute=simple_reply, replyKey=greeting、完全なお礼だけならroute=simple_reply, replyKey=acknowledgementにします。話の途中、相槌、曖昧な発言はroute=simple_reply, replyKey=noneにします。具体的でまとまった依頼として調査・推論・計算・ツール実行を開始できる場合だけroute=delegate, replyKey=noneにします。pendingReasoning=trueならdelegateにしません。自由文、回答、質問、説明、思考タグは禁止です。JSON {\"route\":\"delegate\"または\"simple_reply\",\"replyKey\":\"none\"または\"greeting\"または\"acknowledgement\"} だけを返してください。";

pub(crate) async fn decide(
    ready: &super::Ready,
    history: Vec<Value>,
    pending_reasoning: bool,
    already_greeted: bool,
) -> Result<ConversationDecision, &'static str> {
    let (lfm, qwen) = tokio::join!(
        respond_with_lfm(ready, history.clone(), pending_reasoning, already_greeted),
        async {
            tokio::time::timeout(
                std::time::Duration::from_millis(2_500),
                classify_with_qwen(ready, history.clone(), pending_reasoning),
            )
            .await
            .map_err(|_| "qwen-classifier-timeout")?
        },
    );
    let (classification, structured_output_fallback) = lfm?;
    let current_input = history
        .iter()
        .rev()
        .find(|message| message["role"] == "user")
        .and_then(|message| message["content"].as_str())
        .unwrap_or_default();
    Ok(apply_reasoning_need(
        classification,
        qwen.map(|need| need.think),
        pending_reasoning,
        already_greeted,
        current_input,
        structured_output_fallback,
    ))
}

fn state_note(pending_reasoning: bool, already_greeted: bool) -> String {
    json!({
        "pendingReasoning": pending_reasoning,
        "alreadyGreeted": already_greeted,
    })
    .to_string()
}

fn apply_reasoning_need(
    classification: FrontdeskClassification,
    qwen: Result<bool, &'static str>,
    pending_reasoning: bool,
    already_greeted: bool,
    current_input: &str,
    structured_output_fallback: bool,
) -> ConversationDecision {
    // LFM can always request reasoning. Qwen's parallel classification is a promotion-only safety
    // net: it can recover a missed request, but can never cancel LFM's request. If that safety net
    // is unavailable, only an exact host-owned greeting or acknowledgement may complete locally;
    // every other utterance is delegated rather than silently lost. A pending request is fenced in
    // the host so no model can launch it twice.
    let mut classifier_failure = None;
    let host_reply = allowed_host_reply(&classification, current_input, already_greeted);
    let think = if pending_reasoning {
        false
    } else {
        match qwen {
            Ok(reasoning) => classification.route == Route::Delegate || reasoning,
            Err(code) => {
                classifier_failure = Some(code);
                classification.route == Route::Delegate || host_reply.is_none()
            }
        }
    };
    if think {
        return ConversationDecision {
            say: Some("少々お待ちください、考えます。".into()),
            think: true,
            reply_key: Some("thinking"),
            classifier_failure,
            structured_output_fallback,
        };
    }
    let (say, reply_key) =
        host_reply.map_or((None, None), |(key, text)| (Some(text.into()), Some(key)));
    ConversationDecision {
        say,
        think: false,
        reply_key,
        classifier_failure,
        structured_output_fallback,
    }
}

fn allowed_host_reply(
    classification: &FrontdeskClassification,
    input: &str,
    already_greeted: bool,
) -> Option<(&'static str, &'static str)> {
    if classification.route != Route::SimpleReply {
        return None;
    }
    match (&classification.reply_key, input.trim()) {
        (
            ReplyKey::Greeting,
            "こんにちは" | "こんにちは。" | "おはよう" | "おはよう。" | "こんばんは"
            | "こんばんは。",
        ) if !already_greeted => Some(("greeting", "こんにちは。")),
        (
            ReplyKey::Acknowledgement,
            "ありがとう" | "ありがとう。" | "ありがとうございます" | "ありがとうございます。",
        ) => Some(("acknowledgement", "どういたしまして。")),
        _ => None,
    }
}

async fn respond_with_lfm(
    ready: &super::Ready,
    history: Vec<Value>,
    pending_reasoning: bool,
    already_greeted: bool,
) -> Result<(FrontdeskClassification, bool), &'static str> {
    let lease = ready
        .session
        .acquire("backchannel")
        .await
        .map_err(|_| "lfm-provider-acquire-failed")?;
    let provider = lease.provider();
    let context_window = provider
        .context_window
        .ok_or("lfm-context-window-missing")?;
    let reserve = context_window.output_reserve_tokens;
    if reserve < 256 {
        return Err("lfm-output-budget-too-small");
    }
    let timeout = lease
        .request_budget(std::time::Duration::from_secs(15))
        .map_err(|_| "lfm-lease-expired")?;
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(timeout)
        .build()
        .map_err(|_| "lfm-client-creation-failed")?;
    let mut messages = vec![
        json!({"role":"system","content":INSTRUCTION}),
        json!({"role":"system","content":state_note(pending_reasoning, already_greeted)}),
    ];
    messages.extend(history);
    // UTF-8 bytes conservatively bound token use. Never silently truncate the current request.
    loop {
        let input_bytes = messages
            .iter()
            .map(|m| m["content"].as_str().map_or(0, str::len) + 64)
            .sum::<usize>();
        if input_bytes as u64 <= context_window.max_input_tokens() {
            break;
        }
        if messages.len() <= 3 {
            return Err("lfm-input-context-too-large");
        }
        messages.remove(2); // Evict oldest history, never the final current utterance.
    }
    let endpoint = provider
        .endpoint("chat/completions")
        .map_err(|_| "lfm-endpoint-invalid")?;
    let response = client.post(endpoint.clone()).bearer_auth(provider.token())
        .json(&json!({"model":provider.model,"messages":messages,"stream":false,
            "max_tokens":256,"temperature":0.1,
            "response_format":{"type":"json_schema","json_schema":{"name":"lfm_frontdesk_classification","strict":true,"schema":{
                "type":"object","properties":{
                    "route":{"type":"string","enum":["delegate","simple_reply"]},
                    "replyKey":{"type":"string","enum":["none","greeting","acknowledgement"]}
                },
                "required":["route","replyKey"],"additionalProperties":false
            }}}})).send().await
        .map_err(|_| "lfm-request-failed")?;
    let (response, structured_output_fallback) = if response.status().is_success() {
        (response, false)
    } else if matches!(response.status().as_u16(), 400 | 422) {
        // Some OpenAI-compatible LFM servers reject response_format. The fallback still requires
        // the exact JSON classifier contract and never accepts model-authored user-visible text.
        messages[0] = json!({"role":"system","content":
            INSTRUCTION});
        let fallback = client
            .post(endpoint)
            .bearer_auth(provider.token())
            .json(
                &json!({"model":provider.model,"messages":messages,"stream":false,
                "max_tokens":128,"temperature":0.1}),
            )
            .send()
            .await
            .map_err(|_| "lfm-plain-fallback-request-failed")?;
        if !fallback.status().is_success() {
            return Err("lfm-plain-fallback-rejected");
        }
        (fallback, true)
    } else {
        return Err("lfm-http-request-rejected");
    };
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| "lfm-response-interrupted")?;
        if bytes.len() + chunk.len() > 32_768 {
            return Err("lfm-response-too-large");
        }
        bytes.extend_from_slice(&chunk);
    }
    parse(&bytes).map(|classification| (classification, structured_output_fallback))
}

async fn classify_with_qwen(
    ready: &super::Ready,
    history: Vec<Value>,
    pending_reasoning: bool,
) -> Result<ReasoningNeed, &'static str> {
    if pending_reasoning {
        return Ok(ReasoningNeed { think: false });
    }
    let lease = ready
        .session
        .acquire("llm")
        .await
        .map_err(|_| "qwen-classifier-acquire-failed")?;
    let provider = lease.provider();
    let context_window = provider
        .context_window
        .ok_or("qwen-classifier-context-window-missing")?;
    let timeout = lease
        .request_budget(std::time::Duration::from_secs(15))
        .map_err(|_| "qwen-classifier-lease-expired")?;
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(timeout)
        .build()
        .map_err(|_| "qwen-classifier-client-failed")?;
    let mut messages = vec![json!({"role":"system","content":
        "音声会話の最新発言が、具体的でまとまった依頼として調査・推論・計算・ツール実行を開始できるならthink=true。挨拶、相槌、話の途中、希望を述べただけ、確認待ちはfalse。出力はJSON {\"think\":trueまたはfalse} だけ。"})];
    messages.extend(history);
    loop {
        let input_bytes = messages
            .iter()
            .map(|message| message["content"].as_str().map_or(0, str::len) + 64)
            .sum::<usize>();
        if input_bytes as u64 <= context_window.max_input_tokens() {
            break;
        }
        if messages.len() <= 2 {
            return Err("qwen-classifier-context-too-large");
        }
        messages.remove(1);
    }
    let response = client.post(provider.endpoint("chat/completions")
        .map_err(|_| "qwen-classifier-endpoint-invalid")?).bearer_auth(provider.token())
        .json(&json!({"model":provider.model,"messages":messages,"stream":false,
            "max_tokens":64,"temperature":0.0,
            "response_format":{"type":"json_schema","json_schema":{"name":"reasoning_need","strict":true,"schema":{
                "type":"object","properties":{"think":{"type":"boolean"}},
                "required":["think"],"additionalProperties":false
            }}}})).send().await.map_err(|_| "qwen-classifier-request-failed")?;
    if !response.status().is_success() {
        return Err("qwen-classifier-rejected");
    }
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| "qwen-classifier-interrupted")?;
        if body.len() + chunk.len() > 32_768 {
            return Err("qwen-classifier-too-large");
        }
        body.extend_from_slice(&chunk);
    }
    let value: Value = serde_json::from_slice(&body).map_err(|_| "qwen-classifier-json-invalid")?;
    let choices = value["choices"]
        .as_array()
        .filter(|items| items.len() == 1)
        .ok_or("qwen-classifier-choices-invalid")?;
    if choices[0]["finish_reason"] != "stop" {
        return Err("qwen-classifier-incomplete");
    }
    if choices[0]["message"]
        .get("tool_calls")
        .is_some_and(|value| {
            !value.is_null() && value.as_array().is_none_or(|items| !items.is_empty())
        })
    {
        return Err("qwen-classifier-tool-call-not-allowed");
    }
    serde_json::from_str(
        choices[0]["message"]["content"]
            .as_str()
            .ok_or("qwen-classifier-content-missing")?,
    )
    .map_err(|_| "qwen-classifier-contract-invalid")
}

fn parse(bytes: &[u8]) -> Result<FrontdeskClassification, &'static str> {
    let value: Value = serde_json::from_slice(bytes).map_err(|_| "lfm-response-json-invalid")?;
    let choices = value["choices"]
        .as_array()
        .filter(|v| v.len() == 1)
        .ok_or("lfm-response-choices-invalid")?;
    let choice = &choices[0];
    if choice["finish_reason"] != "stop" {
        return Err("lfm-response-incomplete");
    }
    if choice["message"]
        .get("tool_calls")
        .is_some_and(|v| !v.is_null() && v.as_array().is_none_or(|a| !a.is_empty()))
    {
        return Err("lfm-tool-call-not-allowed");
    }
    let content = choice["message"]["content"]
        .as_str()
        .ok_or("lfm-reply-missing")?
        .trim();
    let classification: FrontdeskClassification =
        serde_json::from_str(content).map_err(|_| "lfm-decision-contract-invalid")?;
    let valid = matches!(
        (&classification.route, &classification.reply_key),
        (Route::Delegate, ReplyKey::None)
            | (
                Route::SimpleReply,
                ReplyKey::None | ReplyKey::Greeting | ReplyKey::Acknowledgement
            )
    );
    if !valid {
        return Err("lfm-decision-combination-invalid");
    }
    Ok(classification)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn response(content: &str, finish: &str) -> Vec<u8> {
        serde_json::to_vec(
            &json!({"choices":[{"finish_reason":finish,"message":{"content":content}}]}),
        )
        .unwrap()
    }
    #[test]
    fn only_the_classifier_contract_is_accepted() {
        assert_eq!(
            parse(&response(
                r#"{"route":"simple_reply","replyKey":"greeting"}"#,
                "stop"
            ))
            .unwrap()
            .reply_key,
            ReplyKey::Greeting
        );
        assert_eq!(
            parse(&response(
                r#"{"route":"delegate","replyKey":"none"}"#,
                "stop"
            ))
            .unwrap()
            .route,
            Route::Delegate
        );
        for invalid in [
            r#"{"route":"simple_reply"}"#,
            r#"{"route":"answer","replyKey":"none"}"#,
            r#"{"route":"delegate","replyKey":"greeting"}"#,
            r#"{"route":"simple_reply","replyKey":"greeting","text":"勝手な回答"}"#,
            "はい、続きをどうぞ。",
        ] {
            assert!(parse(&response(invalid, "stop")).is_err());
        }
        assert!(parse(&response(
            r#"{"route":"delegate","replyKey":"none"}"#,
            "length"
        ))
        .is_err());
    }

    #[test]
    fn qwen_can_promote_but_never_cancel_an_lfm_reasoning_request() {
        let missed_by_lfm = FrontdeskClassification {
            route: Route::SimpleReply,
            reply_key: ReplyKey::None,
        };
        assert!(apply_reasoning_need(missed_by_lfm, Ok(true), false, true, "続き", false).think);

        let requested_by_lfm = FrontdeskClassification {
            route: Route::Delegate,
            reply_key: ReplyKey::None,
        };
        assert!(
            apply_reasoning_need(requested_by_lfm, Ok(false), false, true, "依頼", false).think
        );
    }

    #[test]
    fn qwen_classifier_failure_delegates_unhandled_input_instead_of_silencing_it() {
        let unhandled = FrontdeskClassification {
            route: Route::SimpleReply,
            reply_key: ReplyKey::None,
        };
        let decision = apply_reasoning_need(
            unhandled,
            Err("qwen-classifier-timeout"),
            false,
            true,
            "京都の旅行計画を作って",
            false,
        );
        assert!(decision.think);
        assert_eq!(decision.reply_key, Some("thinking"));
        assert_eq!(decision.classifier_failure, Some("qwen-classifier-timeout"));
    }

    #[test]
    fn qwen_classifier_failure_keeps_an_exact_host_reply_local() {
        let greeting = FrontdeskClassification {
            route: Route::SimpleReply,
            reply_key: ReplyKey::Greeting,
        };
        let decision = apply_reasoning_need(
            greeting,
            Err("qwen-classifier-timeout"),
            false,
            false,
            "こんにちは。",
            false,
        );
        assert!(!decision.think);
        assert_eq!(decision.say.as_deref(), Some("こんにちは。"));
    }

    #[test]
    fn host_owns_every_visible_reply_and_repeated_greetings_become_silent() {
        let greeting = FrontdeskClassification {
            route: Route::SimpleReply,
            reply_key: ReplyKey::Greeting,
        };
        assert_eq!(
            allowed_host_reply(&greeting, "こんにちは。", false),
            Some(("greeting", "こんにちは。"))
        );
        assert_eq!(allowed_host_reply(&greeting, "こんにちは。", true), None);
        assert_eq!(
            allowed_host_reply(&greeting, "こんにちは、調べて", false),
            None
        );
    }
}
