use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc, Mutex,
};

use axum::body::Body;
use axum::http::Request;
use saaa_media::{
    ExplicitSecrets, FixedAvailability, MediaService, MutexDb, SqlStore, SystemClock,
};
use saaa_provider_routing::LarmReachability;
use tower::ServiceExt;

use crate::fixture::FixtureBackend;
use crate::http::{open_service, router, LabHttp};
use crate::store::{load_snapshot, open_lab, LabConfig, LabOpen};
use crate::LabAuth;

fn config(path: &std::path::Path, provider: &str, token: Option<String>) -> LabConfig {
    LabConfig {
        database_path: path.display().to_string(),
        allowed_origin: "http://127.0.0.1:1422".into(),
        session_token: "0123456789abcdef0123456789abcdef".into(),
        provider: provider.into(),
        larm_endpoint: "http://127.0.0.1:9/".into(),
        larm_token: token,
        replace_route: false,
    }
}

#[test]
fn a_non_lab_database_is_refused_and_left_unchanged() {
    let directory = std::env::temp_dir().join(format!("saaa-lab-foreign-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("foreign.sqlite");
    {
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .execute("CREATE TABLE notes(id INTEGER PRIMARY KEY, body TEXT)", [])
            .unwrap();
        connection
            .execute("INSERT INTO notes(body) VALUES('keep')", [])
            .unwrap();
    }
    let before = std::fs::read(&path).unwrap();
    let Err(error) = open_lab(config(&path, "fixture", None)) else {
        panic!("a foreign database must be refused");
    };
    assert!(error.contains("not a feature lab"));
    assert_eq!(std::fs::read(&path).unwrap(), before);
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn reopening_keeps_the_existing_route() {
    let directory = std::env::temp_dir().join(format!("saaa-lab-reopen-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("lab.sqlite");
    let first = open_lab(config(&path, "fixture", None)).unwrap();
    let snapshot = load_snapshot(&first.connection).unwrap();
    drop(first);
    let second = open_lab(config(&path, "fixture", None)).unwrap();
    assert_eq!(
        serde_json::to_value(load_snapshot(&second.connection).unwrap()).unwrap(),
        serde_json::to_value(snapshot).unwrap()
    );
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn a_different_endpoint_is_refused_until_replace_route() {
    let directory = std::env::temp_dir().join(format!("saaa-lab-route-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("lab.sqlite");
    let fixture = "http://127.0.0.1:9/";
    let real = "http://127.0.0.1:10/";
    drop(open_lab(config(&path, "fixture", None)).unwrap());
    let mut next = config(&path, "larm", None);
    next.larm_endpoint = real.into();
    let Err(error) = open_lab(next) else {
        panic!("a saved fixture endpoint must not be reused silently");
    };
    assert!(error.contains(fixture));
    assert!(error.contains(real));
    assert!(!error.contains("0123456789abcdef0123456789abcdef"));
    let kept = open_lab(config(&path, "fixture", None)).unwrap();
    assert_eq!(
        load_snapshot(&kept.connection).unwrap().connections[0].endpoint,
        fixture
    );
    drop(kept);
    let mut switch = config(&path, "larm", None);
    switch.larm_endpoint = real.into();
    switch.replace_route = true;
    let switched = open_lab(switch).unwrap();
    assert_eq!(
        load_snapshot(&switched.connection).unwrap().connections[0].endpoint,
        real
    );
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn larm_without_an_explicit_token_is_refused() {
    let directory = std::env::temp_dir().join(format!("saaa-lab-token-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("lab.sqlite");
    let open = open_lab(config(&path, "larm", None)).unwrap();
    let Err(error) = open_service(open, "127.0.0.1:9".into()) else {
        panic!("larm without an explicit token must be refused");
    };
    assert!(error.contains("explicit token"));
    assert!(path.exists());
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn explicit_secrets_do_not_appear_in_debug() {
    let secret = "named-secret-value-should-stay-hidden";
    let token = "0123456789abcdef0123456789abcdef";
    let secrets = ExplicitSecrets::new(
        Some(token.into()),
        vec![("svc".into(), "acct".into(), secret.into())],
    )
    .unwrap();
    let rendered = format!("{secrets:?}");
    assert!(!rendered.contains(secret));
    assert!(!rendered.contains(token));
    assert!(rendered.contains("<redacted>"));
}

#[tokio::test]
async fn dropping_the_response_does_not_send_the_generation_again() {
    let directory = std::env::temp_dir().join(format!("saaa-lab-drop-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("lab.sqlite");
    let LabOpen {
        connection,
        config,
        owner: _owner,
    } = open_lab(config(&path, "fixture", None)).unwrap();
    let posts = Arc::new(AtomicUsize::new(0));
    let entered = Arc::new(AtomicBool::new(false));
    let (release, hold) = tokio::sync::oneshot::channel();
    let store = Arc::new(SqlStore::new(
        MutexDb::new(connection),
        Arc::new(load_snapshot),
    ));
    let service = Arc::new(MediaService::new(
        store,
        Arc::new(FixedAvailability(LarmReachability::Reachable)),
        Arc::new(SystemClock),
        Arc::new(FixtureBackend {
            posts: posts.clone(),
            entered: Some(entered.clone()),
            hold: Some(Arc::new(Mutex::new(Some(hold)))),
        }),
    ));
    let lab = LabHttp {
        service,
        auth: LabAuth {
            token: config.session_token.clone(),
            origin: config.allowed_origin.clone(),
            host: "127.0.0.1:9".into(),
        },
        posts: posts.clone(),
    };
    let app = router(lab.clone());
    let request_app = app.clone();
    let request_lab = lab.clone();
    let pending = tokio::spawn(async move { request_app.oneshot(post(&request_lab)).await });
    for _ in 0..200 {
        if entered.load(Ordering::SeqCst) {
            break;
        }
        tokio::task::yield_now().await;
    }
    if !entered.load(Ordering::SeqCst) {
        let response = pending.await.unwrap().unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        panic!(
            "stream ended early: {status} {}",
            String::from_utf8_lossy(&bytes)
        );
    }
    pending.abort();
    release.send(()).unwrap();
    let run = "00000000-0000-4000-8000-000000000099";
    for _ in 0..100 {
        let response = app.clone().oneshot(get(&lab, run)).await.unwrap();
        let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        if String::from_utf8(bytes.to_vec())
            .unwrap()
            .contains("\"status\":\"accepted\"")
        {
            break;
        }
        tokio::task::yield_now().await;
    }
    let again = app.oneshot(post(&lab)).await.unwrap();
    assert_eq!(again.status(), axum::http::StatusCode::CONFLICT);
    assert_eq!(posts.load(Ordering::SeqCst), 1);
    let _ = std::fs::remove_dir_all(directory);
}

fn post(lab: &LabHttp) -> Request<Body> {
    authed(
        lab,
        "POST",
        "/api/v1/media/runs",
        Some(r#"{"runId":"00000000-0000-4000-8000-000000000099","kind":"image","prompt":"円"}"#),
    )
}

fn get(lab: &LabHttp, run: &str) -> Request<Body> {
    authed(lab, "GET", &format!("/api/v1/media/runs?runId={run}"), None)
}

fn authed(lab: &LabHttp, method: &str, uri: &str, body: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "127.0.0.1:9")
        .header("origin", "http://127.0.0.1:1422")
        .header("cookie", format!("saaa_lab_session={}", lab.auth.token));
    if body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    let mut request = builder
        .body(Body::from(body.unwrap_or("").to_string()))
        .unwrap();
    request
        .extensions_mut()
        .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from((
            [127, 0, 0, 1],
            1,
        ))));
    request
}
