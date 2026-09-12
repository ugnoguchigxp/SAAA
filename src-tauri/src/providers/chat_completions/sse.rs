//! Byte-oriented SSE framing. UTF-8 is decoded only after a complete line arrives.
use super::super::stream::ProviderFailureKind as Failure;

#[derive(Default)]
pub(super) struct SseDecoder {
    line: Vec<u8>,
    data: Vec<String>,
    pending_cr: bool,
    first_line: bool,
    bytes: usize,
}

impl SseDecoder {
    pub(super) fn push(&mut self, bytes: &[u8]) -> Result<Vec<String>, Failure> {
        let mut events = Vec::new();
        for &byte in bytes {
            if self.pending_cr {
                self.pending_cr = false;
                if byte == b'\n' {
                    continue;
                }
            }
            self.bytes += 1;
            if self.bytes > 524_288 {
                return Err(Failure::RequestTooLarge);
            }
            if byte == b'\r' || byte == b'\n' {
                self.pending_cr = byte == b'\r';
                let raw = std::mem::take(&mut self.line);
                let line = std::str::from_utf8(&raw).map_err(|_| Failure::Protocol)?;
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
                    self.bytes = 0;
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_byte_split_preserves_utf8_comments_and_line_endings() {
        let source =
            "\u{feff}: hello\r\ndata: 日本\r\ndata: 語\r\n\r\ndata: next\n\ndata: last\r\r";
        for split in 0..=source.len() {
            let mut decoder = SseDecoder::default();
            let mut events = decoder.push(&source.as_bytes()[..split]).unwrap();
            events.extend(decoder.push(&source.as_bytes()[split..]).unwrap());
            assert_eq!(events, ["日本\n語", "next", "last"]);
        }
    }
}
