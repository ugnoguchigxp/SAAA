use super::{ProviderAttemptError, ProviderFailureKind as Failure};
use serde_json::Value;

pub(super) async fn read(response: reqwest::Response) -> Result<Value, Failure> {
    let mut response = response;
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| Failure::ResponseInterrupted)?
    {
        if bytes.len().saturating_add(chunk.len()) > 2 * 1024 * 1024 {
            return Err(Failure::RequestTooLarge);
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| Failure::Protocol)
}

pub(super) fn content(
    response: &Value,
    attempt: &mut super::observation::Attempt<'_>,
) -> Result<String, ProviderAttemptError> {
    let usage = response
        .get("usage")
        .filter(|v| v.is_object())
        .map(crate::runtime::context::usage::parse_openai_usage);
    attempt.response(response["model"].as_str(), usage.as_ref());
    let choice = response["choices"]
        .as_array()
        .and_then(|choices| (choices.len() == 1).then(|| &choices[0]))
        .ok_or_else(|| {
            ProviderAttemptError::failed_with_detail(
                Failure::Protocol,
                false,
                Some("invalid-chat-choices"),
            )
        })?;
    if choice["finish_reason"] != "stop" {
        return Err(ProviderAttemptError::failed_with_detail(
            Failure::Protocol,
            false,
            Some("chat-finish-reason-not-stop"),
        ));
    }
    if !choice["message"]["tool_calls"].is_null() {
        return Err(ProviderAttemptError::failed_with_detail(
            Failure::Protocol,
            false,
            Some("unexpected-chat-tool-call"),
        ));
    }
    let content = choice["message"]["content"]
        .as_str()
        .filter(|text| !text.trim().is_empty())
        .ok_or_else(|| {
            ProviderAttemptError::failed_with_detail(
                Failure::Protocol,
                false,
                Some("missing-chat-content"),
            )
        })?
        .to_string();
    Ok(content)
}

pub(super) async fn stream(
    response: reqwest::Response,
    context: &super::ModelStreamContext<'_>,
    attempt: &mut super::observation::Attempt<'_>,
) -> Result<String, ProviderAttemptError> {
    let is_json = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .is_some_and(|v| v.trim().eq_ignore_ascii_case("application/json"));
    if !is_json {
        return Err(ProviderAttemptError::failed(Failure::Protocol, false));
    }
    let value = tokio::select! {
        biased;
        _=context.cancellation.cancelled()=>return Err(ProviderAttemptError::Cancelled{output_started:false}),
        value=read(response)=>value.map_err(|kind|ProviderAttemptError::failed(kind,false))?,
    };
    let text = content(&value, attempt)?;
    if let Some(persistence) = context.output_persistence {
        persistence
            .mark_started()
            .map_err(|_| ProviderAttemptError::failed(Failure::Internal, false))?;
    }
    attempt.content();
    context
        .on_event
        .send_received(
            crate::ipc_contract::RuntimeEvent::Delta {
                run_id: context.input.run_id.clone(),
                text: text.clone(),
            },
            std::time::Instant::now(),
        )
        .map_err(|_| ProviderAttemptError::failed(Failure::ClientDisconnected, true))?;
    Ok(text)
}
