//! Provider-specific live ASR transport. The conversation capture and server share the configured silence boundary.
use std::{collections::HashMap, sync::OnceLock};

use base64::{engine::general_purpose::STANDARD, Engine};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::ipc::Channel;
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{client::IntoClientRequest, http::header::AUTHORIZATION, Message},
};

use crate::{persistence, AppState, ModelProviderSettings};

const PACKET_BYTES: usize = 3_200; // 100 ms, 16 kHz, mono PCM16LE
const MAX_EVENT_BYTES: usize = 64 * 1024;
const FINISH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
const KEEPALIVE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(20);

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct StartInput {
    session_id: String,
    #[serde(default = "default_silence_timeout_ms")]
    silence_timeout_ms: u64,
}

fn default_silence_timeout_ms() -> u64 {
    1_500
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct AudioInput {
    session_id: String,
    utterance_id: String,
    audio: Vec<u8>,
}

#[derive(Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub(crate) enum Event {
    Ready {
        session_id: String,
    },
    SpeechStarted {
        session_id: String,
        utterance_id: String,
    },
    Partial {
        session_id: String,
        utterance_id: String,
        text: String,
    },
    Final {
        session_id: String,
        utterance_id: String,
        text: String,
        language: Option<String>,
    },
    Failed {
        session_id: String,
        utterance_id: Option<String>,
        message: String,
    },
    Stopped {
        session_id: String,
    },
}

enum Command {
    Audio {
        utterance_id: String,
        audio: Vec<u8>,
    },
    Stop {
        done: oneshot::Sender<Result<(), String>>,
    },
}

type Sessions = Mutex<HashMap<String, mpsc::Sender<Command>>>;
static SESSIONS: OnceLock<Sessions> = OnceLock::new();
#[path = "qwen_realtime_asr/selection.rs"]
mod selection;
use selection::selected_provider;

fn sessions() -> &'static Sessions {
    SESSIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

#[tauri::command]
pub(crate) fn conversation_asr_transport(
    state: tauri::State<'_, AppState>,
) -> Result<String, String> {
    Ok(
        selected_provider(&state)?
            .map_or_else(|| "http".to_string(), |provider| provider.transport),
    )
}

#[tauri::command]
pub(crate) async fn start_qwen_asr_session(
    state: tauri::State<'_, AppState>,
    input: StartInput,
    on_event: Channel<Event>,
) -> Result<(), String> {
    crate::validate_identifier(&input.session_id, "ASR session id")?;
    if !(800..=3_000).contains(&input.silence_timeout_ms) {
        return Err("ASRの無音時間は800〜3000msにしてください。".into());
    }
    let provider = selected_provider(&state)?.ok_or("Qwen ASR Providerが選択されていません。")?;
    if provider.transport != "qwen-realtime" {
        return Err("選択済みASR Providerはライブ入力に対応していません。".into());
    }
    if sessions().lock().await.contains_key(&input.session_id) {
        return Err("ASRセッションはすでに開始されています。".into());
    }
    let pinned = state.sqlite_readers.read(|db| {
        let loaded = persistence::service_registry_store::load_registry(db)?;
        crate::providers::service_registry::resolve_route(
            &loaded.snapshot,
            crate::providers::service_registry::Purpose::VoiceTranscribe,
        )
        .map_err(|_| "音声入力の用途設定が無効です".to_string())
    })?;
    let readers = state.sqlite_readers.clone();
    let gate = super::conversation_speaker::prepare(&state)?;
    let mut socket = connect_provider(&provider).await?;
    initialize_session(&mut socket, input.silence_timeout_ms).await?;
    let (sender, receiver) = mpsc::channel(16);
    let mut active = sessions().lock().await;
    if active.contains_key(&input.session_id) {
        drop(active);
        let _ = socket.close(None).await;
        return Err("ASRセッションはすでに開始されています。".into());
    }
    active.insert(input.session_id.clone(), sender);
    drop(active);
    tokio::spawn(run_session(
        input.session_id,
        socket,
        receiver,
        on_event,
        gate,
        readers,
        pinned,
    ));
    Ok(())
}

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn connect_provider(provider: &crate::CloudAsrProviderSettings) -> Result<Socket, String> {
    let mut url = url::Url::parse(&provider.endpoint).map_err(|_| "Qwen ASR URLが不正です。")?;
    if url.scheme() != "https" || url.query().is_some() || url.fragment().is_some() {
        return Err("Qwen ASRにはクエリのないHTTPSエンドポイントを指定してください。".into());
    }
    url.set_scheme("wss")
        .map_err(|_| "Qwen ASR URLを変換できません。")?;
    url.query_pairs_mut().append_pair("model", &provider.model);
    let key = crate::credentials::load_api_key(&provider.id)?
        .ok_or("Qwen ASRのAPIキーが設定されていません。")?;
    let mut request = url
        .as_str()
        .into_client_request()
        .map_err(|_| "Qwen ASRリクエストが不正です。")?;
    request.headers_mut().insert(
        AUTHORIZATION,
        format!("Bearer {}", key.as_str())
            .parse()
            .map_err(|_| "Qwen ASRのAPIキーが不正です。")?,
    );
    let (socket, _) =
        tokio::time::timeout(std::time::Duration::from_secs(10), connect_async(request))
            .await
            .map_err(|_| "Qwen ASRへの接続が時間切れになりました。")?
            .map_err(|_| "Qwen ASRへの接続に失敗しました。")?;
    Ok(socket)
}

