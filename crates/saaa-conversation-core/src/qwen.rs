//! Strict framing for a single streamed Qwen answer.
use crate::contracts::Action;
use serde::de::{self, MapAccess, Visitor};
use serde::Deserialize;
use std::fmt;

const MAX_CONTROL_BYTES: usize = 4 * 1024;

#[derive(Debug, PartialEq, Eq)]
pub enum Event {
    Control(Action),
    Body(String),
}

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    InvalidPrefix,
    InvalidControl,
    MissingSeparator,
    ControlTooLarge,
    Incomplete,
    MissingBody,
    DuplicateControl,
    InvalidUtf8,
    SseTooLarge,
    ProviderProtocol,
    ProviderFinish,
}

#[derive(Default)]
enum Phase {
    #[default]
    Control,
    Separator,
    Body,
}

#[derive(Default)]
pub struct ControlParser {
    phase: Phase,
    control: String,
    depth: usize,
    in_string: bool,
    escaped: bool,
    separator_has_newline: bool,
    body_seen: bool,
    body_prefix: String,
}

impl ControlParser {
    pub fn push(&mut self, delta: &str) -> Result<Vec<Event>, Error> {
        let mut events = Vec::new();
        let mut body = String::new();
        for ch in delta.chars() {
            match self.phase {
                Phase::Control => self.push_control(ch, &mut events)?,
                Phase::Separator => {
                    if ch == '\n' {
                        self.separator_has_newline = true;
                    } else if !ch.is_whitespace() {
                        if !self.separator_has_newline {
                            return Err(Error::MissingSeparator);
                        }
                        self.phase = Phase::Body;
                        self.body_seen = true;
                        body.push(ch);
                    }
                }
                Phase::Body => {
                    self.body_seen = true;
                    body.push(ch);
                }
            }
        }
        if !body.is_empty() {
            // A second control record at the very beginning of the body is a
            // protocol violation. Keep only a short prefix across SSE deltas.
            if self.body_prefix.len() < MAX_CONTROL_BYTES {
                for ch in body.chars() {
                    if self.body_prefix.len() + ch.len_utf8() > MAX_CONTROL_BYTES {
                        break;
                    }
                    self.body_prefix.push(ch);
                }
                let after_open = self.body_prefix.trim_start().strip_prefix('{');
                if after_open.is_some_and(|rest| rest.trim_start().starts_with("\"action\"")) {
                    return Err(Error::DuplicateControl);
                }
            }
            events.push(Event::Body(body));
        }
        Ok(events)
    }

