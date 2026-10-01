use super::{chunks::Completion, sse::SseDecoder, ModelStreamContext, ProviderAttemptError};
use crate::ipc_contract::RuntimeEvent;
use crate::providers::stream::ProviderFailureKind as Failure;
use futures_util::StreamExt;
use std::time::{Duration, Instant};

pub(super) async fn run(
    request: reqwest::RequestBuilder,
    model: &str,
    timeout_ms: u64,
    context: ModelStreamContext<'_>,
    attempt: &mut super::observation::Attempt<'_>,
) -> Result<String, ProviderAttemptError> {
    let cancellation = context.cancellation.clone();
    let mut output_started = false;
    let result = tokio::time::timeout(Duration::from_millis(timeout_ms), async {
        let response = tokio::select! {
            biased;
            _ = cancellation.cancelled() => return Err(ProviderAttemptError::Cancelled { output_started }),
            response = request.send() => response.map_err(|_| ProviderAttemptError::failed(Failure::Network, output_started))?,
        };
        if !response.status().is_success() {
            return Err(ProviderAttemptError::failed(
                crate::providers::http::status_failure(response.status().as_u16()),
                output_started,
            ));
        }
        let is_sse = response.headers().get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(';').next())
            .is_some_and(|value| value.trim().eq_ignore_ascii_case("text/event-stream"));
        if !is_sse {
            return Err(ProviderAttemptError::failed(Failure::Protocol, output_started));
        }
        let mut decoder = SseDecoder::default();
        let mut completion = Completion::default();
        let mut stream = response.bytes_stream();
        'read: loop {
            let next = tokio::select! {
                biased;
                _ = cancellation.cancelled() => return Err(ProviderAttemptError::Cancelled { output_started }),
                next = stream.next() => next,
            };
            let Some(next) = next else { break };
            let bytes = next.map_err(|_| ProviderAttemptError::failed(Failure::ResponseInterrupted, output_started))?;
            for event in decoder.push(&bytes).map_err(|kind| ProviderAttemptError::failed(kind, output_started))? {
                let absorbed = completion.absorb(&event, model);
                attempt.response(completion.response_model.as_deref(), completion.usage.as_ref());
                let delta = absorbed.map_err(|kind| ProviderAttemptError::failed(kind, output_started))?;
                if !delta.is_empty() {
                    attempt.content();
                    if !output_started {
                        if let Some(persistence) = context.output_persistence {
                            persistence.mark_started().map_err(|_| ProviderAttemptError::failed(Failure::Internal, false))?;
                        }
                        output_started = true;
                    }
                    context.on_event.send_received(RuntimeEvent::Delta {
                        run_id: context.input.run_id.clone(), text: delta,
                    }, Instant::now()).map_err(|_| ProviderAttemptError::failed(Failure::ClientDisconnected, true))?;
                }
                // [DONE] is the protocol terminator. Waiting for the server to close
                // its HTTP body can delay the final speech chunk indefinitely.
                if completion.done { break 'read; }
            }
        }
        if !completion.complete().map_err(|kind| ProviderAttemptError::failed(kind, output_started))?.is_empty() {
            return Err(ProviderAttemptError::failed(Failure::Protocol, output_started));
        }
        if completion.content.trim().is_empty() {
            return Err(ProviderAttemptError::failed(Failure::Protocol, output_started));
        }
        Ok(completion.content)
    }).await;
    result.unwrap_or_else(|_| {
        Err(ProviderAttemptError::failed(
            Failure::Timeout,
            output_started,
        ))
    })
}
