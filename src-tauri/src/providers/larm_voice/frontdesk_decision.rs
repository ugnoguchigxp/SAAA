//! LFM owns conversational replies and the decision to request deeper reasoning.
//! It never invents a replacement user request or grants tool authority.
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ConversationDecision {
    pub reply: String,
    pub action: ConversationAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ConversationAction {
    Respond,
    Delegate,
}

const INSTRUCTION: &str = "あなたはSAAAの会話窓口LFMです。ユーザーの音声は1.5秒の無音ごとに届きます。無音は依頼完了を意味しません。発言に応じた短い自然な日本語の相槌、簡単な受け答え、必要な確認を行います。話の途中、挨拶、曖昧な発言はrespondです。具体的にまとまった依頼で、調査・推論・計算・ツール実行が必要な場合だけdelegateを選び、短く『少々お待ちください、考えます。』などと返します。前の発言も踏まえて判断します。pendingReasoning=trueの時、委譲済み依頼を再送してはいけません。進捗を捏造しません。自発的に話し続けません。実際の処理と権限確認はQwen側の担当です。返答は次のJSONのみ: {\"reply\":\"短い応答\",\"action\":\"respond\"} または {\"reply\":\"短い応答\",\"action\":\"delegate\"}。JSON以外の説明や思考タグは禁止。";

pub(crate) async fn decide(
    ready: &super::Ready,
    history: Vec<Value>,
    pending_reasoning: bool,
) -> Result<ConversationDecision, &'static str> {
    let lease = ready.session.acquire("backchannel").await
        .map_err(|_| "lfm-provider-acquire-failed")?;
    let provider = lease.provider();
    let reserve = provider.context_window.ok_or("lfm-context-window-missing")?.output_reserve_tokens;
    if reserve < 256 { return Err("lfm-output-budget-too-small"); }
    let timeout = lease.request_budget(std::time::Duration::from_secs(15))
        .map_err(|_| "lfm-lease-expired")?;
    let client = reqwest::Client::builder().no_proxy()
        .redirect(reqwest::redirect::Policy::none()).timeout(timeout).build()
        .map_err(|_| "lfm-client-creation-failed")?;
    let mut messages = vec![json!({"role":"system","content":INSTRUCTION}),
        json!({"role":"system","content":format!("pendingReasoning={pending_reasoning}")})];
    messages.extend(history);
    // UTF-8 bytes conservatively bound token use. Never silently truncate the current request.
    loop {
        let input_bytes = messages.iter().map(|m|m["content"].as_str().map_or(0,str::len) + 64).sum::<usize>();
        if input_bytes as u64 <= provider.context_window.unwrap().max_input_tokens() {break;}
        if messages.len() <= 3 {return Err("lfm-input-context-too-large");}
        messages.remove(2); // Evict oldest history, never the final current utterance.
    }
    let response = client.post(provider.endpoint("chat/completions")
        .map_err(|_| "lfm-endpoint-invalid")?).bearer_auth(provider.token())
        .json(&json!({"model":provider.model,"messages":messages,"stream":false,
            "max_tokens":256,"temperature":0.1,
            "response_format":{"type":"json_schema","json_schema":{"name":"lfm_conversation_decision","strict":true,"schema":{
                "type":"object","properties":{"reply":{"type":"string"},"action":{"type":"string","enum":["respond","delegate"]}},
                "required":["reply","action"],"additionalProperties":false
            }}}})).send().await
        .map_err(|_| "lfm-request-failed")?;
    if !response.status().is_success() { return Err("lfm-http-request-rejected"); }
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| "lfm-response-interrupted")?;
        if bytes.len() + chunk.len() > 32_768 { return Err("lfm-response-too-large"); }
        bytes.extend_from_slice(&chunk);
    }
    parse(&bytes)
}

fn parse(bytes: &[u8]) -> Result<ConversationDecision, &'static str> {
    let value: Value = serde_json::from_slice(bytes).map_err(|_| "lfm-response-json-invalid")?;
    let choices = value["choices"].as_array().filter(|v| v.len() == 1)
        .ok_or("lfm-response-choices-invalid")?;
    let choice = &choices[0];
    if choice["finish_reason"] != "stop" { return Err("lfm-response-incomplete"); }
    if choice["message"].get("tool_calls").is_some_and(|v| !v.is_null()
        && v.as_array().is_none_or(|a| !a.is_empty())) { return Err("lfm-tool-call-not-allowed"); }
    let mut decision: ConversationDecision = serde_json::from_str(choice["message"]["content"]
        .as_str().ok_or("lfm-reply-missing")?).map_err(|_| "lfm-decision-contract-invalid")?;
    if decision.reply.trim().is_empty() || decision.reply.chars().count() > 240 {
        return Err("lfm-reply-length-invalid");
    }
    // A delegation response must not contain an unverified answer or promise completed work.
    // Routing remains LFM's decision; the waiting acknowledgement has a fixed, truthful meaning.
    if decision.action == ConversationAction::Delegate {
        decision.reply = "少々お待ちください、考えます。".into();
    }
    Ok(decision)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn response(content: &str, finish: &str) -> Vec<u8> {
        serde_json::to_vec(&json!({"choices":[{"finish_reason":finish,"message":{"content":content}}]})).unwrap()
    }
    #[test]
    fn only_explicit_complete_decisions_can_delegate() {
        assert_eq!(parse(&response(r#"{"reply":"はい。","action":"respond"}"#, "stop")).unwrap().action, ConversationAction::Respond);
        assert_eq!(parse(&response(r#"{"reply":"考えます。","action":"delegate"}"#, "stop")).unwrap().action, ConversationAction::Delegate);
        for invalid in [r#"{"reply":"はい。"}"#, r#"{"reply":"はい。","action":"qwen"}"#, r#"{"reply":"はい。","action":"delegate","request":"invented"}"#, r#"{"reply":"","action":"delegate"}"#] {
            assert!(parse(&response(invalid, "stop")).is_err());
        }
        assert!(parse(&response(r#"{"reply":"考えます。","action":"delegate"}"#, "length")).is_err());
    }
}
