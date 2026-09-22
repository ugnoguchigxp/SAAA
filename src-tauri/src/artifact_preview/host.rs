use tauri::webview::{NewWindowResponse, WebviewBuilder};
use tauri::{AppHandle, LogicalPosition, LogicalSize, Manager, WebviewUrl};
use url::Url;

use super::{
    contracts::{MountArtifactPreviewInput, PREVIEW_SCHEME},
    policy,
    tokens::PreviewRuntime,
};

pub(crate) fn document_url(token: &str) -> String {
    format!("{PREVIEW_SCHEME}://localhost/preview/{token}/index.html")
}

pub(crate) fn validate_bounds(x: f64, y: f64, width: f64, height: f64) -> Result<(), &'static str> {
    let finite = [x, y, width, height].iter().all(|value| value.is_finite());
    if !finite || width <= 0.0 || height <= 0.0 || width > 8_192.0 || height > 8_192.0 {
        return Err("preview-geometry");
    }
    if !(-16_384.0..=16_384.0).contains(&x) || !(-16_384.0..=16_384.0).contains(&y) {
        return Err("preview-geometry");
    }
    Ok(())
}

pub(crate) fn mount(
    app: &AppHandle,
    runtime: &PreviewRuntime,
    input: &MountArtifactPreviewInput,
) -> Result<(), String> {
    validate_bounds(input.x, input.y, input.width, input.height).map_err(str::to_string)?;
    let entry = runtime
        .lookup_active(&input.preview_token)
        .map_err(str::to_string)?;
    let window = app.get_window("main").ok_or("preview-unavailable")?;
    if let Some(existing) = window
        .webviews()
        .into_iter()
        .find(|webview| webview.label() == entry.webview_label)
    {
        existing
            .set_position(LogicalPosition::new(input.x, input.y))
            .map_err(|_| "preview-geometry".to_string())?;
        existing
            .set_size(LogicalSize::new(input.width, input.height))
            .map_err(|_| "preview-geometry".to_string())?;
        return Ok(());
    }
    let url = Url::parse(&document_url(&input.preview_token)).map_err(|_| "preview-unavailable")?;
    let navigation_runtime = runtime.clone();
    let navigation_token = input.preview_token.clone();
    let download_runtime = runtime.clone();
    let popup_runtime = runtime.clone();
    let builder = WebviewBuilder::new(entry.webview_label, WebviewUrl::CustomProtocol(url))
        .initialization_script(policy::DEVICE_DENY_SCRIPT)
        .incognito(true)
        .focused(false)
        .disable_drag_drop_handler()
        .devtools(cfg!(debug_assertions))
        .on_navigation(move |url| {
            let allowed = policy::allow_navigation(&navigation_token, url.as_str()).is_ok();
            if !allowed {
                navigation_runtime.record_denial("navigation-denied");
            }
            allowed
        })
        .on_download(move |_, _| {
            let allowed = policy::allow_download("download").is_ok();
            if !allowed {
                download_runtime.record_denial("download-denied");
            }
            allowed
        })
        .on_new_window(move |_, _| {
            if policy::allow_popup("popup").is_err() {
                popup_runtime.record_denial("popup-denied");
            }
            NewWindowResponse::Deny
        });
    window
        .add_child(
            builder,
            LogicalPosition::new(input.x, input.y),
            LogicalSize::new(input.width, input.height),
        )
        .map_err(|_| "preview-create-failed".to_string())?;
    Ok(())
}
