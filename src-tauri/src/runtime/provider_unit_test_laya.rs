//! Opt-in Laya diagnostics using saved settings read-only.
use super::*;
pub async fn run_saved_laya_unit_test(
    database: &std::path::Path,
    text: &str,
) -> Result<serde_json::Value, String> {
    let db =
        rusqlite::Connection::open_with_flags(database, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|_| "診断用DBを読み取り専用で開けませんでした。")?;
    let settings = persistence::load_model_providers(&db)?;
    let credential =
        crate::providers::dynamic_lan::credential::load().map_err(|e| e.code().to_string())?;
    let result = run_with_settings(
        &settings,
        credential.token(),
        "laya",
        text,
        None,
        false,
        &|stage| eprintln!("Laya diagnostic stage: {stage}"),
    )
    .await?;
    serde_json::to_value(result).map_err(|_| "Layaの結果を変換できませんでした。".into())
}

pub async fn run_saved_laya_avatar_test(
    database: &std::path::Path,
    text: &str,
) -> Result<serde_json::Value, String> {
    let db =
        rusqlite::Connection::open_with_flags(database, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|_| "診断用DBを読み取り専用で開けませんでした。")?;
    let settings = persistence::load_model_providers(&db)?;
    let credential =
        crate::providers::dynamic_lan::credential::load().map_err(|e| e.code().to_string())?;
    let state = serde_json::json!({"utterance":text});
    crate::providers::laya::choose(
        &settings.harness,
        credential.token(),
        &state,
        crate::providers::laya::speech_question(),
        &|_| {},
    )
    .await
    .map(|r| r.response)
}
