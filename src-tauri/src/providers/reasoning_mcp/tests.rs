use super::*;
#[test]
fn config_rejects_remote_and_embedded_credentials() {
    for url in [
        "https://example.com/mcp",
        "http://user:secret@localhost/mcp",
        "http://localhost/mcp?token=secret",
    ] {
        assert!(Client::new(url, "fixture-token-long-enough".into()).is_err());
    }
    assert!(Client::new(
        "http://127.0.0.1:8791/mcp",
        "fixture-token-long-enough".into()
    )
    .is_ok());
    assert!(Client::new("http://127.0.0.1:8791/mcp", "short".into()).is_err());
}

use std::sync::Mutex as StdMutex;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
pub(crate) struct Fixture {
    pub url: String,
    pub calls: Arc<StdMutex<Vec<Value>>>,
    pub entered: Arc<tokio::sync::Notify>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}
pub(crate) async fn fixture(mode: &'static str) -> Fixture {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/mcp", listener.local_addr().unwrap());
    let calls = Arc::new(StdMutex::new(Vec::new()));
    let captured = calls.clone();
    let entered = Arc::new(tokio::sync::Notify::new());
    let signal = entered.clone();
    let task = tokio::spawn(async move {
        let mut children = tokio::task::JoinSet::new();
        loop {
            let (mut stream, _) = listener.accept().await.unwrap();
            let captured = captured.clone();
            let signal = signal.clone();
            children.spawn(async move {
                let mut bytes=Vec::new();let mut chunk=[0;4096];
                let body=loop {
                    let n=stream.read(&mut chunk).await.unwrap();if n==0 {return;}
                    bytes.extend_from_slice(&chunk[..n]);
                    if let Some(end)=bytes.windows(4).position(|v|v==b"\r\n\r\n") {
                        let headers=String::from_utf8_lossy(&bytes[..end]).to_lowercase();
                        let length:usize=headers.lines().find_map(|l|l.strip_prefix("content-length:").map(str::trim)).unwrap().parse().unwrap();
                        if bytes.len()>=end+4+length {break serde_json::from_slice::<Value>(&bytes[end+4..end+4+length]).unwrap();}
                    }
                };
                captured.lock().unwrap().push(body.clone());
                let id=body["id"].clone();
                let result=match body["method"].as_str().unwrap() {
                    "initialize"=>{
                        if mode == "slow-init" { signal.notify_one(); tokio::time::sleep(Duration::from_millis(50)).await; }
                        json!({"protocolVersion":PROTOCOL,"capabilities":{"tools":{}}})
                    },
                    "tools/list"=>{
                        let mut tool = saaa_reasoning_contract::schema::tool();
                        if mode == "old-contract" { tool["inputSchema"]["properties"]["schemaVersion"]["const"] = json!("reasoning-answer-v1"); }
                        json!({"tools":[tool]})
                    },
                    "tools/call"=>{
                        signal.notify_one();
                        if mode=="slow" {tokio::time::sleep(Duration::from_secs(30)).await;}
                        let request:Request=serde_json::from_value(body["params"]["arguments"].clone()).unwrap();
                        let answer=saaa_reasoning_contract::Answer{intent:saaa_reasoning_contract::Intent::Answer,speech_text:"比較結果です。".into(),key_points:vec![],evidence_ids:vec![],limitations:vec![]};
                        let mut response=Response::bind(&request,answer).unwrap();
                        if mode=="stale" {response.context_revision+=1;}
                        json!({"isError":false,"structuredContent":response,"content":[]})
                    }
                    _=>{let _=stream.write_all(b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;return;}
                };
                let encoded=json!({"jsonrpc":"2.0","id":id,"result":result}).to_string();
                let reply=format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",encoded.len(),encoded);
                let _=stream.write_all(reply.as_bytes()).await;
            });
        }
    });
    Fixture {
        url,
        calls,
        entered,
        task,
    }
}
fn request() -> Request {
    serde_json::from_str(include_str!(
        "../../../../crates/reasoning-contract/fixtures/request.json"
    ))
    .unwrap()
}
#[tokio::test]
async fn reasoning_client_reuses_handshake_and_rejects_stale_results() {
    let server = fixture("good").await;
    let client = Client::new(&server.url, "fixture-token-long-enough".into()).unwrap();
    for _ in 0..2 {
        assert!(client.answer(&request(), Arc::default()).await.is_ok());
    }
    assert_eq!(
        server
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|c| c["method"] == "initialize")
            .count(),
        1
    );
    let server = fixture("stale").await;
    let client = Client::new(&server.url, "fixture-token-long-enough".into()).unwrap();
    assert!(client
        .answer(&request(), Arc::default())
        .await
        .unwrap_err()
        .contains("stale"));
}
#[tokio::test]
async fn reasoning_client_cancellation_reaches_mcp_and_finishes_promptly() {
    let server = fixture("slow").await;
    let client = Arc::new(Client::new(&server.url, "fixture-token-long-enough".into()).unwrap());
    let cancellation = Arc::new(RunCancellation::default());
    let token = cancellation.clone();
    let task = tokio::spawn(async move { client.answer(&request(), token).await });
    tokio::time::timeout(Duration::from_secs(2), server.entered.notified())
        .await
        .unwrap();
    cancellation.cancel();
    let result = tokio::time::timeout(Duration::from_secs(1), task)
        .await
        .unwrap()
        .unwrap();
    assert!(result.unwrap_err().contains("Cancelled"));
    assert!(server
        .calls
        .lock()
        .unwrap()
        .iter()
        .any(|c| c["method"] == "notifications/cancelled"));
}

#[tokio::test]
async fn wd_10_rejects_old_input_contract_before_sending_any_context() {
    let server = fixture("old-contract").await;
    let client = Client::new(&server.url, "fixture-token-long-enough".into()).unwrap();
    assert!(client.answer(&request(), Arc::default()).await.is_err());
    assert!(!server
        .calls
        .lock()
        .unwrap()
        .iter()
        .any(|r| r["method"] == "tools/call"));
}
