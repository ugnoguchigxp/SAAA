#![cfg(test)]
use super::{catalog, contracts::*, host, policy, protocol, service, tokens::PreviewRuntime};
use serde_json::json;
use std::sync::{
    atomic::{AtomicI64, Ordering},
    Arc,
};
use tauri::http::{Request, StatusCode};

fn runtime_at(ms: i64) -> (PreviewRuntime, Arc<AtomicI64>) {
    let clock = Arc::new(AtomicI64::new(ms));
    let runtime = PreviewRuntime::with_clock({
        let clock = clock.clone();
        Arc::new(move || clock.load(Ordering::SeqCst))
    });
    (runtime, clock)
}

fn prepare_ok(runtime: &PreviewRuntime) -> ArtifactPreviewDescriptor {
    service::prepare(
        runtime,
        &PrepareArtifactPreviewInput {
            artifact_id: catalog::INTERACTIVE_HTML.artifact_id.into(),
            revision_id: catalog::INTERACTIVE_HTML.revision_id.into(),
        },
    )
    .expect("fixture prepares")
}

#[test]
fn unknown_fields_are_rejected() {
    let err = serde_json::from_value::<PrepareArtifactPreviewInput>(json!({
        "artifactId": "artifact.fixture.interactive-html",
        "revisionId": "rev.fixture.interactive-html.v1",
        "html": "<script>"
    }));
    assert!(err.is_err());
}

#[test]
fn prepare_rejects_scope_media_type_and_lifecycle() {
    let runtime = PreviewRuntime::default();
    for (artifact, revision, code) in [
        (
            "artifact.fixture.foreign-scope",
            "rev.fixture.foreign-scope.v1",
            "preview-scope-mismatch",
        ),
        (
            "artifact.fixture.plain",
            "rev.fixture.plain.v1",
            "preview-media-type",
        ),
        (
            "artifact.fixture.archived",
            "rev.fixture.archived.v1",
            "preview-unavailable",
        ),
        (
            "artifact.fixture.deleted",
            "rev.fixture.deleted.v1",
            "preview-unavailable",
        ),
        (
            "artifact.fixture.missing",
            "rev.fixture.interactive-html.v1",
            "preview-not-found",
        ),
    ] {
        let error = service::prepare(
            &runtime,
            &PrepareArtifactPreviewInput {
                artifact_id: artifact.into(),
                revision_id: revision.into(),
            },
        )
        .expect_err(code);
        assert_eq!(error, code);
        assert!(!error.contains('<'));
        assert!(!error.contains("http"));
    }
}

#[test]
fn digest_and_size_and_nul_are_rejected() {
    let payload = catalog::INTERACTIVE_HTML.payload;
    assert_eq!(
        service::validate_revision_bytes(payload, "00").unwrap_err(),
        "preview-digest-mismatch"
    );
    let mut oversized = Vec::from(&b"<html><body>"[..]);
    oversized.resize(MAX_HTML_BYTES + 8, b'a');
    oversized.extend_from_slice(b"</body></html>");
    assert_eq!(
        service::validate_revision_bytes(&oversized, "00").unwrap_err(),
        "preview-too-large"
    );
    assert_eq!(
        service::validate_revision_bytes(b"<html>\0</html>", "00").unwrap_err(),
        "preview-invalid-payload"
    );
}

#[test]
fn token_issue_expiry_release_and_limit() {
    let (runtime, clock) = runtime_at(1_000);
    let first = prepare_ok(&runtime);
    assert_eq!(runtime.active_count(), 1);
    service::release(&runtime, &first.preview_token);
    service::release(&runtime, &first.preview_token);
    assert_eq!(runtime.active_count(), 0);

    for _ in 0..MAX_ACTIVE_TOKENS {
        prepare_ok(&runtime);
    }
    let overflow = service::prepare(
        &runtime,
        &PrepareArtifactPreviewInput {
            artifact_id: catalog::INTERACTIVE_HTML.artifact_id.into(),
            revision_id: catalog::INTERACTIVE_HTML.revision_id.into(),
        },
    )
    .expect_err("limit");
    assert_eq!(overflow, "preview-token-limit");

    clock.store(1_000 + TOKEN_TTL_MS + 1, Ordering::SeqCst);
    assert_eq!(runtime.active_count(), 0);
    let expiring = prepare_ok(&runtime);
    clock.store(1_000 + TOKEN_TTL_MS + TOKEN_TTL_MS + 2, Ordering::SeqCst);
    assert_eq!(
        runtime.lookup_active(&expiring.preview_token).unwrap_err(),
        "token-expired"
    );
    assert_eq!(runtime.active_count(), 0);
}

