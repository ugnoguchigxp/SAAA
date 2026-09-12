//! Shadow-only: 2B output cannot downgrade a request or change the spoken answer.
use crate::RunCancellation;
use futures_util::StreamExt;
use std::sync::Arc;
pub(crate) async fn classify_shadow(
    conversation: &str,
    text: &str,
    cancellation: Arc<RunCancellation>,
) {
    if !super::enabled() {
        return;
    }
    let started = std::time::Instant::now();
    let work = async {
        let ready = super::current(conversation).await?;
        let lease = ready
            .session
            .acquire("decision-default")
            .await
            .map_err(str::to_string)?;
        let provider = lease.provider();
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| "Decision client failed")?;
        let response = client.post(provider.endpoint("chat/completions").map_err(str::to_string)?)
            .bearer_auth(provider.token()).json(&serde_json::json!({"model":provider.model,
                "messages":[{"role":"system","content":"Classify the user message. Return only JSON: {\"route\":\"delegate\"} or {\"route\":\"simple_reply\",\"replyKey\":\"greeting\"} or {\"route\":\"simple_reply\",\"replyKey\":\"acknowledgement\"}. Use delegate for questions, requests and ambiguous input."},
                    {"role":"user","content":text}],"stream":false,"max_tokens":128})).send().await.map_err(|_| "Decision request failed")?;
        if !response.status().is_success() {
            return Err("Decision request rejected".to_string());
        }
        let mut stream = response.bytes_stream();
        let mut bytes = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| "Decision response interrupted")?;
            if bytes.len() + chunk.len() > 16_384 {
                return Err("Decision response too large".into());
            }
            bytes.extend_from_slice(&chunk);
        }
        parse(&bytes).map_err(str::to_string)
    };
    let result = tokio::select! { biased;
        _ = cancellation.cancelled() => return,
        result = tokio::time::timeout(std::time::Duration::from_secs(3), work) => result,
    };
    crate::providers::http_metrics::record(
        match result {
            Ok(Ok(metric)) => metric,
            _ => "decisionShadowFailed",
        },
        started.elapsed(),
    );
}

#[derive(serde::Deserialize)]
#[serde(tag = "route", rename_all = "snake_case", deny_unknown_fields)]
enum Classification {
    Delegate {},
    SimpleReply {
        #[serde(rename = "replyKey")]
        reply: Reply,
    },
}
#[derive(serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum Reply {
    Greeting,
    Acknowledgement,
}
fn parse(bytes: &[u8]) -> Result<&'static str, &'static str> {
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|_| "Decision response invalid")?;
    let choices = value["choices"]
        .as_array()
        .filter(|v| v.len() == 1)
        .ok_or("Decision choices invalid")?;
    let choice = &choices[0];
    if choice["finish_reason"] != "stop"
        || choice["message"]
            .get("tool_calls")
            .is_some_and(|v| !v.is_null() && v.as_array().is_none_or(|a| !a.is_empty()))
    {
        return Err("Decision response incomplete");
    }
    let content = choice["message"]["content"]
        .as_str()
        .ok_or("Decision content missing")?;
    match serde_json::from_str::<Classification>(content)
        .map_err(|_| "Decision classification invalid")?
    {
        Classification::Delegate {} => Ok("decisionShadowDelegate"),
        Classification::SimpleReply {
            reply: Reply::Greeting,
        } => Ok("decisionShadowGreeting"),
        Classification::SimpleReply {
            reply: Reply::Acknowledgement,
        } => Ok("decisionShadowAcknowledgement"),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn completion(content: &str, finish: &str) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({"choices":[{"finish_reason":finish,"message":{"content":content}}]})).unwrap()
    }
    #[test]
    fn shadow_classifications_require_the_exact_contract_and_complete_response() {
        assert_eq!(
            parse(&completion(r#"{"route":"delegate"}"#, "stop")),
            Ok("decisionShadowDelegate")
        );
        assert_eq!(
            parse(&completion(
                r#"{"route":"simple_reply","replyKey":"greeting"}"#,
                "stop"
            )),
            Ok("decisionShadowGreeting")
        );
        for content in [
            r#"{"route":"simple_reply"}"#,
            r#"{"route":"simple_reply","replyKey":"made_up"}"#,
            r#"{"route":"delegate","text":"untrusted reply"}"#,
        ] {
            assert!(parse(&completion(content, "stop")).is_err());
        }
        assert!(parse(&completion(r#"{"route":"delegate"}"#, "length")).is_err());
    }
}
