# Artifact WebView security results — 2026-09-22

Host-side observation is from Rust unit tests of prepare/release, protocol, and policy. Live WebView process network taps were not recorded.

## Isolation

- Capability file binds only webview label `main`. Child preview labels do not match.
- Custom protocol returns HTML only for an active in-memory token. Unknown, expired, traversal, and other files are 404 with empty body.
- Response headers: `Content-Type: text/html; charset=utf-8`, `Cache-Control: no-store`, `X-Content-Type-Options: nosniff`, `Referrer-Policy: no-referrer`, CSP as specified, and `Permissions-Policy` denying camera, microphone, geolocation, notifications, and clipboard.
- Navigation policy allows the custom-scheme preview URL and the Windows `http://saaa-artifact-preview.localhost` form for the same token. It denies `https`, other `http` hosts, `file`, `data`, `blob`, `about`, and other tokens. Popup and download handlers return deny.
- Reason codes are fixed strings. Tests assert they do not contain HTML, `http`, `file`, or the token.
- Adversarial fixtures exist for IPC and network attempts. They are not executed in a live child WebView in this evidence pack.

## Prepare rejections

Unknown JSON fields, missing revision, foreign scope, archived/deleted, `text/plain`, digest mismatch, NUL, invalid UTF-8, and payload over 1 MiB are rejected without returning HTML.

## Token ledger

Process memory only. Release is idempotent. Expiry recovers unused tokens. Maximum 8 concurrent tokens. Entries store revision identity, scope, digest, expiry, and webview label. Payload bytes are not stored.

## Negative evidence

Tauri 2.11 `WebviewBuilder` exposes navigation, popup, and download hooks. Those are attached when the child is created. It does not expose a permission-request hook. wry 0.55 on macOS grants `requestMediaCapturePermission` inside its UI delegate, so camera and microphone are not denied by the OS hook. The preview response sends `Permissions-Policy`, and the child runs a document-start script that replaces `mediaDevices`, `geolocation`, `clipboard`, and `Notification.requestPermission`. Context menu and link preview have no builder API. Live WKWebView / WebView2 / WebKitGTK attachment was not recorded in this session.
