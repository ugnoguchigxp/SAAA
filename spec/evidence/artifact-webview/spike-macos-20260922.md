# Artifact WebView spike — macOS 2026-09-22

Platform: macOS (WKWebView). Windows / Linux live runs were not executed on this host.

## WVP-00

Child preview is implemented as a Tauri 2 child WebView on the `main` window. The bundled fixture `artifact.fixture.interactive-html` is served from `saaa-artifact-preview://localhost/preview/<token>/index.html` (Windows maps to `http://saaa-artifact-preview.localhost/...`). Create, geometry follow, hide, and close are wired in `useArtifactWebview`. Unit tests cover create/close and tab switch cleanup. Live DPI / maximize / multi-monitor matrix is not recorded here.

## WVP-01 / WVP-02

`src-tauri/capabilities/default.json` targets `webviews: ["main"]` only. Identifier `main-webview`. Child labels `artifact-preview-*` receive no capability. Main keeps `core:default` and the webview position/size/show/hide/close allows. `llm-fetch:default` stays off the main webview because page fetch is a Rust-owned worker, not a frontend IPC command. No `windows: ["*"]` or `webviews: ["*"]`. `core:webview:allow-create-webview-window` is not granted.

Observed from ACL: a WebView whose label does not match a capability has no IPC. Child preview therefore cannot call SAAA commands, `core:*`, or `llm-fetch`.

## WVP-03 / WVP-04 / WVP-05

Frontend unit tests: geometry (scale, fractional, zero, offscreen, stale generation), lifecycle (unmount cleanup, switch), Artifact Workspace (semantic + interactive tabs, Escape, max 8).

## Scheme

macOS / Linux conceptual URL: `saaa-artifact-preview://localhost/preview/<token>/index.html`.
Windows: `http://saaa-artifact-preview.localhost/preview/<token>/index.html`.
The parser accepts both plus `saaa-artifact-preview://preview/<token>/index.html`.
