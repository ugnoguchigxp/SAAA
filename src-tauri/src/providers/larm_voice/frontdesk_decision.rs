//! LFM owns conversational replies and the decision to request deeper reasoning.
//! It never invents a replacement user request or grants tool authority.
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ConversationDecision {
    pub say: String,
    pub think: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReasoningNeed {
    think: bool,
}

const INSTRUCTION: &str = "あなたはSAAAで常に会話を担当するLFMです。ユーザーの音声は1.5秒の無音ごとに届きますが、無音は依頼完了を意味しません。発言に反応した短い自然な日本語の相槌、簡単な受け答え、必要な確認をsayに書きます。自発的に話し続けません。話の途中、挨拶、曖昧な発言ではthink=falseです。具体的にまとまった依頼で、調査・推論・計算・ツール実行が必要な場合だけthink=trueにし、sayは『少々お待ちください、考えます。』のような短い応答にします。think=trueでも会話担当をQwenへ委譲するのではなく、LFMは次の発言にも応対し続けます。pendingReasoning=trueなら同じ依頼について再度think=trueにしてはいけません。進捗や完了を捏造しません。返答はJSON {\"say\":\"短い応答\",\"think\":trueまたはfalse} の2項目だけです。JSON以外の説明や思考タグは禁止です。";

pub(crate) async fn decide(
    ready: &super::Ready,
    history: Vec<Value>,
    pending_reasoning: bool,
) -> Result<ConversationDecision, &'static str> {
    let (lfm, qwen) = tokio::join!(
        respond_with_lfm(ready, history.clone(), pending_reasoning),
        async {
            tokio::time::timeout(
                std::time::Duration::from_millis(2_500),
                classify_with_qwen(ready, history, pending_reasoning),
            )
            .await
            .map_err(|_| "qwen-classifier-timeout")?
        },
    );
    let mut decision = lfm?;
    // Qwen is a supporting classifier, not the conversational owner. If its short classification
    // is unavailable (for example while the reasoning slot is occupied), LFM keeps responding.
    // A pending request is fenced in the host so no model can launch it twice.
    if pending_reasoning {
        decision.think = false;
    } else if let Ok(reasoning) = qwen {
        decision.think = reasoning.think;
    }
    if decision.think {
        decision.say = "少々お待ちください、考えます。".into();
    }
    Ok(decision)
}

async fn respond_with_lfm(
    ready: &super::Ready,
    history: Vec<Value>,
    pending_reasoning: bool,
) -> Result<ConversationDecision, &'static str> {
    let lease = ready
        .session
        .acquire("backchannel")
        .await
        .map_err(|_| "lfm-provider-acquire-failed")?;
    let provider = lease.provider();
    let reserve = provider
        .context_window
        .ok_or("lfm-context-window-missing")?
        .output_reserve_tokens;
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
        json!({"role":"system","content":format!("pendingReasoning={pending_reasoning}")}),
    ];
    messages.extend(history);
    // UTF-8 bytes conservatively bound token use. Never silently truncate the current request.
    loop {
        let input_bytes = messages
            .iter()
            .map(|m| m["content"].as_str().map_or(0, str::len) + 64)
            .sum::<usize>();
        if input_bytes as u64 <= provider.context_window.unwrap().max_input_tokens() {
            break;
        }
        if messages.len() <= 3 {
            return Err("lfm-input-context-too-large");
        }
        messages.remove(2); // Evict oldest history, never the final current utterance.
    }
    let response = client.post(provider.endpoint("chat/completions")
        .map_err(|_| "lfm-endpoint-invalid")?).bearer_auth(provider.token())
        .json(&json!({"model":provider.model,"messages":messages,"stream":false,
            "max_tokens":256,"temperature":0.1,
            "response_format":{"type":"json_schema","json_schema":{"name":"lfm_conversation_response","strict":true,"schema":{
                "type":"object","properties":{"say":{"type":"string","maxLength":80},"think":{"type":"boolean"}},
                "required":["say","think"],"additionalProperties":false
            }}}})).send().await
        .map_err(|_| "lfm-request-failed")?;
    if !response.status().is_success() {
        return Err("lfm-http-request-rejected");
    }
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| "lfm-response-interrupted")?;
        if bytes.len() + chunk.len() > 32_768 {
            return Err("lfm-response-too-large");
        }
        bytes.extend_from_slice(&chunk);
    }
    parse(&bytes)
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
    let body = response
        .bytes()
        .await
        .map_err(|_| "qwen-classifier-interrupted")?;
    if body.len() > 32_768 {
        return Err("qwen-classifier-too-large");
    }
    let value: Value = serde_json::from_slice(&body).map_err(|_| "qwen-classifier-json-invalid")?;
    let choices = value["choices"]
        .as_array()
        .filter(|items| items.len() == 1)
        .ok_or("qwen-classifier-choices-invalid")?;
    if choices[0]["finish_reason"] != "stop" {
        return Err("qwen-classifier-incomplete");
    }
    serde_json::from_str(
        choices[0]["message"]["content"]
            .as_str()
            .ok_or("qwen-classifier-content-missing")?,
    )
    .map_err(|_| "qwen-classifier-contract-invalid")
}

fn parse(bytes: &[u8]) -> Result<ConversationDecision, &'static str> {
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
    let mut decision: ConversationDecision = serde_json::from_str(
        choice["message"]["content"]
            .as_str()
            .ok_or("lfm-reply-missing")?,
    )
    .map_err(|_| "lfm-decision-contract-invalid")?;
    if decision.say.trim().is_empty() || decision.say.chars().count() > 80 {
        return Err("lfm-say-length-invalid");
    }
    // A reasoning request must not contain an unverified answer or promise completed work.
    if decision.think {
        decision.say = "少々お待ちください、考えます。".into();
    }
    Ok(decision)
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
    fn only_the_two_field_contract_is_accepted() {
        assert!(
            !parse(&response(r#"{"say":"はい。","think":false}"#, "stop"))
                .unwrap()
                .think
        );
        assert!(
            parse(&response(r#"{"say":"考えます。","think":true}"#, "stop"))
                .unwrap()
                .think
        );
        for invalid in [
            r#"{"say":"はい。"}"#,
            r#"{"say":"はい。","think":"yes"}"#,
            r#"{"say":"はい。","think":true,"request":"invented"}"#,
            r#"{"say":"","think":true}"#,
        ] {
            assert!(parse(&response(invalid, "stop")).is_err());
        }
        assert!(parse(&response(r#"{"say":"考えます。","think":true}"#, "length")).is_err());
    }
}