pub(crate) async fn probe(provider: &crate::CloudAsrProviderSettings) -> Result<String, String> {
    let mut socket = connect_provider(provider).await?;
    initialize_session(&mut socket, default_silence_timeout_ms()).await?;
    socket
        .send(Message::Text(
            json!({"event_id":uuid::Uuid::new_v4().to_string(), "type":"session.finish"})
                .to_string()
                .into(),
        ))
        .await
        .map_err(|_| "Qwen ASRの接続テストに失敗しました。")?;
    let response = tokio::time::timeout(FINISH_TIMEOUT, async {
        while let Some(message) = socket.next().await {
            let message = message.map_err(|_| "Qwen ASRの接続テストに失敗しました。")?;
            if let Message::Text(text) = message {
                let value: Value =
                    serde_json::from_str(&text).map_err(|_| "Qwen ASRの応答が不正です。")?;
                if value["type"] == "session.finished" {
                    return Ok(());
                }
                if value["type"] == "error" {
                    return Err("Qwen ASRが接続テストを拒否しました。".into());
                }
            }
        }
        Err("Qwen ASRの接続が終了しました。".into())
    })
    .await
    .map_err(|_| "Qwen ASRの接続テストが時間切れになりました。")?;
    let _ = socket.close(None).await;
    response.map(|()| "Qwen ASR Realtimeに接続できました。".into())
}

async fn initialize_session(socket: &mut Socket, silence_timeout_ms: u64) -> Result<(), String> {
    socket
        .send(session_update(silence_timeout_ms))
        .await
        .map_err(|_| "Qwen ASRのセッション設定を送信できませんでした。")?;
    tokio::time::timeout(FINISH_TIMEOUT, async {
        while let Some(message) = socket.next().await {
            let message =
                message.map_err(|_| "Qwen ASRのセッション設定を確認できませんでした。")?;
            if let Message::Text(text) = message {
                if text.len() > MAX_EVENT_BYTES {
                    return Err("Qwen ASRイベントが大きすぎます。".into());
                }
                let value: Value =
                    serde_json::from_str(&text).map_err(|_| "Qwen ASRの応答が不正です。")?;
                if value["type"] == "session.updated" {
                    return Ok::<(), String>(());
                }
                if value["type"] == "error" {
                    return Err("Qwen ASRがセッション設定を拒否しました。".into());
                }
            }
        }
        Err("Qwen ASRの接続が終了しました。".into())
    })
    .await
    .map_err(|_| "Qwen ASRのセッション設定が時間切れになりました。")??;
    Ok(())
}

#[tauri::command]
pub(crate) async fn append_qwen_asr_audio(input: AudioInput) -> Result<(), String> {
    crate::validate_identifier(&input.session_id, "ASR session id")?;
    crate::validate_identifier(&input.utterance_id, "utterance id")?;
    if input.audio.len() != PACKET_BYTES {
        return Err("ASR音声パケットの長さが不正です。".into());
    }
    let sender = sessions()
        .lock()
        .await
        .get(&input.session_id)
        .cloned()
        .ok_or("ASRセッションが見つかりません。")?;
    sender
        .send(Command::Audio {
            utterance_id: input.utterance_id,
            audio: input.audio,
        })
        .await
        .map_err(|_| "ASRセッションが終了しました。".into())
}

#[tauri::command]
pub(crate) async fn stop_qwen_asr_session(session_id: String) -> Result<(), String> {
    crate::validate_identifier(&session_id, "ASR session id")?;
    let sender = sessions()
        .lock()
        .await
        .get(&session_id)
        .cloned()
        .ok_or("ASRセッションが見つかりません。")?;
    let (done, wait) = oneshot::channel();
    sender
        .send(Command::Stop { done })
        .await
        .map_err(|_| "ASRセッションが終了しました。")?;
    tokio::time::timeout(FINISH_TIMEOUT, wait)
        .await
        .map_err(|_| "Qwen ASRの終了を確認できませんでした。")?
        .map_err(|_| "Qwen ASRの終了を確認できませんでした。")?
}

