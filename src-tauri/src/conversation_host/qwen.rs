//! One leased, one-request Qwen stream. The caller owns persistence and speech dispatch.
use futures_util::StreamExt;
use saaa_conversation_core::{
    contracts::Failure,
    qwen::{CompletionDecoder, ControlParser, Event, SseDecoder},
};
use std::{future::Future, sync::Arc, time::Duration};
use tokio::{sync::watch, time::Instant};

const SYSTEM: &str = "あなたはSAAAの一次応答です。出力の最初の文字は必ず { にしてください。最初の行には {\"action\":\"reply\"}、{\"action\":\"clarify\"}、{\"action\":\"delegate\"} のいずれか1つだけを置きます。直後に改行を1つ入れ、2行目から利用者向けの短い本文を出してください。Markdownの囲み、前置き、思考過程、別のJSON項目は出さないでください。記憶・外部情報・ツールを使ったと主張しないでください。";

pub(crate) struct RunMeta {
    pub request_id: String,
    pub connection_id: String,
    pub allocation_id: String,
    pub model: String,
}

pub(crate) async fn run<F, Fut>(
    session: &Arc<saaa_larm_session::Session>,
    input: &str,
    request_id: &str,
    deadline: Instant,
    mut cancellation: watch::Receiver<bool>,
    mut on_event: F,
) -> Result<RunMeta, Failure>
where
    F: FnMut(Event) -> Fut,
    Fut: Future<Output = Result<(), Failure>>,
{
    tokio::select! {
        biased;
        _ = cancelled(&mut cancellation) => Err(failure("qwen", "request_outcome_unknown", request_id)),
        outcome = tokio::time::timeout_at(deadline, run_inner(session, input, request_id, &mut on_event)) => {
            match outcome {
                Ok(result) => result,
                Err(_) => Err(failure("qwen", "request_outcome_unknown", request_id)),
            }
        }
    }
}

async fn run_inner<F, Fut>(
    session: &Arc<saaa_larm_session::Session>,
    input: &str,
    request_id: &str,
    on_event: &mut F,
) -> Result<RunMeta, Failure>
where
    F: FnMut(Event) -> Fut,
    Fut: Future<Output = Result<(), Failure>>,
{
    let lease = session
        .acquire("backchannel")
        .await
        .map_err(|code| failure("resources", code, request_id))?;
    let provider = lease.provider();
    if provider.protocol != "openai.chat-completions.v1" {
        return Err(failure("qwen", "unsupported_protocol", request_id));
    }
    let url = provider
        .endpoint("chat/completions")
        .map_err(|code| failure("qwen", code, request_id))?;
    let budget = lease
        .request_budget(Duration::from_secs(8))
        .map_err(|code| failure("resources", code, request_id))?;
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(5))
        .build()
        .map_err(|_| failure("qwen", "client_unavailable", request_id))?;
    let response = client
        .post(url)
        .bearer_auth(provider.token())
        .header("Accept", "text/event-stream")
        .header("X-Request-ID", request_id)
        .timeout(budget)
        .json(&serde_json::json!({
            "model": provider.model,
            "messages": [
                {"role": "system", "content": SYSTEM},
                {"role": "user", "content": input}
            ],
            "stream": true,
            "max_tokens": 512,
            "chat_template_kwargs": {"enable_thinking": false}
        }))
        .send()
        .await
        .map_err(|_| failure("qwen", "request_outcome_unknown", request_id))?;
    if !response.status().is_success() {
        return Err(failure("qwen", "provider_rejected", request_id));
    }
    let mut stream = response.bytes_stream();
    let mut sse = SseDecoder::default();
    let mut completion = CompletionDecoder::default();
    let mut control = ControlParser::default();
    #[cfg(test)]
    let mut diagnostic_prefix = String::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| failure("qwen", "stream_interrupted", request_id))?;
        for event in sse
            .push(&chunk)
            .map_err(|_| failure("qwen", "invalid_sse", request_id))?
        {
            if let Some((_seq, content)) = completion
                .push_event(&event)
                .map_err(|_| failure("qwen", "invalid_completion", request_id))?
            {
                #[cfg(test)]
                if diagnostic_prefix.len() < 512 {
                    diagnostic_prefix.push_str(
                        &content
                            .chars()
                            .take(512 - diagnostic_prefix.len())
                            .collect::<String>(),
                    );
                }
                let parsed = control.push(&content).map_err(|_error| {
                    #[cfg(test)]
                    eprintln!("Qwen control parse: {_error:?}, prefix: {diagnostic_prefix:?}");
                    failure("qwen", "invalid_control", request_id)
                })?;
                for event in parsed {
                    on_event(event).await?;
                }
            }
        }
    }
    completion
        .finish_stream()
        .map_err(|_| failure("qwen", "missing_finish", request_id))?;
    control
        .finish()
        .map_err(|_| failure("qwen", "incomplete_body", request_id))?;
    Ok(RunMeta {
        request_id: request_id.to_string(),
        connection_id: session.connection_id().to_string(),
        allocation_id: lease.allocation_id().to_string(),
        model: provider.model.clone(),
    })
}

async fn cancelled(receiver: &mut watch::Receiver<bool>) {
    while !*receiver.borrow_and_update() {
        if receiver.changed().await.is_err() {
            break;
        }
    }
}

fn failure(stage: &str, code: &str, request_id: &str) -> Failure {
    Failure {
        stage: stage.to_string(),
        code: code.to_string(),
        message: match code {
            "cancelled" => "応答を中止しました。",
            "request_outcome_unknown" => "応答の停止を確認できませんでした。",
            _ => "応答を取得できませんでした。",
        }
        .to_string(),
        request_id: Some(request_id.to_string()),
    }
}
