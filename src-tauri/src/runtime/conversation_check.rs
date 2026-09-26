//! Small text response path for checking the selected conversation provider in the normal UI.
//! The full ASR, reasoning, and speech response host is still under construction.
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::{
    database_error, now_iso, persistence, validate_identifier, AppState, ModelProviderSettings,
    RunCancellation, StartTurnInput, PRIMARY_CONVERSATION_ID,
};

static BUSY: AtomicBool = AtomicBool::new(false);

struct BusyGuard;
impl BusyGuard {
    fn acquire() -> Result<Self, String> {
        BUSY.compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
            .map(|_| Self)
            .map_err(|_| "会話の応答を処理中です。".to_string())
    }
}
impl Drop for BusyGuard {
    fn drop(&mut self) {
        BUSY.store(false, Ordering::Release);
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SubmitInput {
    input_id: String,
    text: String,
    source: CheckSource,
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case")]
enum CheckSource {
    Configured,
    Larm,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SubmitResult {
    content: String,
    model: String,
    provider_label: String,
}

#[tauri::command]
pub(crate) async fn submit_conversation_text(
    state: tauri::State<'_, AppState>,
    input: SubmitInput,
) -> Result<SubmitResult, String> {
    validate_identifier(&input.input_id, "input id")?;
    if input.text.trim().is_empty() || input.text.len() > 4096 {
        return Err("入力は1〜4096バイトにしてください。".into());
    }
    let _busy = BusyGuard::acquire()?;
    let user_id = format!("check_{}", input.input_id);
    let answer_id = format!("reply_{}", input.input_id);
    let saved = state.sqlite_readers.read(|connection| {
        connection
            .query_row(
                "SELECT u.content, a.content FROM conversation_messages u \
             JOIN conversation_messages a ON a.id=?2 \
             WHERE u.id=?1 AND u.conversation_id=?3 AND a.conversation_id=?3",
                params![user_id, answer_id, PRIMARY_CONVERSATION_ID],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(database_error)
    })?;
    if let Some((original, content)) = saved {
        if original != input.text {
            return Err("同じ入力IDで異なる本文は送信できません。".into());
        }
        return Ok(SubmitResult {
            content,
            model: "保存済み".into(),
            provider_label: "会話記録".into(),
        });
    }
    let (providers, route) = state.sqlite_readers.read(|connection| {
        Ok((
            persistence::load_model_providers(connection)?,
            persistence::load_routing_settings(connection)?.conversation_respond,
        ))
    })?;
    let (content, model, provider_label) = if matches!(input.source, CheckSource::Larm)
        || route.source == "harness"
    {
        let (content, model) = complete_larm(&providers, &input.text, route.timeout_ms).await?;
        (content, model, "LARM backchannel".to_string())
    } else {
        let provider = providers
            .providers
            .iter()
            .find(|candidate| {
                route.primary_provider_id.as_deref() == Some(candidate.id()) && candidate.enabled()
            })
            .ok_or("設定済みの会話用Providerが見つかりません。")?;
        match provider {
            ModelProviderSettings::OpenAiCompatible(provider) => {
                let key = crate::providers::openai_compatible::provider_api_key(provider)?;
                if provider.authentication == "api-key" && key.is_none() {
                    return Err("会話用ProviderのAPIキーがありません。".into());
                }
                let authorization = key
                    .as_deref()
                    .map(|key| zeroize::Zeroizing::new(format!("Bearer {key}")));
                let content = complete_http(
                    &provider.endpoint,
                    authorization.as_deref().map(String::as_str),
                    &provider.model,
                    &input.text,
                    route.timeout_ms,
                    provider.request_options.as_ref(),
                    false,
                )
                .await?;
                (content, provider.model.clone(), provider.label.clone())
            }
            _ => return Err("この会話用Provider形式は最小確認画面では未対応です。".into()),
        }
    };
    if content.trim().is_empty() || content.len() > 8192 {
        return Err("Providerの回答が空か、上限を超えました。".into());
    }
    state.sqlite_writer.write(|connection| {
        let transaction = connection.transaction().map_err(database_error)?;
        let now = now_iso();
        transaction
            .execute(
                "INSERT INTO conversation_messages(id,conversation_id,role,content,created_at) \
             VALUES(?1,?2,'user',?3,?4)",
                params![user_id, PRIMARY_CONVERSATION_ID, input.text, now],
            )
            .map_err(database_error)?;
        transaction
            .execute(
                "INSERT INTO conversation_messages(id,conversation_id,role,content,created_at) \
             VALUES(?1,?2,'assistant',?3,?4)",
                params![answer_id, PRIMARY_CONVERSATION_ID, content, now],
            )
            .map_err(database_error)?;
        transaction
            .execute(
                "UPDATE conversations SET updated_at=?1 WHERE id=?2",
                params![now, PRIMARY_CONVERSATION_ID],
            )
            .map_err(database_error)?;
        transaction.commit().map_err(database_error)
    })?;
    Ok(SubmitResult {
        content,
        model,
        provider_label,
    })
}

async fn complete_larm(
    providers: &crate::ModelProvidersSettings,
    text: &str,
    timeout_ms: u64,
) -> Result<(String, String), String> {
    let credential = crate::providers::dynamic_lan::credential::load()
        .map_err(|error| error.code().to_string())?;
    let preference = crate::providers::larm_resources::profile::preference(
        providers.harness.larm_profile.as_deref(),
    );
    let (_stop, receiver) = tokio::sync::watch::channel(false);
    let connection = saaa_larm_session::Session::connect_with_profile_credential_and_key(
        &providers.harness.address,
        preference,
        credential.token().to_string(),
        format!("saaa-conversation-check-{}", uuid::Uuid::new_v4().simple()),
        receiver,
    )
    .await;
    let session = match connection {
        Ok(session) => session,
        Err(error) => {
            if let Some(cleanup) = &error.cleanup {
                let _ = cleanup.close().await;
            }
            return Err(format!("LARMへの接続に失敗しました: {error}"));
        }
    };
    let answer = async {
        let lease = session
            .acquire("backchannel")
            .await
            .map_err(str::to_string)?;
        let provider = lease.provider();
        if provider.protocol != "openai.chat-completions.v1" {
            return Err("選択済みLARMの会話プロトコルに対応していません。".into());
        }
        let budget = lease
            .request_budget(std::time::Duration::from_millis(timeout_ms.min(120_000)))
            .map_err(str::to_string)?;
        let authorization = zeroize::Zeroizing::new(format!("Bearer {}", provider.token()));
        let content = complete_http(
            provider.base_url.as_str(),
            Some(authorization.as_str()),
            &provider.model,
            text,
            budget.as_millis() as u64,
            None,
            true,
        )
        .await?;
        Ok::<_, String>((content, provider.model.clone()))
    }
    .await;
    let closed = session.close().await;
    if closed.is_err() {
        return Err("LARM接続の解放を確認できませんでした。".into());
    }
    answer
}

async fn complete_http(
    endpoint: &str,
    authorization: Option<&str>,
    model: &str,
    text: &str,
    timeout_ms: u64,
    configured_options: Option<&saaa_larm_session::http_api::LlmOptions>,
    no_proxy: bool,
) -> Result<String, String> {
    let input = StartTurnInput {
        run_id: format!("check_{}", uuid::Uuid::new_v4().simple()),
        conversation_id: PRIMARY_CONVERSATION_ID.into(),
        content: text.into(),
        workspace_path: None,
        retry_input_message_id: None,
        source_id: None,
        scope_refs: Vec::new(),
        input_origin: "text".into(),
        presentation_mode: "visual".into(),
    };
    let history = [crate::ipc_contract::ConversationMessage {
        parts: None,
        id: input.run_id.clone(),
        conversation_id: input.conversation_id.clone(),
        role: "user".into(),
        content: text.into(),
        created_at: String::new(),
    }];
    let sink = tauri::ipc::Channel::new(|_| Ok(()));
    let options = configured_options.cloned().unwrap_or_default();
    crate::providers::chat_completions::run_with_proxy_policy(
        endpoint,
        authorization,
        model,
        &history,
        timeout_ms.min(120_000),
        crate::providers::stream::ModelStreamContext {
            reasoning_effort: "provider-default",
            max_output_tokens: 512,
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
        no_proxy,
    )
    .await
    .map_err(|error| match error {
        crate::providers::stream::ProviderAttemptError::Failed { kind, .. } => {
            kind.public_message().as_str().to_string()
        }
        _ => "Providerから回答を取得できませんでした。".into(),
    })
}