#[test]
fn protocol_requires_token_and_sets_security_headers() {
    let runtime = PreviewRuntime::default();
    let descriptor = prepare_ok(&runtime);
    let uri = format!(
        "{PREVIEW_SCHEME}://localhost/preview/{}/index.html",
        descriptor.preview_token
    );
    let request = Request::builder().uri(&uri).body(Vec::new()).unwrap();
    let response = protocol::respond(&runtime, request);
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get("content-type").unwrap(),
        "text/html; charset=utf-8"
    );
    assert_eq!(response.headers().get("cache-control").unwrap(), "no-store");
    assert_eq!(
        response.headers().get("x-content-type-options").unwrap(),
        "nosniff"
    );
    assert_eq!(
        response.headers().get("referrer-policy").unwrap(),
        "no-referrer"
    );
    assert_eq!(
        response.headers().get("content-security-policy").unwrap(),
        CSP
    );
    assert_eq!(
        response.headers().get("permissions-policy").unwrap(),
        PERMISSIONS_POLICY
    );
    assert_eq!(
        response.body().as_slice(),
        catalog::INTERACTIVE_HTML.payload
    );

    let missing = Request::builder()
        .uri(format!(
            "{PREVIEW_SCHEME}://localhost/preview/{}/index.html",
            "ab".repeat(32)
        ))
        .body(Vec::new())
        .unwrap();
    assert_eq!(
        protocol::respond(&runtime, missing).status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(runtime.last_denial(), Some("token-unknown"));

    let traversal = Request::builder()
        .uri(format!(
            "{PREVIEW_SCHEME}://localhost/preview/../secrets/index.html"
        ))
        .body(Vec::new())
        .unwrap();
    assert_eq!(
        protocol::respond(&runtime, traversal).status(),
        StatusCode::NOT_FOUND
    );

    let other = Request::builder()
        .uri(format!(
            "{PREVIEW_SCHEME}://localhost/preview/{}/other.html",
            descriptor.preview_token
        ))
        .body(Vec::new())
        .unwrap();
    assert_eq!(
        protocol::respond(&runtime, other).status(),
        StatusCode::NOT_FOUND
    );
}

#[test]
fn mount_bounds_reject_non_finite_and_empty_rects() {
    assert!(host::validate_bounds(10.0, 20.0, 300.0, 200.0).is_ok());
    assert_eq!(
        host::validate_bounds(0.0, 0.0, 0.0, 10.0).unwrap_err(),
        "preview-geometry"
    );
    assert_eq!(
        host::validate_bounds(f64::NAN, 0.0, 10.0, 10.0).unwrap_err(),
        "preview-geometry"
    );
}

#[test]
fn navigation_and_device_policy_deny_external_targets() {
    let token = "a".repeat(64);
    let ok = format!("{PREVIEW_SCHEME}://localhost/preview/{token}/index.html#ok");
    assert!(policy::allow_navigation(&token, &ok).is_ok());
    let windows = format!("http://{PREVIEW_SCHEME}.localhost/preview/{token}/index.html");
    assert!(policy::allow_navigation(&token, &windows).is_ok());
    for denied in [
        "https://example.invalid/",
        "http://example.invalid/",
        "file:///etc/hosts",
        "data:text/html,x",
        "blob:https://example.invalid/1",
        &format!("https://{PREVIEW_SCHEME}.localhost/preview/{token}/index.html"),
        &format!(
            "{PREVIEW_SCHEME}://localhost/preview/{}/index.html",
            "b".repeat(64)
        ),
    ] {
        assert_eq!(
            policy::allow_navigation(&token, denied).unwrap_err(),
            "navigation-denied"
        );
        let reason = policy::sanitize_reason("navigation-denied");
        assert!(!reason.contains("http"));
        assert!(!reason.contains("file"));
        assert!(!reason.contains(&token));
    }
    assert_eq!(
        policy::allow_popup("https://x").unwrap_err(),
        "popup-denied"
    );
    assert_eq!(
        policy::allow_download("https://x").unwrap_err(),
        "download-denied"
    );
    assert_eq!(
        policy::allow_permission("geolocation").unwrap_err(),
        "permission-denied"
    );
    assert!(policy::DEVICE_DENY_SCRIPT.contains("getUserMedia"));
    assert!(policy::DEVICE_DENY_SCRIPT.contains("geolocation"));
    assert!(policy::DEVICE_DENY_SCRIPT.contains("clipboard"));
    assert!(policy::DEVICE_DENY_SCRIPT.contains("requestPermission"));
}

#[test]
fn capability_file_targets_main_webview_only() {
    let value: serde_json::Value =
        serde_json::from_str(include_str!("../../capabilities/default.json")).unwrap();
    assert_eq!(value["identifier"], "main-webview");
    assert_eq!(value["webviews"], json!(["main"]));
    assert!(value.get("windows").is_none());
    let permissions = value["permissions"].as_array().unwrap();
    assert!(permissions.iter().all(|item| item.as_str().is_some_and(
        |name| !name.contains('*') && name != "core:webview:allow-create-webview-window"
    )));
    for required in [
        "core:default",
        "core:webview:allow-set-webview-position",
        "core:webview:allow-set-webview-size",
        "core:webview:allow-webview-show",
        "core:webview:allow-webview-hide",
        "core:webview:allow-webview-close",
    ] {
        assert!(permissions.iter().any(|item| item == required));
    }
}

#[test]
fn registered_commands_are_listed_for_reachability_audit() {
    let source = include_str!("../runtime/command_registry.rs");
    assert!(source.contains("prepare_artifact_preview"));
    assert!(source.contains("release_artifact_preview"));
    assert!(source.contains("mount_artifact_preview"));
    assert!(!source.contains("windows: [\"*\"]"));
}
