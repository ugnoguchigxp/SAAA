use crate::RunCancellation;
use std::sync::Arc;
use std::time::Duration;
use url::Url;

/// control plane の state endpoint に GET するだけ。claim も model probe もしない。
pub(crate) async fn reachable(host: &str, timeout: Duration) -> bool {
    let Ok(base) = super::control_base_url(host) else {
        eprintln!("dynamic_lan reachability probe rejected host");
        return false;
    };
    reachable_at(base, timeout).await
}

pub(crate) async fn reachable_at(base: Url, timeout: Duration) -> bool {
    let Ok(url) = base.join("v3/agent-profiles") else {
        return false;
    };
    let client = match reqwest::Client::builder()
        .connect_timeout(timeout)
        .timeout(timeout)
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            eprintln!("dynamic_lan reachability probe client: {error}");
            return false;
        }
    };
    let mut request = client.get(url);
    match super::http::control_credential() {
        Ok(credential) => {
            request = request.header(reqwest::header::AUTHORIZATION, credential);
        }
        Err(error) => {
            eprintln!(
                "dynamic_lan reachability probe credential: {}",
                error.public_message()
            );
            return false;
        }
    }
    match request.send().await {
        Ok(_) => true,
        Err(error) => {
            eprintln!("dynamic_lan reachability probe failed: {error}");
            false
        }
    }
}

/// Same check as a live Agent Connection: claim, then the model readiness probe.
/// A chat-completion request is not part of reachability.
pub(crate) async fn probe(provider: &crate::DynamicLanProviderSettings) -> Result<String, String> {
    let connection = crate::providers::dynamic_lan::DynamicLanConnection::resolve(
        &provider.host,
        None,
        Arc::new(RunCancellation::default()),
    )
    .await
    .map_err(|error| error.public_message().to_string())?;
    connection
        .release()
        .await
        .map_err(|error| error.public_message().to_string())?;
    Ok("Agent connection and model readiness probe succeeded.".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    struct TokenGuard {
        previous: Option<String>,
    }

    impl TokenGuard {
        fn set() -> Self {
            let previous = std::env::var("LARM_API_TOKEN").ok();
            std::env::set_var("LARM_API_TOKEN", "test-control-token");
            Self { previous }
        }
    }

    impl Drop for TokenGuard {
        fn drop(&mut self) {
            match self.previous.take() {
                Some(token) => std::env::set_var("LARM_API_TOKEN", token),
                None => std::env::remove_var("LARM_API_TOKEN"),
            }
        }
    }

    fn serve(status: &'static str) -> (Url, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let server = std::thread::spawn(move || {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let mut buffer = [0_u8; 1024];
            let _ = stream.read(&mut buffer);
            let body = "{}";
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes());
        });
        (
            Url::parse(&format!("http://{address}/")).expect("url"),
            server,
        )
    }

    #[tokio::test]
    async fn rr_ls_04_reachable_at_classifies_http_and_refusal() {
        let _guard = TokenGuard::set();
        let (ok_url, ok_server) = serve("200 OK");
        assert!(reachable_at(ok_url, Duration::from_secs(2)).await);
        ok_server.join().expect("200 server");

        let (denied_url, denied_server) = serve("401 Unauthorized");
        assert!(reachable_at(denied_url, Duration::from_secs(2)).await);
        denied_server.join().expect("401 server");

        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        drop(listener);
        let refused = Url::parse(&format!("http://{address}/")).expect("url");
        assert!(!reachable_at(refused, Duration::from_millis(200)).await);
    }
}
