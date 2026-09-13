use crate::runtime::contracts::RunFailureCode;
use crate::{CodexReaderMessage, MAX_CODEX_STDOUT_BYTES};
use serde_json::Value;
use std::{
    io::{BufRead, BufReader, Read},
    sync::mpsc,
    thread,
};

// Own the pipe on one reader thread; preserve bounded output and backpressure.
pub(super) fn spawn(
    stdout: std::process::ChildStdout,
) -> (mpsc::Receiver<CodexReaderMessage>, thread::JoinHandle<()>) {
    let (sender, receiver) = mpsc::sync_channel(256);
    let stdout_reader = thread::spawn(move || {
        let mut reader = BufReader::new(stdout.take(MAX_CODEX_STDOUT_BYTES + 1));
        let mut bytes_read = 0_u64;
        loop {
            let mut line = String::new();
            let count = match reader.read_line(&mut line) {
                Ok(0) => break,
                Ok(count) => count,
                Err(_) => {
                    let _ = sender.send(CodexReaderMessage::Failed {
                        code: RunFailureCode::ProtocolError,
                        message: "Could not read Codex app-server output",
                    });
                    break;
                }
            };
            bytes_read = bytes_read.saturating_add(count as u64);
            if bytes_read > MAX_CODEX_STDOUT_BYTES {
                let _ = sender.send(CodexReaderMessage::Failed {
                    code: RunFailureCode::ResponseTooLarge,
                    message: "Codex app-server output exceeded the bounded stream limit",
                });
                break;
            }
            match serde_json::from_str::<Value>(line.trim_end()) {
                Ok(message) => {
                    if sender.send(CodexReaderMessage::Message(message)).is_err() {
                        break;
                    }
                }
                Err(_) => {
                    let _ = sender.send(CodexReaderMessage::Failed {
                        code: RunFailureCode::ProtocolError,
                        message: "Codex app-server returned invalid JSON",
                    });
                    break;
                }
            }
        }
    });
    (receiver, stdout_reader)
}
