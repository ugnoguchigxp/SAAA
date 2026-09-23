//! Incremental WAV/PCM decoder. Raw PCM has the explicit OpenAI 24 kHz mono s16le contract.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Format {
    pub(crate) rate: u32,
    pub(crate) channels: u16,
}

pub(crate) struct Decoder {
    pending: Vec<u8>,
    pub(crate) format: Option<Format>,
    header_done: bool,
    remaining: Option<usize>,
    decoded: usize,
    header_cursor: usize,
}
impl Decoder {
    pub(crate) fn new(format: &str) -> Result<Self, String> {
        if !matches!(format, "wav" | "pcm") {
            return Err("Unsupported TTS audio format".into());
        }
        Ok(Self {
            pending: Vec::new(),
            format: (format == "pcm").then_some(Format {
                rate: 24000,
                channels: 1,
            }),
            header_done: format == "pcm",
            remaining: None,
            decoded: 0,
            header_cursor: 12,
        })
    }
    pub(crate) fn push(&mut self, bytes: &[u8]) -> Result<Vec<i16>, String> {
        self.pending.extend_from_slice(bytes);
        if !self.header_done {
            if self.pending.len() < 12 {
                return Ok(Vec::new());
            }
            if &self.pending[..4] != b"RIFF" || &self.pending[8..12] != b"WAVE" {
                return Err("TTS response is not WAV".into());
            }
            let mut cursor = self.header_cursor;
            loop {
                if cursor + 8 > self.pending.len() {
                    break;
                }
                let kind = &self.pending[cursor..cursor + 4];
                let size_bytes: [u8; 4] = self.pending[cursor + 4..cursor + 8]
                    .try_into()
                    .map_err(|_| "WAV chunk size is truncated".to_string())?;
                let size = u32::from_le_bytes(size_bytes) as usize;
                if kind == b"data" {
                    if self.format.is_none() {
                        return Err("WAV data arrived before its format".into());
                    }
                    self.remaining = if size == u32::MAX as usize || size == 0 {
                        None
                    } else {
                        Some(size)
                    };
                    self.pending.drain(..cursor + 8);
                    self.header_done = true;
                    break;
                }
                if size > 65536 || cursor + 8 + size > 65536 {
                    return Err("WAV header exceeded the size limit".into());
                }
                if cursor + 8 + size > self.pending.len() {
                    return Ok(Vec::new());
                }
                if kind == b"fmt " {
                    if self.format.is_some() || size < 16 {
                        return Err("Invalid WAV format chunk".into());
                    }
                    let data = &self.pending[cursor + 8..cursor + 8 + size];
                    let u16at = |n: usize| -> Result<u16, String> {
                        let bytes: [u8; 2] = data
                            .get(n..n + 2)
                            .ok_or_else(|| "WAV format chunk is truncated".to_string())?
                            .try_into()
                            .map_err(|_| "WAV format field is truncated".to_string())?;
                        Ok(u16::from_le_bytes(bytes))
                    };
                    let rate_bytes: [u8; 4] = data
                        .get(4..8)
                        .ok_or_else(|| "WAV sample rate is truncated".to_string())?
                        .try_into()
                        .map_err(|_| "WAV sample rate is truncated".to_string())?;
                    let rate = u32::from_le_bytes(rate_bytes);
                    let byte_rate_bytes: [u8; 4] = data
                        .get(8..12)
                        .ok_or_else(|| "WAV byte rate is truncated".to_string())?
                        .try_into()
                        .map_err(|_| "WAV byte rate is truncated".to_string())?;
                    let byte_rate = u32::from_le_bytes(byte_rate_bytes);
                    let channels = u16at(2)?;
                    if u16at(0)? != 1
                        || u16at(14)? != 16
                        || !(1..=2).contains(&channels)
                        || !(8000..=192000).contains(&rate)
                        || u16at(12)? != channels * 2
                        || byte_rate != rate * u32::from(channels) * 2
                    {
                        return Err("WAV must be PCM s16le, 1–2 channels, 8–192 kHz".into());
                    }
                    self.format = Some(Format { rate, channels });
                }
                cursor += 8 + size + (size % 2);
                self.header_cursor = cursor;
            }
            if !self.header_done {
                return Ok(Vec::new());
            }
        }
        let format = self
            .format
            .ok_or_else(|| "WAV format is missing before sample decode".to_string())?;
        let align = format.channels as usize * 2;
        let available = self
            .remaining
            .unwrap_or(self.pending.len())
            .min(self.pending.len());
        let count = available / align * align;
        let samples = self.pending[..count]
            .chunks_exact(2)
            .map(|v| i16::from_le_bytes([v[0], v[1]]))
            .collect();
        self.pending.drain(..count);
        self.decoded += count;
        if let Some(remaining) = &mut self.remaining {
            *remaining -= count;
            if *remaining == 0 {
                self.pending.clear();
            }
        }
        Ok(samples)
    }
    pub(crate) fn finish(&self) -> Result<(), String> {
        if !self.header_done
            || self.decoded == 0
            || self.remaining.is_some_and(|n| n != 0)
            || !self.pending.is_empty()
        {
            return Err("TTS audio was empty, truncated, or not sample-aligned".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fragmented_wave_starts_before_eof_and_checks_length() {
        let wave = crate::voice::network_asr::encode_wav(&[0.25; 160], 16000).unwrap();
        let mut decoder = Decoder::new("wav").unwrap();
        let mut samples = Vec::new();
        for byte in &wave[..wave.len() - 2] {
            samples.extend(decoder.push(&[*byte]).unwrap());
        }
        assert_eq!(samples.len(), 159);
        assert!(decoder.finish().is_err());
        samples.extend(decoder.push(&wave[wave.len() - 2..]).unwrap());
        assert_eq!(samples.len(), 160);
        assert!(decoder.finish().is_ok());
    }
    #[test]
    fn raw_pcm_has_an_explicit_format_and_rejects_odd_eof() {
        let mut decoder = Decoder::new("pcm").unwrap();
        assert_eq!(
            decoder.format,
            Some(Format {
                rate: 24000,
                channels: 1
            })
        );
        assert!(decoder.push(&[1]).unwrap().is_empty());
        assert!(decoder.finish().is_err());
        assert_eq!(decoder.push(&[0]).unwrap(), [1]);
        assert!(decoder.finish().is_ok());
        assert!(Decoder::new("mp3").is_err());
    }
}
