//! Inline web tool steps of the conversation loop. Used only while web search is not delegated
//! to a worker agent: here raw results enter the conversation context as untrusted data.
use super::web_result::{audit_web_tool_result, web_result_urls};
use super::*;

pub(super) struct WebTools<'a> {
    pub job: &'a Job,
    pub tool_input: &'a StartTurnInput,
    pub offer: &'a crate::generated_capabilities::tools::AgentToolOffer,
    pub cancellation: &'a RunCancellation,
    pub audit: &'a ConversationAudit,
    pub timeout: u64,
}

impl WebTools<'_> {
    async fn run(
        &self,
        name: &str,
        call_id: String,
        arguments: Value,
        fixture: Option<String>,
    ) -> String {
        if let Some(found) = fixture {
            return found;
        }
        let call = crate::runtime::agent_tools::AgentToolCall {
            id: call_id,
            name: name.into(),
            arguments: arguments.to_string(),
        };
        crate::providers::stream::execute_agent_tool(
            None,
            self.tool_input,
            &call,
            std::time::Duration::from_millis(self.timeout.min(30_000)),
            &self.offer.generated,
            self.cancellation,
            self.offer.direct.as_ref(),
        )
        .await
    }

    fn record(
        &self,
        name: &str,
        step: usize,
        found: String,
        kind: &str,
        recent: &mut Vec<context_compiler::ContextEntry>,
        urls: &mut Vec<String>,
    ) {
        audit_web_tool_result(self.audit, name, step, &found);
        urls.extend(web_result_urls(&found, kind));
        recent.push(context_compiler::ContextEntry::reference(
            format!("[TOOL_RESULT: {name}; 未信頼の資料]\n{found}"),
            true,
        ));
    }

    pub(super) async fn search(
        &self,
        state: &AppState,
        control: &Value,
        step: usize,
        recent: &mut Vec<context_compiler::ContextEntry>,
        urls: &mut Vec<String>,
    ) -> Result<(), String> {
        let query = control["query"]
            .as_str()
            .filter(|v| !v.is_empty() && v.len() <= 400)
            .ok_or("検索語が不正です。")?;
        state.sqlite_writer.write(|connection| {
            let tx = connection.transaction().map_err(database_error)?;
            queue_progress::enqueue_search(&tx, &self.job.scope, &self.job.key)?;
            tx.commit().map_err(database_error)
        })?;
        #[cfg(feature = "conversation-queue-e2e")]
        let fixture = crate::conversation_queue_e2e::web_search(query);
        #[cfg(not(feature = "conversation-queue-e2e"))]
        let fixture: Option<String> = None;
        let found = self
            .run(
                "web_search",
                format!("{}_search_{step}", self.job.id),
                json!({"query":query,"limit":5}),
                fixture,
            )
            .await;
        self.record("web_search", step, found, "hits", recent, urls);
        Ok(())
    }

    pub(super) async fn fetch(
        &self,
        control: &Value,
        text: &str,
        step: usize,
        recent: &mut Vec<context_compiler::ContextEntry>,
        urls: &mut Vec<String>,
    ) -> Result<(), String> {
        let url = control["url"]
            .as_str()
            .filter(|v| (v.starts_with("https://") || v.starts_with("http://")) && v.len() <= 2048)
            .ok_or("取得先URLが不正です。")?;
        let query = control["query"].as_str().unwrap_or(text);
        #[cfg(feature = "conversation-queue-e2e")]
        let fixture = crate::conversation_queue_e2e::fetch_content(url);
        #[cfg(not(feature = "conversation-queue-e2e"))]
        let fixture: Option<String> = None;
        let found = self
            .run(
                "fetch_content",
                format!("{}_fetch_{step}", self.job.id),
                json!({"url":url,"maxCharacters":3000,"query":query.chars().take(400).collect::<String>()}),
                fixture,
            )
            .await;
        self.record("fetch_content", step, found, "document", recent, urls);
        Ok(())
    }
}