#[derive(Clone)]
struct Range {
    utterance_id: String,
    start_ms: u64,
    end_ms: u64,
}

fn utterance_for_start(ranges: &[Range], start_ms: u64) -> Option<&str> {
    ranges
        .iter()
        .find(|range| range.start_ms <= start_ms && start_ms < range.end_ms)
        .map(|range| range.utterance_id.as_str())
}

fn session_update(silence_timeout_ms: u64) -> Message {
    Message::Text(
        json!({"event_id": uuid::Uuid::new_v4().to_string(), "type":"session.update", "session":{
            "input_audio_format":"pcm", "sample_rate":16000,
            "turn_detection":{"type":"server_vad", "silence_duration_ms":silence_timeout_ms}
        }})
        .to_string()
        .into(),
    )
}

async fn run_session(
    session_id: String,
    socket: Socket,
    mut receiver: mpsc::Receiver<Command>,
    events: Channel<Event>,
    mut gate: super::streaming_asr::speaker_gate_runtime::SpeakerGate,
    readers: persistence::SqliteReaders,
    pinned: crate::providers::service_registry::ResolvedRoute,
) {
    let (mut writer, mut reader) = socket.split();
    let mut ranges: Vec<Range> = Vec::new();
    let mut item_ids: HashMap<String, String> = HashMap::new();
    let mut audio_ms = 0u64;
    let mut pending_ids = std::collections::VecDeque::new();
    let mut stop_reply: Option<oneshot::Sender<Result<(), String>>> = None;
    let mut finish_at: Option<tokio::time::Instant> = None;
    let mut keepalive = tokio::time::interval(KEEPALIVE_INTERVAL);
    keepalive.tick().await;
    let _ = events.send(Event::Ready {
        session_id: session_id.clone(),
    });
    let result: Result<(), String> = 'session: loop {
        tokio::select! {
            _ = keepalive.tick() => {
                if writer.send(Message::Ping(Vec::new().into())).await.is_err() {
                    break Err("Qwen ASR接続の維持に失敗しました。".into());
                }
            }
            _ = tokio::time::sleep_until(finish_at.unwrap_or_else(|| tokio::time::Instant::now() + std::time::Duration::from_secs(86400))), if finish_at.is_some() => {
                break Err("Qwen ASRの終了を確認できませんでした。".into());
            }
            command = receiver.recv() => match command {
                Some(Command::Audio { utterance_id, audio }) => {
                    if let Err(error)=readers.read(|db|crate::providers::service_registry::validate_active(db,&pinned)) { break 'session Err(error); }
                    pending_ids.push_back(utterance_id);
                    for audio in gate.push(zeroize::Zeroizing::new(audio)).await {
                    let utterance_id = pending_ids.pop_front().expect("one id per gated packet");
                    if let Some(last) = ranges.last_mut().filter(|range| range.utterance_id == utterance_id) {
                        last.end_ms += 100;
                    } else {
                        ranges.push(Range { utterance_id, start_ms: audio_ms, end_ms: audio_ms + 100 });
                        if ranges.len() > 64 { ranges.remove(0); }
                    }
                    audio_ms += 100;
                    let payload = json!({"event_id":uuid::Uuid::new_v4().to_string(), "type":"input_audio_buffer.append", "audio":STANDARD.encode(audio.as_slice())});
                    if writer.send(Message::Text(payload.to_string().into())).await.is_err() { break 'session Err::<(), String>("Qwen ASRへの音声送信に失敗しました。".into()); }
                    }
                }
                Some(Command::Stop { done }) => {
                    if stop_reply.is_some() {
                        let _ = done.send(Err("Qwen ASRは終了処理中です。".into()));
                        continue;
                    }
                    stop_reply = Some(done);
                    finish_at = Some(tokio::time::Instant::now() + FINISH_TIMEOUT);
                    for audio in gate.flush().await {
                        let utterance_id = pending_ids.pop_front().expect("one id per gated packet");
                        if let Some(last) = ranges.last_mut().filter(|r| r.utterance_id == utterance_id) { last.end_ms += 100; }
                        else { ranges.push(Range { utterance_id, start_ms: audio_ms, end_ms: audio_ms + 100 }); }
                        audio_ms += 100;
                        let payload = json!({"event_id":uuid::Uuid::new_v4().to_string(), "type":"input_audio_buffer.append", "audio":STANDARD.encode(audio.as_slice())});
                        if writer.send(Message::Text(payload.to_string().into())).await.is_err() { break 'session Err("Qwen ASRへの音声送信に失敗しました。".into()); }
                    }
                    let payload = json!({"event_id":uuid::Uuid::new_v4().to_string(), "type":"session.finish"});
                    if writer.send(Message::Text(payload.to_string().into())).await.is_err() { break Err("Qwen ASRを終了できませんでした。".into()); }
                }
                None => break Err("Qwen ASRの入力が終了しました。".into()),
            },
            message = reader.next() => match message {
                Some(Ok(Message::Text(text))) if text.len() <= MAX_EVENT_BYTES => {
                    let value: Value = match serde_json::from_str(&text) { Ok(value) => value, Err(_) => break Err("Qwen ASRから不正なイベントを受信しました。".into()) };
                    let kind = value["type"].as_str().unwrap_or("");
                    let item_id = value["item_id"].as_str().unwrap_or("");
                    if kind == "input_audio_buffer.speech_started" {
                        let start_ms = value["audio_start_ms"].as_u64().unwrap_or(audio_ms);
                        if let Some(id) = utterance_for_start(&ranges, start_ms) {
                            item_ids.insert(item_id.into(), id.into());
                            let _ = events.send(Event::SpeechStarted { session_id: session_id.clone(), utterance_id: id.into() });
                        }
                    } else if kind == "conversation.item.input_audio_transcription.text" {
                        if let Some(id) = item_ids.get(item_id) {
                            let text = format!("{}{}", value["text"].as_str().unwrap_or(""), value["stash"].as_str().unwrap_or(""));
                            let _ = events.send(Event::Partial { session_id: session_id.clone(), utterance_id: id.clone(), text });
                        }
                    } else if kind == "conversation.item.input_audio_transcription.completed" {
                        if let Some(id) = item_ids.remove(item_id) {
                            let _ = events.send(Event::Final { session_id: session_id.clone(), utterance_id: id, text: value["transcript"].as_str().unwrap_or("").into(), language: value["language"].as_str().map(str::to_string) });
                        }
                    } else if kind == "conversation.item.input_audio_transcription.failed" || kind == "error" {
                        let _ = events.send(Event::Failed { session_id: session_id.clone(), utterance_id: item_ids.remove(item_id), message: value["error"]["message"].as_str().unwrap_or("Qwen ASRで認識に失敗しました。").into() });
                    } else if kind == "session.finished" { break Ok(()); }
                }
                Some(Ok(Message::Text(_))) => break Err("Qwen ASRイベントが大きすぎます。".into()),
                Some(Ok(Message::Close(_))) | None => break Err("Qwen ASR接続が終了しました。".into()),
                Some(Err(_)) => break Err("Qwen ASR通信に失敗しました。".into()),
                _ => {},
            }
        }
    };
    let _ = writer.close().await;
    if let Err(message) = &result {
        let _ = events.send(Event::Failed {
            session_id: session_id.clone(),
            utterance_id: None,
            message: message.clone(),
        });
    }
    if let Some(done) = stop_reply {
        let _ = done.send(result.clone());
    }
    let _ = events.send(Event::Stopped {
        session_id: session_id.clone(),
    });
    sessions().lock().await.remove(&session_id);
}

#[cfg(feature = "provider-unit-test-harness")]
pub(crate) async fn exercise_local_websocket_fixture() {
    tests::live_session_maps_qwen_partial_and_final_to_the_sent_utterance().await;
}

#[cfg(feature = "provider-unit-test-harness")]
pub(crate) async fn exercise_rejected_session_fixture() {
    tests::session_start_rejects_a_provider_configuration_error().await;
}

mod dispatch;
pub(crate) use dispatch::with_handler;

#[cfg(any(test, feature = "provider-unit-test-harness"))]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex as StdMutex};

    #[cfg(test)]
    #[test]
    fn provider_speech_start_uses_the_audio_range_of_its_local_utterance() {
        let ranges = vec![
            Range {
                utterance_id: "first".into(),
                start_ms: 0,
                end_ms: 3_500,
            },
            Range {
                utterance_id: "second".into(),
                start_ms: 3_500,
                end_ms: 5_000,
            },
        ];
        assert_eq!(utterance_for_start(&ranges, 200), Some("first"));
        assert_eq!(utterance_for_start(&ranges, 3_500), Some("second"));
        assert_eq!(utterance_for_start(&ranges, 5_000), None);
    }

    #[cfg_attr(test, tokio::test)]
    pub(crate) async fn live_session_maps_qwen_partial_and_final_to_the_sent_utterance() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let provider = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            let setup = socket.next().await.unwrap().unwrap().into_text().unwrap();
            assert_eq!(
                serde_json::from_str::<Value>(&setup).unwrap()["type"],
                "session.update"
            );
            socket
                .send(Message::Text(
                    json!({"type":"session.updated"}).to_string().into(),
                ))
                .await
                .unwrap();
            assert_eq!(
                serde_json::from_str::<Value>(&setup).unwrap()["session"]["turn_detection"]
                    ["silence_duration_ms"],
                800
            );
            let audio = socket.next().await.unwrap().unwrap().into_text().unwrap();
            assert_eq!(
                serde_json::from_str::<Value>(&audio).unwrap()["type"],
                "input_audio_buffer.append"
            );
            for event in [
                json!({"type":"input_audio_buffer.speech_started","audio_start_ms":0,"item_id":"item-1"}),
                json!({"type":"conversation.item.input_audio_transcription.text","item_id":"item-1","text":"こん","stash":"にちは"}),
                json!({"type":"conversation.item.input_audio_transcription.completed","item_id":"item-1","transcript":"こんにちは","language":"ja"}),
            ] {
                socket
                    .send(Message::Text(event.to_string().into()))
                    .await
                    .unwrap();
            }
            let finish = socket.next().await.unwrap().unwrap().into_text().unwrap();
            assert_eq!(
                serde_json::from_str::<Value>(&finish).unwrap()["type"],
                "session.finish"
            );
            socket
                .send(Message::Text(
                    json!({"type":"session.finished"}).to_string().into(),
                ))
                .await
                .unwrap();
        });
        let (mut socket, _) = connect_async(format!("ws://{address}")).await.unwrap();
        initialize_session(&mut socket, 800).await.unwrap();
        let observed = Arc::new(StdMutex::new(Vec::<Value>::new()));
        let sink = observed.clone();
        let events = Channel::new(move |body| {
            if let tauri::ipc::InvokeResponseBody::Json(json) = body {
                sink.lock()
                    .unwrap()
                    .push(serde_json::from_str(&json).unwrap());
            }
            Ok(())
        });
        let (sender, receiver) = mpsc::channel(4);
        let db = rusqlite::Connection::open_in_memory().unwrap();
        crate::initialize_database(&db).unwrap();
        let state = crate::test_support::app_state(db);
        let pinned = state
            .sqlite_readers
            .read(|db| {
                let snapshot = persistence::service_registry_store::load_registry(db)?.snapshot;
                crate::providers::service_registry::resolve_route(
                    &snapshot,
                    crate::providers::service_registry::Purpose::VoiceTranscribe,
                )
                .map_err(|e| format!("{e:?}"))
            })
            .unwrap();
        let session = tokio::spawn(run_session(
            "test-session".into(),
            socket,
            receiver,
            events,
            super::super::streaming_asr::speaker_gate_runtime::SpeakerGate::new(None, 0.008),
            state.sqlite_readers.clone(),
            pinned,
        ));
        sender
            .send(Command::Audio {
                utterance_id: "local-1".into(),
                audio: vec![0; PACKET_BYTES],
            })
            .await
            .unwrap();
        let (done, wait) = oneshot::channel();
        sender.send(Command::Stop { done }).await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(3), wait)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        session.await.unwrap();
        provider.await.unwrap();
        let events = observed.lock().unwrap();
        assert!(events
            .iter()
            .any(|event| event["type"] == "speechStarted" && event["utteranceId"] == "local-1"));
        assert!(events.iter().any(|event| event["type"] == "partial"
            && event["utteranceId"] == "local-1"
            && event["text"] == "こんにちは"));
        assert!(events.iter().any(|event| event["type"] == "final"
            && event["utteranceId"] == "local-1"
            && event["text"] == "こんにちは"));
    }

    #[cfg_attr(test, tokio::test)]
    pub(crate) async fn session_start_rejects_a_provider_configuration_error() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let provider = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            let setup = socket.next().await.unwrap().unwrap().into_text().unwrap();
            assert_eq!(
                serde_json::from_str::<Value>(&setup).unwrap()["type"],
                "session.update"
            );
            socket
                .send(Message::Text(
                    json!({"type":"error", "error":{"message":"bad session"}})
                        .to_string()
                        .into(),
                ))
                .await
                .unwrap();
        });
        let (mut socket, _) = connect_async(format!("ws://{address}")).await.unwrap();
        assert!(initialize_session(&mut socket, 1_500).await.is_err());
        provider.await.unwrap();
    }
}
