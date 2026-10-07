//! Lab process. One JSON document on stdin, one ready document on stdout, diagnostics on stderr.
use std::{future::IntoFuture, io::BufRead, net::SocketAddr, time::Duration};

use saaa_feature_lab::{authorized_host, open_lab, open_service, router, LabConfig};
use serde_json::json;
use tokio::net::TcpListener;

#[tokio::main]
async fn main() {
    let mut line = String::new();
    if std::io::stdin().lock().read_line(&mut line).is_err() {
        eprintln!("feature-lab: could not read startup configuration");
        std::process::exit(1);
    }
    let config: LabConfig = match serde_json::from_str(&line) {
        Ok(config) => config,
        Err(_) => {
            eprintln!("feature-lab: startup configuration was not valid JSON");
            std::process::exit(1);
        }
    };
    let allowed_origin = config.allowed_origin.clone();
    let open = match open_lab(config) {
        Ok(open) => open,
        Err(error) => {
            eprintln!("feature-lab: {error}");
            std::process::exit(1);
        }
    };
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("feature-lab: {error}");
            std::process::exit(1);
        }
    };
    let port = match listener.local_addr() {
        Ok(address) => address.port(),
        Err(error) => {
            eprintln!("feature-lab: {error}");
            std::process::exit(1);
        }
    };
    let host = match authorized_host(&allowed_origin) {
        Ok(host) => host,
        Err(error) => {
            eprintln!("feature-lab: {error}");
            std::process::exit(1);
        }
    };
    let running = match open_service(open, host) {
        Ok(running) => running,
        Err(error) => {
            eprintln!("feature-lab: {error}");
            std::process::exit(1);
        }
    };
    let (lab, _owner) = running.into_parts();
    println!("{}", json!({"ready": true, "port": port}));
    let service = lab.service.clone();
    let app = router(lab).into_make_service_with_connect_info::<SocketAddr>();
    let (stdin_tx, stdin_rx) = tokio::sync::oneshot::channel::<()>();
    std::thread::spawn(move || {
        let mut discarded = Vec::new();
        let _ = std::io::Read::read_to_end(&mut std::io::stdin(), &mut discarded);
        let _ = stdin_tx.send(());
    });
    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    let mut server_stop = stop_rx.clone();
    let server = axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            while !*server_stop.borrow() {
                if server_stop.changed().await.is_err() {
                    break;
                }
            }
        })
        .into_future();
    tokio::pin!(server);
    let mut stdin_rx = stdin_rx;
    tokio::select! {
        result = &mut server => {
            if let Err(error) = result {
                eprintln!("feature-lab: {error}");
            }
        }
        _ = async {
            tokio::select! {
                _ = &mut stdin_rx => {}
                _ = interrupt() => {}
            }
        } => {
            let _ = stop_tx.send(true);
            match tokio::time::timeout(Duration::from_secs(10), &mut server).await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => eprintln!("feature-lab: {error}"),
                Err(_) => eprintln!("feature-lab: http drain timed out"),
            }
        }
    }
    if let Err(error) = service.shutdown(Duration::from_secs(10)).await {
        eprintln!(
            "feature-lab: shutdown persistence failed: {}",
            error.message
        );
        std::process::exit(1);
    }
}

async fn interrupt() {
    let ctrl_c = async {
        if tokio::signal::ctrl_c().await.is_err() {
            std::future::pending::<()>().await
        }
    };
    #[cfg(unix)]
    {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut terminate) => {
                tokio::select! {
                    _ = ctrl_c => {}
                    _ = terminate.recv() => {}
                }
            }
            Err(_) => ctrl_c.await,
        }
    }
    #[cfg(not(unix))]
    ctrl_c.await;
}
