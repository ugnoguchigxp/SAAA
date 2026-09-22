use super::*;
#[tokio::test]
async fn tts_retries_transient_requests_but_stops_on_authentication() {
    for (primary_status, expected_fallbacks) in [(503, 1), (401, 0)] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count = hits.clone();
        let app = axum::Router::new()
            .route(
                "/primary/audio/speech",
                axum::routing::post(move || async move {
                    axum::http::StatusCode::from_u16(primary_status).unwrap()
                }),
            )
            .route(
                "/fallback/audio/speech",
                axum::routing::post(move || {
                    let count = count.clone();
                    async move {
                        count.fetch_add(1, Ordering::SeqCst);
                        axum::http::StatusCode::UNAUTHORIZED
                    }
                }),
            );
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let provider = |path: &str| {
            TtsRoute::Cloud(crate::CloudTtsProviderSettings {
                id: path.into(),
                enabled: true,
                label: path.into(),
                location: "local".into(),
                endpoint: format!("{base}/{path}"),
                model: "fixture".into(),
                voice: "fixture".into(),
                response_format: "wav".into(),
                authentication: "none".into(),
                style: None,
                speed: None,
                pitch_scale: None,
                intonation_scale: None,
            })
        };
        let connection = rusqlite::Connection::open_in_memory().unwrap();
        crate::initialize_database(&connection).unwrap();
        let state = crate::test_support::app_state(connection);
        let directory = tempfile::tempdir().unwrap();
        let context = RenderSessionContext {
            route: TtsRoute::Fallback(vec![provider("primary"), provider("fallback")], 80),
            timeout_ms: 400,
            cancellation: Arc::default(),
            child: Arc::new(Mutex::new(None)),
            cache_directory: directory.path().to_path_buf(),
            situation: state.situation.clone(),
            on_event: tauri::ipc::Channel::new(|_| Ok(())),
            run_id: "tts-fixture".into(),
            writer: None,
        };
        let (send, mut receive) = mpsc::channel(2);
        send.send(SpeechWork::Chunk {
            text: "synthetic".into(),
            boundary_at: Instant::now(),
        })
        .await
        .unwrap();
        send.send(SpeechWork::Finish).await.unwrap();
        let error = render(&mut receive, &context).await.unwrap_err();
        assert_eq!(
            error,
            crate::providers::stream::ProviderFailureKind::Authentication
                .public_message()
                .as_str()
        );
        assert_eq!(hits.load(Ordering::SeqCst), expected_fallbacks);
        server.abort();
    }
}
