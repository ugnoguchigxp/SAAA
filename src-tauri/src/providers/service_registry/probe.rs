//! Explicit diagnostics send a fixed input, never conversation/history or microphone audio.
use super::{AdapterKind, RegistrySnapshot, ServiceResource};
use crate::{
    providers::stream::{ModelStreamContext, ProviderAttemptError},
    RunCancellation, StartTurnInput,
};
use serde_json::{json, Value};
use std::sync::Arc;

pub(crate) fn fingerprint(
    snapshot: &RegistrySnapshot,
    resource: &ServiceResource,
) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    let connection = snapshot
        .connection(&resource.connection_id)
        .ok_or("Service is missing")?;
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&(connection, resource)).map_err(|e| e.to_string())?)
    ))
}

pub(crate) async fn run(
    snapshot: &RegistrySnapshot,
    resource: &ServiceResource,
    kind: &str,
) -> Result<Value, String> {
    let connection = snapshot
        .connection(&resource.connection_id)
        .ok_or("Service is missing")?;
    if !matches!(
        connection.adapter_kind,
        AdapterKind::ChatCompletions | AdapterKind::AnthropicMessages
    ) {
        return Err("この接続方式の動作確認は既存のProvider試験画面を使ってください".into());
    }
    let secret = connection
        .credential_ref
        .as_ref()
        .map(|r| {
            crate::credentials::load_named_secret(&r.service, &r.account)
                .and_then(|value| value.ok_or("APIキーを登録してください".into()))
        })
        .transpose()?;
    let authorization = secret
        .as_ref()
        .map(|key| zeroize::Zeroizing::new(format!("Bearer {}", &**key)));
    if kind == "models" {
        let base = connection.endpoint.trim_end_matches('/');
        let base = if connection.adapter_kind == AdapterKind::AnthropicMessages {
            base.strip_suffix("/messages").unwrap_or(base)
        } else {
            base
        };
        let endpoint = saaa_larm_session::http_api::operation_url(base, "models")?;
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .map_err(|_| "接続準備に失敗しました")?;
        let mut request = client.get(endpoint);
        if connection.adapter_kind == AdapterKind::AnthropicMessages {
            request = request.header("anthropic-version", "2023-06-01");
            if let Some(secret) = &secret {
                request = request.header("x-api-key", secret.as_str());
            }
        } else if let Some(authorization) = &authorization {
            request = request.header(reqwest::header::AUTHORIZATION, authorization.as_str());
        }
        let mut response = request
            .send()
            .await
            .map_err(|_| "モデル一覧へ接続できませんでした")?;
        if !response.status().is_success() {
            return Err(
                crate::providers::http::status_failure(response.status().as_u16())
                    .public_message()
                    .as_str()
                    .to_string(),
            );
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| "モデル一覧の受信に失敗しました")?
        {
            if bytes.len().saturating_add(chunk.len()) > 256 * 1024 {
                return Err("モデル一覧が大きすぎます".into());
            }
            bytes.extend_from_slice(&chunk);
        }
        let value: Value =
            serde_json::from_slice(&bytes).map_err(|_| "モデル一覧の形式が不正です")?;
        let items = value["data"]
            .as_array()
            .ok_or("モデル一覧の形式が不正です")?;
        let mut models = std::collections::BTreeSet::new();
        for item in items.iter().take(512) {
            let id = item["id"]
                .as_str()
                .filter(|id| !id.trim().is_empty() && id.len() <= 256)
                .ok_or("モデル一覧の形式が不正です")?;
            models.insert(id);
        }
        return Ok(
            json!({"models":models,"message":"モデル一覧を取得しました。会話での動作は別途確認が必要です。"}),
        );
    }
    if kind != "generation" {
        return Err("Unknown diagnostic".into());
    }
    if connection.adapter_kind == AdapterKind::AnthropicMessages {
        return crate::providers::anthropic_messages::run(&connection.endpoint,secret.as_ref().map(|v|v.as_str()),&resource.model,"",&[],"Reply with OK.",10_000,64,Arc::default()).await
            .map(|_|json!({"message":"固定文へのMessages応答を確認しました。会話の実行記録とは別です。"}))
            .map_err(|error|match error { ProviderAttemptError::Failed{kind,..}=>kind.public_message().as_str().to_string(),_=>"動作確認は中止されました".into() });
    }
    let input = StartTurnInput {
        run_id: "service_probe".into(),
        conversation_id: "service_probe".into(),
        content: "Reply with OK.".into(),
        workspace_path: None,
        retry_input_message_id: None,
        source_id: None,
        scope_refs: Vec::new(),
        input_origin: "text".into(),
        presentation_mode: "visual".into(),
    };
    let history = [crate::ipc_contract::ConversationMessage {
        parts: None,
        id: "service_probe".into(),
        conversation_id: input.conversation_id.clone(),
        role: "user".into(),
        content: input.content.clone(),
        created_at: String::new(),
    }];
    let sink = tauri::ipc::Channel::new(|_| Ok(()));
    let mut options = resource
        .request_options
        .clone()
        .map(serde_json::from_value::<saaa_larm_session::http_api::LlmOptions>)
        .transpose()
        .map_err(|_| "モデル設定が不正です")?
        .unwrap_or_default();
    options.tools = false;
    let result = crate::providers::chat_completions::run_with_options(
        &connection.endpoint,
        authorization.as_ref().map(|v| v.as_str()),
        &resource.model,
        &history,
        10_000,
        ModelStreamContext {
            reasoning_effort: "provider-default",
            max_output_tokens: 64,
            input: &input,
            on_event: &sink,
            cancellation: Arc::new(RunCancellation::default()),
            context_health: "green",
            context_sources: &[],
            context_omissions: &[],
            output_persistence: None,
        },
        crate::providers::chat_completions::RequestMode::JsonProbe,
        &options,
    )
    .await;
    match result {
        Ok(_) => Ok(
            json!({"message":"固定のテスト文への応答を確認しました。会話の実行記録とは別です。"}),
        ),
        Err(ProviderAttemptError::Failed { kind, .. }) => {
            Err(kind.public_message().as_str().to_string())
        }
        Err(_) => Err("動作確認は中止されました".into()),
    }
}

pub(crate) fn latest(
    db: &rusqlite::Connection,
    snapshot: &RegistrySnapshot,
) -> Result<Vec<Value>, String> {
    let mut statement = db.prepare("SELECT occurred_at,outcome,attributes_json FROM audit_events WHERE event_name='service-resource-probe' AND phase='terminal' ORDER BY sequence DESC LIMIT 512").map_err(crate::database_error)?;
    let rows = statement
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })
        .map_err(crate::database_error)?;
    let mut seen = std::collections::HashSet::new();
    let mut values = Vec::new();
    for row in rows {
        let (at, outcome, raw) = row.map_err(crate::database_error)?;
        let mut value: Value = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
        let key = (
            value["resourceId"].as_str().unwrap_or_default().to_string(),
            value["kind"].as_str().unwrap_or_default().to_string(),
        );
        if !seen.insert(key.clone()) {
            continue;
        }
        value["matchesCurrentSettings"] = json!(snapshot
            .resource(&key.0)
            .and_then(|r| fingerprint(snapshot, r).ok())
            .is_some_and(|fp| value["fingerprint"].as_str() == Some(fp.as_str())));
        value["occurredAt"] = json!(at);
        value["outcome"] = json!(outcome);
        values.push(value);
    }
    Ok(values)
}
