//! Extract the answer string from a streamed JSON action without speaking tool requests.
use std::sync::{Arc, Mutex};
use crate::ipc_contract::RuntimeEvent;
use crate::runtime::event_hub::RuntimeEventSender;
use tauri::Emitter;

pub(super) struct AnswerDeltaSender<R: tauri::Runtime> {
    state: Arc<Mutex<Extractor>>,
    output: tokio::sync::mpsc::UnboundedSender<String>,
    app: tauri::AppHandle<R>,
    input_id: String,
}

impl<R: tauri::Runtime> Clone for AnswerDeltaSender<R> {
    fn clone(&self) -> Self {
        Self { state: self.state.clone(), output: self.output.clone(), app: self.app.clone(), input_id: self.input_id.clone() }
    }
}

impl<R: tauri::Runtime> AnswerDeltaSender<R> {
    pub(super) fn new(output: tokio::sync::mpsc::UnboundedSender<String>, app: tauri::AppHandle<R>, input_id: String) -> Self {
        Self { state: Arc::new(Mutex::new(Extractor::default())), output, app, input_id }
    }

    pub(super) fn complete_content(&self) -> Option<String> {
        self.state.lock().ok().and_then(|state| state.closed.then(|| state.sent.clone()))
    }
}

impl<R: tauri::Runtime> RuntimeEventSender for AnswerDeltaSender<R> {
    fn send(&self, event: RuntimeEvent) -> tauri::Result<()> {
        if let RuntimeEvent::Delta { text, .. } = event {
            if let Ok(mut state) = self.state.lock() {
                if let Some(delta) = state.append(&text) {
                    let _ = self.app.emit("conversation-answer-delta", serde_json::json!({"inputId":self.input_id,"text":delta}));
                    let _ = self.output.send(delta);
                }
            }
        }
        Ok(())
    }

    fn clone_box(&self) -> Box<dyn RuntimeEventSender> {
        Box::new(self.clone())
    }
}

#[derive(Default)]
struct Extractor {
    raw: String,
    content_start: Option<usize>,
    sent: String,
    stopped: bool,
    closed: bool,
}

impl Extractor {
    fn append(&mut self, delta: &str) -> Option<String> {
        if self.stopped { return None; }
        self.raw.push_str(delta);
        if self.raw.len() > 65_536 { self.stopped = true; return None; }
        if self.content_start.is_none() {
            match answer_content_start(&self.raw) {
                Prefix::Ready(start) => self.content_start = Some(start),
                Prefix::Incomplete => return None,
                Prefix::Other => { self.stopped = true; return None; }
            }
        }
        let tail = &self.raw[self.content_start?..];
        let mut escaped = false;
        let mut unicode_left = 0;
        let mut safe_end = 0;
        for (index, ch) in tail.char_indices() {
            if unicode_left > 0 {
                if !ch.is_ascii_hexdigit() { self.stopped = true; return None; }
                unicode_left -= 1;
            } else if escaped {
                if ch == 'u' { unicode_left = 4; }
                else if !matches!(ch, '"' | '\\' | '/' | 'b' | 'f' | 'n' | 'r' | 't') {
                    self.stopped = true; return None;
                }
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                self.closed = true;
                self.stopped = true;
                break;
            } else if ch.is_control() {
                self.stopped = true;
                return None;
            }
            if !escaped && unicode_left == 0 { safe_end = index + ch.len_utf8(); }
        }
        if safe_end == 0 { return None; }
        let decoded: String = serde_json::from_str(&format!("\"{}\"", &tail[..safe_end])).ok()?;
        let suffix = decoded.strip_prefix(&self.sent)?.to_string();
        self.sent = decoded;
        (!suffix.is_empty()).then_some(suffix)
    }
}

enum Prefix { Ready(usize), Incomplete, Other }

fn answer_content_start(raw: &str) -> Prefix {
    let mut cursor = 0;
    for token in ["{", "\"action\"", ":", "\"answer\"", ",", "\"content\"", ":", "\""] {
        while raw.as_bytes().get(cursor).is_some_and(u8::is_ascii_whitespace) { cursor += 1; }
        let remaining = &raw[cursor..];
        if remaining.len() < token.len() && token.starts_with(remaining) { return Prefix::Incomplete; }
        if !remaining.starts_with(token) { return Prefix::Other; }
        cursor += token.len();
    }
    Prefix::Ready(cursor)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn streams_only_answer_content_with_split_escapes() {
        let mut parser = Extractor::default();
        assert_eq!(parser.append("{\"action\":\"answer\",\"con"), None);
        assert_eq!(parser.append("tent\":\"こんにちは。\n"), None);
        // A raw newline is invalid JSON and must never be spoken.
        assert_eq!(parser.append("次"), None);
        let mut parser = Extractor::default();
        assert_eq!(parser.append("{\"action\":\"answer\",\"content\":\"こんにちは。\\"), Some("こんにちは。".into()));
        assert_eq!(parser.append("n続きです。\",\"sources\":[]}"), Some("\n続きです。".into()));
        let mut tool = Extractor::default();
        assert_eq!(tool.append("{\"action\":\"web_search\",\"query\":\"秘密\"}"), None);
    }
}