    fn push_control(&mut self, ch: char, events: &mut Vec<Event>) -> Result<(), Error> {
        if self.control.is_empty() && ch != '{' {
            return Err(Error::InvalidPrefix);
        }
        self.control.push(ch);
        if self.control.len() > MAX_CONTROL_BYTES {
            return Err(Error::ControlTooLarge);
        }
        if self.in_string {
            if self.escaped {
                self.escaped = false;
            } else if ch == '\\' {
                self.escaped = true;
            } else if ch == '"' {
                self.in_string = false;
            }
        } else {
            match ch {
                '"' => self.in_string = true,
                '{' => self.depth += 1,
                '}' => {
                    self.depth = self.depth.checked_sub(1).ok_or(Error::InvalidControl)?;
                    if self.depth == 0 {
                        events.push(Event::Control(strict_control(&self.control)?));
                        self.phase = Phase::Separator;
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    pub fn finish(&self) -> Result<(), Error> {
        match self.phase {
            Phase::Control => Err(Error::Incomplete),
            Phase::Separator => Err(Error::MissingBody),
            Phase::Body if !self.body_seen => Err(Error::MissingBody),
            Phase::Body => Ok(()),
        }
    }
}

fn strict_control(raw: &str) -> Result<Action, Error> {
    struct ControlVisitor;
    impl<'de> Visitor<'de> for ControlVisitor {
        type Value = Action;

        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("an object containing only action")
        }

        fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Action, M::Error> {
            let mut action = None;
            while let Some(key) = map.next_key::<String>()? {
                if key != "action" || action.is_some() {
                    return Err(de::Error::custom("unknown or duplicate control key"));
                }
                action = Some(map.next_value::<Action>()?);
            }
            action.ok_or_else(|| de::Error::custom("missing action"))
        }
    }
    struct Control(Action);
    impl<'de> Deserialize<'de> for Control {
        fn deserialize<D: de::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            deserializer.deserialize_map(ControlVisitor).map(Control)
        }
    }
    let mut deserializer = serde_json::Deserializer::from_str(raw);
    let control = Control::deserialize(&mut deserializer).map_err(|_| Error::InvalidControl)?;
    deserializer.end().map_err(|_| Error::InvalidControl)?;
    Ok(control.0)
}

/// Byte framing happens before UTF-8 decode, so multibyte characters may cross
/// arbitrary network reads without corrupting a model delta.
#[derive(Default)]
pub struct SseDecoder {
    line: Vec<u8>,
    data: Vec<String>,
    pending_cr: bool,
    first_line: bool,
    event_bytes: usize,
}

impl SseDecoder {
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<String>, Error> {
        let mut events = Vec::new();
        for &byte in bytes {
            if self.pending_cr {
                self.pending_cr = false;
                if byte == b'\n' {
                    continue;
                }
            }
            self.event_bytes += 1;
            if self.event_bytes > 512 * 1024 {
                return Err(Error::SseTooLarge);
            }
            if byte == b'\r' || byte == b'\n' {
                self.pending_cr = byte == b'\r';
                let raw = std::mem::take(&mut self.line);
                let line = std::str::from_utf8(&raw).map_err(|_| Error::InvalidUtf8)?;
                let line = if !self.first_line {
                    line.trim_start_matches('\u{feff}')
                } else {
                    line
                };
                self.first_line = true;
                if line.is_empty() {
                    if !self.data.is_empty() {
                        events.push(self.data.join("\n"));
                    }
                    self.data.clear();
                    self.event_bytes = 0;
                } else if let Some(data) = line.strip_prefix("data:") {
                    self.data
                        .push(data.strip_prefix(' ').unwrap_or(data).to_string());
                } else if line == "data" {
                    self.data.push(String::new());
                }
            } else {
                self.line.push(byte);
            }
        }
        Ok(events)
    }
}

/// Extracts only choice zero's public content. Reasoning and tool fields are
/// deliberately ignored as data, while tool-call finish is a failed request.
#[derive(Default)]
pub struct CompletionDecoder {
    finished: bool,
    done: bool,
    next_seq: u64,
}

impl CompletionDecoder {
    pub fn push_event(&mut self, event: &str) -> Result<Option<(u64, String)>, Error> {
        if self.done {
            return Err(Error::ProviderProtocol);
        }
        if event == "[DONE]" {
            if !self.finished {
                return Err(Error::ProviderFinish);
            }
            self.done = true;
            return Ok(None);
        }
        if self.finished {
            return Err(Error::ProviderProtocol);
        }
        let value: serde_json::Value =
            serde_json::from_str(event).map_err(|_| Error::ProviderProtocol)?;
        let choices = value["choices"].as_array().ok_or(Error::ProviderProtocol)?;
        if choices.len() != 1 || choices[0]["index"] != 0 {
            return Err(Error::ProviderProtocol);
        }
        let choice = &choices[0];
        if !choice["finish_reason"].is_null() {
            if choice["finish_reason"] == "stop" {
                self.finished = true;
            } else {
                return Err(Error::ProviderFinish);
            }
        }
        let delta = choice["delta"].as_object().ok_or(Error::ProviderProtocol)?;
        if delta.contains_key("tool_calls") || delta.contains_key("function_call") {
            return Err(Error::ProviderFinish);
        }
        let Some(content) = delta.get("content") else {
            return Ok(None);
        };
        if content.is_null() {
            return Ok(None);
        }
        let content = content.as_str().ok_or(Error::ProviderProtocol)?;
        if content.is_empty() {
            return Ok(None);
        }
        let seq = self.next_seq;
        self.next_seq += 1;
        Ok(Some((seq, content.to_string())))
    }

    pub fn finish_stream(&self) -> Result<(), Error> {
        if self.finished && self.done {
            Ok(())
        } else {
            Err(Error::ProviderFinish)
        }
    }
}
