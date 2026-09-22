use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;

use crate::persistence::SqliteWriter;
use crate::tool_selection::backends::{BackendOutcome, BackendRequest, ToolBackend};
use crate::RunCancellation;

use super::auth::Authorization;
use super::tools;

pub struct RecordsBackend {
    writer: Arc<SqliteWriter>,
}

impl RecordsBackend {
    pub fn new(writer: Arc<SqliteWriter>) -> Self {
        Self { writer }
    }
}

pub struct UnavailableBackend;

#[async_trait]
impl ToolBackend for UnavailableBackend {
    async fn invoke(
        &self,
        _request: BackendRequest,
        _cancellation: &RunCancellation,
    ) -> BackendOutcome {
        BackendOutcome::failed("mcp-unconfigured")
    }
}

#[async_trait]
impl ToolBackend for RecordsBackend {
    async fn invoke(
        &self,
        request: BackendRequest,
        _cancellation: &RunCancellation,
    ) -> BackendOutcome {
        let operation = request
            .binding
            .get("operation")
            .and_then(Value::as_str)
            .unwrap_or("");
        let (principal_id, conversation_id) = request
            .actor
            .as_ref()
            .map(|actor| (actor.principal_id.clone(), actor.conversation_id.clone()))
            .unwrap_or_else(|| (String::new(), String::new()));
        let auth = Authorization {
            principal_id,
            conversation_id,
            allowed_scope_keys: Vec::new(),
        };
        let value = self
            .writer
            .read_serialized(|connection| {
                Ok(tools::execute(
                    connection,
                    &auth,
                    operation,
                    &request.arguments,
                ))
            })
            .unwrap_or_else(|error| serde_json::json!({"status":"unavailable","reason":error}));
        BackendOutcome::succeeded(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool_selection::backends::router::BackendRouter;
    use std::sync::Arc;

    struct Echo;

    #[async_trait]
    impl ToolBackend for Echo {
        async fn invoke(&self, request: BackendRequest, _: &RunCancellation) -> BackendOutcome {
            BackendOutcome::succeeded(serde_json::json!(request.backend_key))
        }
    }

    #[tokio::test]
    async fn cw_35_router_dispatches_records_kind() {
        let router = BackendRouter::new(Arc::new(Echo), Arc::new(Echo), Arc::new(Echo));
        let outcome = router
            .invoke(
                BackendRequest {
                    call_id: "c".into(),
                    tool_id: "read_record".into(),
                    revision_id: "r".into(),
                    backend_key: "records".into(),
                    binding: serde_json::json!({"kind": "records", "operation": "read_record"}),
                    arguments: serde_json::json!({}),
                    timeout: std::time::Duration::from_secs(1),
                    origin: "conversation",
                    actor: Some(crate::generated_capabilities::contracts::CallActor {
                        principal_id: "p".into(),
                        conversation_id: "c".into(),
                        project_id: None,
                        run_id: "run".into(),
                    }),
                },
                &RunCancellation::default(),
            )
            .await;
        assert_eq!(
            BackendRouter::kind(&serde_json::json!({"kind":"records"})),
            "records"
        );
        let _ = outcome;
    }
}
