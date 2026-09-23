//! Spoken progress is derived from host-known phases, never tool arguments or results.
use super::super::stream::{execute_agent_tool, ModelStreamContext};
use crate::generated_capabilities::publication::GeneratedToolSnapshot;
use futures_util::FutureExt;
use std::time::Duration;

pub(super) fn supports(name: &str) -> bool {
    progress_text(name, "auto").is_some()
}

pub(super) async fn execute(
    context: &ModelStreamContext<'_>,
    call: &crate::runtime::agent_tools::AgentToolCall,
    report_progress: bool,
    timeout: Duration,
    generated: &GeneratedToolSnapshot,
) -> (String, bool) {
    let web_activity = match call.name.as_str() {
        "web_search" => Some("web-search-started"),
        "fetch_content" => Some("source-fetch-started"),
        _ => None,
    };
    if let Some(kind) = web_activity {
        let _ = context
            .on_event
            .send(crate::ipc_contract::RuntimeEvent::Activity {
                run_id: context.input.run_id.clone(),
                kind: kind.into(),
                summary: "Retrieving public source information.".into(),
            });
    }
    let audit = crate::providers::session_store::ToolExecutionAudit::start(context, call);
    let tool = audit.run_with_outcome(catch_tool_execution(execute_agent_tool(
        context.output_persistence,
        context.input,
        call,
        timeout,
        generated,
        &context.cancellation,
    )));
    let language = language(context);
    let Some(canonical) = report_progress
        .then(|| progress_text(&call.name, &language))
        .flatten()
    else {
        let result = tool.await;
        announce_search_source(context, call, &result);
        if web_activity.is_some() {
            let _ = context
                .on_event
                .send(crate::ipc_contract::RuntimeEvent::Activity {
                    run_id: context.input.run_id.clone(),
                    kind: "source-retrieval-completed".into(),
                    summary: "Source retrieval finished; preparing the answer.".into(),
                });
        }
        return (result, false);
    };
    let progress = crate::larm_voice::render_response(
        &context.input.conversation_id,
        crate::larm_voice::ResponseKind::ToolProgress,
        canonical,
        &language,
        context.cancellation.clone(),
    );
    tokio::pin!(tool);
    tokio::pin!(progress);
    let result = tokio::select! {
        biased;
        result = &mut tool => (result, false),
        rendered = &mut progress => {
            let mut spoken = false;
            if let Ok(speech) = rendered {
                if !context.cancellation.is_cancelled() {
                    spoken = context.on_event.speak_voice_response(
                        &context.input.run_id,
                        crate::larm_voice::ResponseKind::ToolProgress,
                        &speech,
                    ).is_ok();
                }
            }
            (tool.await, spoken)
        }
    };
    announce_search_source(context, call, &result.0);
    if web_activity.is_some() {
        let _ = context
            .on_event
            .send(crate::ipc_contract::RuntimeEvent::Activity {
                run_id: context.input.run_id.clone(),
                kind: "source-retrieval-completed".into(),
                summary: "Source retrieval finished; preparing the answer.".into(),
            });
    }
    result
}

fn announce_search_source(
    context: &ModelStreamContext<'_>,
    call: &crate::runtime::agent_tools::AgentToolCall,
    result: &str,
) {
    if call.name != "web_search" {
        return;
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(result) else {
        return;
    };
    let Some(hit) = value
        .get("hits")
        .and_then(|hits| hits.as_array())
        .and_then(|hits| hits.first())
    else {
        return;
    };
    let (Some(url), Some(title)) = (
        hit.get("url").and_then(|url| url.as_str()),
        hit.get("title").and_then(|title| title.as_str()),
    ) else {
        return;
    };
    if !matches!(url::Url::parse(url), Ok(parsed) if matches!(parsed.scheme(), "http" | "https")) {
        return;
    }
    let _ = context
        .on_event
        .send(crate::ipc_contract::RuntimeEvent::Activity {
            run_id: context.input.run_id.clone(),
            kind: "source-available".into(),
            summary: serde_json::json!({"url": url, "title": title}).to_string(),
        });
}

async fn catch_tool_execution(
    future: impl std::future::Future<Output = String>,
) -> (String, &'static str) {
    match std::panic::AssertUnwindSafe(future).catch_unwind().await {
        Ok(result) => (result, "success"),
        Err(_) => (
            crate::runtime::agent_tools::tool_error_content(
                "tool-internal-error",
                "The tool stopped unexpectedly. Continue without assuming it succeeded.",
            ),
            "failure",
        ),
    }
}
fn language(context: &ModelStreamContext<'_>) -> String {
    context
        .output_persistence
        .and_then(|persistence| {
            persistence
                .state
                .sqlite_readers
                .read(|connection| {
                    Ok(
                        crate::persistence::settings::regional_preferences::load(connection)?
                            .language,
                    )
                })
                .ok()
        })
        .filter(|language| matches!(language.as_str(), "ja" | "en"))
        .unwrap_or_else(|| "auto".to_string())
}

fn progress_text(name: &str, language: &str) -> Option<&'static str> {
    let english = language == "en";
    if crate::runtime::web_fetch::is_web_fetch_tool(name) {
        return Some(if english {
            "I am checking the information needed for the answer."
        } else {
            "回答に必要な情報を調べています。"
        });
    }
    if name == crate::memory::contracts::RECALL_TOOL_NAME
        || crate::runtime::agent_tools::is_typed_memory_tool(name)
    {
        return Some(if english {
            "I am checking the relevant conversation records."
        } else {
            "関連する会話の記録を確認しています。"
        });
    }
    if crate::coding::contracts::NAMES.contains(&name) {
        return Some(if english {
            "I am starting or checking the requested work."
        } else {
            "依頼された作業の開始または状況確認を進めています。"
        });
    }
    if crate::generative_ui::tools::NAMES.contains(&name) {
        return Some(if english {
            "I am preparing the requested display."
        } else {
            "依頼された表示内容を整えています。"
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn describes_only_known_host_tool_phases() {
        assert_eq!(
            progress_text(crate::runtime::web_fetch::WEB_SEARCH_TOOL_NAME, "ja"),
            Some("回答に必要な情報を調べています。")
        );
        assert_eq!(
            progress_text(crate::memory::contracts::RECALL_TOOL_NAME, "en"),
            Some("I am checking the relevant conversation records.")
        );
        assert!(
            progress_text(crate::voice_behavior::UPDATE_VOICE_BEHAVIOR_TOOL_NAME, "ja").is_none()
        );
        assert!(progress_text("provider_invented_tool", "ja").is_none());
    }

    #[tokio::test]
    async fn tool_panic_is_caught_at_the_provider_boundary() {
        let (result, outcome) = catch_tool_execution(async { panic!("fixture panic") }).await;
        assert_eq!(outcome, "failure");
        assert!(result.contains("tool-internal-error"));
    }
}
