//! Manual end-to-end gate for the WebFetch WebView migration (W2/W4/W10).
//!
//! Real Tauri app (main thread, hidden worker) that drives the SAAA stack:
//! plugin manager → `TauriWebViewContentFetcher` / `RustSearchProvider` →
//! compact projection. Flow: live `web_search` → first hit URL →
//! `fetch_content` → assert `untrusted`/`tainted` schema.
//!
//! Run: `SAAA_WEBFETCH_BACKEND=webview ./target/debug/webfetch_e2e`
//! Exit 0 = pass. Not wired into offline gates (needs network + WebView).

use std::sync::Arc;
use std::time::Duration;

use saaa_lib::runtime::agent_tools::AgentToolCall;
use saaa_lib::runtime::web_fetch::contracts::WebFetchCancel;
use saaa_lib::runtime::web_fetch::{execute_with_cancel, install_runtime, WebFetchRuntime};
use tauri_plugin_llm_fetch::LlmFetchExt;

fn main() {
    let backend = std::env::var("SAAA_WEBFETCH_BACKEND").unwrap_or_else(|_| "auto".to_string());
    if backend != "webview" && backend != "auto" {
        eprintln!("webfetch_e2e requires SAAA_WEBFETCH_BACKEND=webview (or auto on macOS)");
        std::process::exit(2);
    }
    let result = tauri::Builder::default()
        .plugin(tauri_plugin_llm_fetch::init())
        .setup(|app| {
            let manager = app.llm_fetch().0.clone();
            let content = Arc::new(
                saaa_lib::runtime::web_fetch::content::TauriWebViewContentFetcher::new(manager),
            );
            let search = saaa_lib::runtime::web_fetch::search::RustSearchProvider::new()
                .map_err(|_| std::io::Error::other("search client failed to build"))?;
            install_runtime(WebFetchRuntime {
                content,
                search: Arc::new(search),
            });
            Ok(())
        })
        .build(tauri::generate_context!())
        .map_err(|error| {
            eprintln!("webfetch_e2e: app build failed: {error}");
            std::process::exit(2);
        })
        .expect("app builds");
    // Drive the run on the same thread that owns the app handle, then exit.
    let code = result.run_return(|app, event| {
        if matches!(event, tauri::RunEvent::Ready) {
            let handle = app.clone();
            std::thread::spawn(move || {
                let code = tauri::async_runtime::block_on(async { run_flow().await });
                handle.exit(code);
            });
        }
    });
    std::process::exit(code);
}

async fn run_flow() -> i32 {
    // 1. Live search through the SAAA dispatcher (Rust provider).
    let search_call = AgentToolCall {
        id: "e2e-search".to_string(),
        name: "web_search".to_string(),
        arguments: r#"{"query":"rust programming language","limit":5}"#.to_string(),
    };
    let search_json = execute_with_cancel(
        &search_call,
        Duration::from_secs(60),
        WebFetchCancel::never(),
    )
    .await;
    println!("SEARCH_RESULT={search_json}");
    let search_value: serde_json::Value = match serde_json::from_str(&search_json) {
        Ok(value) => value,
        Err(_) => {
            eprintln!("webfetch_e2e: search output is not JSON");
            return 1;
        }
    };
    if search_value.pointer("/type").and_then(|v| v.as_str()) != Some("web_search_result") {
        eprintln!("webfetch_e2e: search did not return web_search_result: {search_json}");
        return 1;
    }
    if search_value
        .pointer("/security/tainted")
        .and_then(|v| v.as_bool())
        != Some(true)
    {
        eprintln!("webfetch_e2e: search result is not tainted");
        return 1;
    }
    let mut follow_url: Option<String> = None;
    if let Some(hits) = search_value.pointer("/hits").and_then(|v| v.as_array()) {
        for hit in hits {
            let url = hit.pointer("/url").and_then(|v| v.as_str()).unwrap_or("");
            // Skip ad/tracker URLs (DDG `y.js`, bing `aclick`): the model may
            // pick any hit, but the E2E gate needs a fetchable document.
            // Over-long URLs are rejected by strict input validation anyway.
            if url.len() <= 1024
                && !url.contains("/y.js?")
                && !url.contains("aclick")
                && !url.contains("ad_domain")
            {
                follow_url = Some(url.to_string());
                break;
            }
        }
    }
    let Some(url) = follow_url else {
        eprintln!("webfetch_e2e: no fetchable search hits to follow");
        return 1;
    };
    println!("FOLLOW_URL={url}");

    // 2. Content fetch of the search-returned URL through the real worker.
    let fetch_call = AgentToolCall {
        id: "e2e-fetch".to_string(),
        name: "fetch_content".to_string(),
        arguments: serde_json::json!({"url": url, "maxCharacters": 2000}).to_string(),
    };
    let fetch_json = execute_with_cancel(
        &fetch_call,
        Duration::from_secs(90),
        WebFetchCancel::never(),
    )
    .await;
    println!(
        "FETCH_RESULT={}",
        fetch_json.chars().take(2000).collect::<String>()
    );
    let fetch_value: serde_json::Value = match serde_json::from_str(&fetch_json) {
        Ok(value) => value,
        Err(_) => {
            eprintln!("webfetch_e2e: fetch output is not JSON");
            return 1;
        }
    };
    if fetch_value.pointer("/type").and_then(|v| v.as_str()) != Some("fetch_content_result") {
        eprintln!("webfetch_e2e: fetch did not return fetch_content_result");
        return 1;
    }
    if fetch_value
        .pointer("/security/tainted")
        .and_then(|v| v.as_bool())
        != Some(true)
    {
        eprintln!("webfetch_e2e: fetch result is not tainted");
        return 1;
    }
    let text_len = fetch_value
        .pointer("/document/text")
        .and_then(|v| v.as_str())
        .map(|text| text.len())
        .unwrap_or(0);
    if text_len == 0 {
        eprintln!("webfetch_e2e: fetched document text is empty");
        return 1;
    }
    println!("webfetch_e2e: PASS (search hits + {text_len} chars fetched, all untrusted/tainted)");
    0
}
