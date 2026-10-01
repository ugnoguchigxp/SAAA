use super::*;
use serde_json::json;

#[tauri::command]
fn fallback_probe() -> &'static str {
    "fallback works"
}

#[test]
fn media_commands_and_existing_fallback_are_reachable_through_real_ipc_dispatch() {
    let connection = rusqlite::Connection::open_in_memory().unwrap();
    crate::initialize_database(&connection).unwrap();
    let app = tauri::test::mock_builder()
        .manage(crate::test_support::app_state(connection))
        .invoke_handler(with_handler(tauri::generate_handler![fallback_probe]))
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    let window = tauri::WebviewWindowBuilder::new(&app, "media-test", Default::default())
        .build()
        .unwrap();
    let invoke = |cmd: &str, body| {
        tauri::test::get_ipc_response(
            &window,
            tauri::webview::InvokeRequest {
                cmd: cmd.into(),
                callback: tauri::ipc::CallbackFn(0),
                error: tauri::ipc::CallbackFn(1),
                url: "tauri://localhost".parse().unwrap(),
                body: tauri::ipc::InvokeBody::Json(body),
                headers: Default::default(),
                invoke_key: tauri::test::INVOKE_KEY.to_string(),
            },
        )
    };
    assert_eq!(
        invoke("generate_media", json!({"input":{"runId":uuid::Uuid::new_v4().to_string(),"kind":"image","prompt":""},"onProgress":"__CHANNEL__:1"})).unwrap_err(),
        json!("生成内容は1〜16384バイトで入力してください。")
    );
    for cmd in ["cancel_media_generation", "read_generated_media"] {
        assert_eq!(
            invoke(cmd, json!({"runId":"invalid", "artifactIndex":0})).unwrap_err(),
            json!("生成要求の識別子が不正です。")
        );
    }
    assert_eq!(
        invoke("fallback_probe", json!({}))
            .unwrap()
            .deserialize::<String>()
            .unwrap(),
        "fallback works"
    );
}
