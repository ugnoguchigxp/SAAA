//! Incremental record/body framing for one-request Qwen responses.
//! A JSON control object may span SSE deltas and lines; body bytes stay separate.
use serde_json::Value;

const MAX_CONTROL_BYTES: usize = 4_096;

#[derive(Debug, PartialEq)]
pub(crate) enum Event {
    Control(Value),
    Body(String),
}

#[derive(Debug, PartialEq)]
pub(crate) enum Error {
    InvalidPrefix,
    InvalidControl,
    MissingSeparator,
    ControlTooLarge,
    Incomplete,
    MissingBody,
}

#[derive(Default)]
enum Phase {
    #[default]
    Control,
    Separator,
    Body,
}

#[derive(Default)]
pub(crate) struct Parser {
    phase: Phase,
    control: String,
    depth: usize,
    in_string: bool,
    escaped: bool,
    separator_has_newline: bool,
    body_seen: bool,
}

impl Parser {
    pub(crate) fn push(&mut self, delta: &str) -> Result<Vec<Event>, Error> {
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
            events.push(Event::Body(body));
        }
        Ok(events)
    }

    fn push_control(&mut self, ch: char, events: &mut Vec<Event>) -> Result<(), Error> {
        if self.control.is_empty() && ch.is_whitespace() {
            return Ok(());
        }
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
                        let value: Value = serde_json::from_str(&self.control)
                            .map_err(|_| Error::InvalidControl)?;
                        if !value.is_object() {
                            return Err(Error::InvalidControl);
                        }
                        events.push(Event::Control(value));
                        self.phase = Phase::Separator;
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    pub(crate) fn finish(&self) -> Result<(), Error> {
        match self.phase {
            Phase::Control => Err(Error::Incomplete),
            Phase::Separator => Err(Error::MissingBody),
            Phase::Body if !self.body_seen => Err(Error::MissingBody),
            Phase::Body => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multiline_control_and_body_survive_every_split() {
        let response = "{\n  \"kind\": \"greeting\"\n}\n\nこんにちは！";
        for split in response.char_indices().map(|(index, _)| index).skip(1) {
            let mut parser = Parser::default();
            let mut events = parser.push(&response[..split]).unwrap();
            events.extend(parser.push(&response[split..]).unwrap());
            assert_eq!(
                events.first(),
                Some(&Event::Control(serde_json::json!({"kind":"greeting"})))
            );
            let body = events
                .into_iter()
                .filter_map(|event| match event {
                    Event::Body(text) => Some(text),
                    Event::Control(_) => None,
                })
                .collect::<String>();
            assert_eq!(body, "こんにちは！");
            assert_eq!(parser.finish(), Ok(()));
        }
    }

    #[test]
    fn quoted_braces_do_not_finish_control() {
        let mut parser = Parser::default();
        let events = parser
            .push("{\"kind\":\"answer\",\"note\":\"} \\\" {\"}\n本文")
            .unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(parser.finish(), Ok(()));
    }

    #[test]
    fn rejects_unframed_or_incomplete_output() {
        let mut parser = Parser::default();
        assert_eq!(parser.push("説明から開始"), Err(Error::InvalidPrefix));
        let mut parser = Parser::default();
        assert_eq!(
            parser.push("{\"kind\":\"greeting\"}本文"),
            Err(Error::MissingSeparator)
        );
        let mut parser = Parser::default();
        assert_eq!(parser.push("{\"kind\":"), Ok(vec![]));
        assert_eq!(parser.finish(), Err(Error::Incomplete));
        let mut parser = Parser::default();
        parser.push("{\"kind\":\"greeting\"}\n").unwrap();
        assert_eq!(parser.finish(), Err(Error::MissingBody));
    }

    #[test]
    fn bounds_unfinished_control() {
        let mut parser = Parser::default();
        assert_eq!(
            parser.push(&format!("{{\"x\":\"{}", "a".repeat(MAX_CONTROL_BYTES))),
            Err(Error::ControlTooLarge)
        );
    }
}
