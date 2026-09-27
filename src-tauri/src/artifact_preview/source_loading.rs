use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

use serde::Serialize;
use tauri::{ipc::Channel, webview::PageLoadEvent, Webview};

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum SourceLoadPhase {
    Loading,
    Preparing,
    Ready,
    Error,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SourceLoadEvent {
    navigation: u64,
    phase: SourceLoadPhase,
}

// This confirms the initial document's load and layout, not future SPA updates,
// animations, or every pixel reaching the compositor. No page IPC is enabled.
const CHECK_LAYOUT: &str = r#"(() => {
  if (!/^https?:$/.test(location.protocol) || document.readyState !== 'complete'
      || !document.documentElement || !document.body) return false;
  const bounds = document.documentElement.getBoundingClientRect();
  return bounds.width > 0 && bounds.height > 0;
})()"#;

pub(super) fn handler(
    events: Channel<SourceLoadEvent>,
) -> impl Fn(Webview, tauri::webview::PageLoadPayload<'_>) + Send + Sync + 'static {
    let generation = Arc::new(AtomicU64::new(0));
    move |webview, payload| {
        if payload.event() == PageLoadEvent::Started {
            let navigation = generation.fetch_add(1, Ordering::SeqCst) + 1;
            // On macOS Started means document commit, not the start of DNS/TLS.
            let _ = webview.hide();
            let _ = events.send(SourceLoadEvent {
                navigation,
                phase: SourceLoadPhase::Loading,
            });
            return;
        }
        let navigation = generation.load(Ordering::SeqCst);
        let _ = events.send(SourceLoadEvent {
            navigation,
            phase: SourceLoadPhase::Preparing,
        });
        let current = generation.clone();
        let callback_events = events.clone();
        if webview
            .eval_with_callback(CHECK_LAYOUT, move |result| {
                if current.load(Ordering::SeqCst) != navigation {
                    return;
                }
                let phase = if matches!(serde_json::from_str::<bool>(&result), Ok(true)) {
                    SourceLoadPhase::Ready
                } else {
                    SourceLoadPhase::Error
                };
                let _ = callback_events.send(SourceLoadEvent { navigation, phase });
            })
            .is_err()
        {
            let _ = events.send(SourceLoadEvent {
                navigation,
                phase: SourceLoadPhase::Error,
            });
        }
    }
}
