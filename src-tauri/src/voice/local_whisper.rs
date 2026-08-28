use crate::{bounded_text, whisper_transcript_line, ProcessGuard, RunCancellation};
use std::{
    env,
    fs::File,
    io::{BufRead, BufReader, Read},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::Duration,
};

const MAX_WHISPER_STDOUT_BYTES: u64 = 256 * 1_024;
const MAX_PARTIAL_EVENTS: usize = 128;
const MAX_TRANSCRIPT_FILE_BYTES: u64 = 64 * 1_024;
const MAX_TRANSCRIPT_CHARS: usize = 16_000;

enum WhisperReaderMessage {
    Line(String),
    Failed(&'static str),
}

pub fn transcribe<F>(
    samples: &[f32],
    sample_rate: u32,
    model: &Path,
    cancellation: &RunCancellation,
    mut on_partial: F,
) -> Result<String, String>
where
    F: FnMut(String),
{
    let directory = temporary_workspace()?;
    let wav_path = directory.path().join("input.wav");
    let output_base = directory.path().join("transcript");
    write_wav(&wav_path, samples, sample_rate)?;
    let child = Command::new(executable()?)
        .arg("-m")
        .arg(model)
        .arg("-f")
        .arg(&wav_path)
        .arg("-otxt")
        .arg("-of")
        .arg(&output_base)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("Could not start local whisper: {error}"))?;
    let mut process = ProcessGuard::new(child);
    let stdout = process
        .child_mut()
        .stdout
        .take()
        .ok_or_else(|| "Whisper stdout is unavailable".to_string())?;
    let (sender, receiver) = mpsc::sync_channel(64);
    let reader = thread::spawn(move || {
        let mut reader = BufReader::new(stdout.take(MAX_WHISPER_STDOUT_BYTES + 1));
        let mut bytes_read = 0_u64;
        loop {
            let mut line = String::new();
            let count = match reader.read_line(&mut line) {
                Ok(0) => break,
                Ok(count) => count,
                Err(_) => {
                    let _ = sender.send(WhisperReaderMessage::Failed(
                        "Could not read local whisper output",
                    ));
                    break;
                }
            };
            bytes_read = bytes_read.saturating_add(count as u64);
            if bytes_read > MAX_WHISPER_STDOUT_BYTES {
                let _ = sender.send(WhisperReaderMessage::Failed(
                    "Local whisper output exceeded the size limit",
                ));
                break;
            }
            if sender.send(WhisperReaderMessage::Line(line)).is_err() {
                break;
            }
        }
    });
    let result = (|| {
        let mut partial_fallback = String::new();
        let mut partial_count = 0;
        loop {
            if cancellation.is_cancelled() {
                return Err("Transcription cancelled".to_string());
            }
            while let Ok(message) = receiver.try_recv() {
                if cancellation.is_cancelled() {
                    return Err("Transcription cancelled".to_string());
                }
                match message {
                    WhisperReaderMessage::Line(line) => collect_partial(
                        &line,
                        &mut partial_fallback,
                        &mut partial_count,
                        &mut on_partial,
                    ),
                    WhisperReaderMessage::Failed(message) => return Err(message.to_string()),
                }
            }
            if let Some(status) = process
                .child_mut()
                .try_wait()
                .map_err(|error| format!("Could not inspect whisper process: {error}"))?
            {
                if !status.success() {
                    return Err(format!("Local whisper exited with {status}"));
                }
                break;
            }
            thread::sleep(Duration::from_millis(50));
        }
        while let Ok(message) = receiver.recv() {
            if cancellation.is_cancelled() {
                return Err("Transcription cancelled".to_string());
            }
            match message {
                WhisperReaderMessage::Line(line) => collect_partial(
                    &line,
                    &mut partial_fallback,
                    &mut partial_count,
                    &mut on_partial,
                ),
                WhisperReaderMessage::Failed(message) => return Err(message.to_string()),
            }
        }
        let transcript =
            read_transcript_file(&output_base.with_extension("txt"))?.unwrap_or(partial_fallback);
        if transcript.trim().is_empty() {
            return Err("Local whisper completed without a transcript".to_string());
        }
        Ok(bounded_text(transcript.trim(), MAX_TRANSCRIPT_CHARS))
    })();

    drop(receiver);
    process.terminate();
    if reader.join().is_err() && result.is_ok() {
        return Err("Local whisper output reader stopped unexpectedly".to_string());
    }
    result
}

fn collect_partial<F>(
    line: &str,
    fallback: &mut String,
    partial_count: &mut usize,
    on_partial: &mut F,
) where
    F: FnMut(String),
{
    if *partial_count >= MAX_PARTIAL_EVENTS {
        return;
    }
    let Some(text) = whisper_transcript_line(line) else {
        return;
    };
    *partial_count += 1;
    if !fallback.is_empty() && fallback.chars().count() < MAX_TRANSCRIPT_CHARS {
        fallback.push(' ');
    }
    let remaining = MAX_TRANSCRIPT_CHARS.saturating_sub(fallback.chars().count());
    fallback.push_str(&bounded_text(&text, remaining));
    on_partial(text);
}

fn read_transcript_file(path: &Path) -> Result<Option<String>, String> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("Could not read local transcript: {error}")),
    };
    let mut bytes = Vec::with_capacity(MAX_TRANSCRIPT_FILE_BYTES as usize + 1);
    file.take(MAX_TRANSCRIPT_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("Could not read local transcript: {error}"))?;
    if bytes.len() > MAX_TRANSCRIPT_FILE_BYTES as usize {
        return Err("Local whisper transcript exceeded the size limit".to_string());
    }
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|_| "Local whisper transcript is not valid UTF-8".to_string())
}

fn temporary_workspace() -> Result<tempfile::TempDir, String> {
    tempfile::tempdir().map_err(|error| format!("Could not create audio workspace: {error}"))
}

pub fn write_wav(path: &Path, samples: &[f32], sample_rate: u32) -> Result<(), String> {
    if samples.is_empty()
        || !(8_000..=192_000).contains(&sample_rate)
        || samples.iter().any(|sample| !sample.is_finite())
    {
        return Err("Invalid audio samples".to_string());
    }
    let resampled = resample_pcm(samples, sample_rate, 16_000);
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 16_000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec)
        .map_err(|error| format!("Could not create local audio file: {error}"))?;
    for sample in resampled {
        writer
            .write_sample((sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)
            .map_err(|error| format!("Could not write local audio: {error}"))?;
    }
    writer
        .finalize()
        .map_err(|error| format!("Could not finalize local audio: {error}"))
}

pub fn resample_pcm(samples: &[f32], source_rate: u32, target_rate: u32) -> Vec<f32> {
    if samples.is_empty() || source_rate == 0 || target_rate == 0 {
        return Vec::new();
    }
    if source_rate == target_rate {
        return samples.to_vec();
    }
    let ratio = source_rate as f64 / target_rate as f64;
    let target_len = ((samples.len() as f64) / ratio).floor() as usize;
    (0..target_len)
        .map(|index| {
            let position = index as f64 * ratio;
            let left = position.floor() as usize;
            let right = (left + 1).min(samples.len().saturating_sub(1));
            let fraction = (position - left as f64) as f32;
            samples[left] * (1.0 - fraction) + samples[right] * fraction
        })
        .collect()
}

pub fn executable() -> Result<PathBuf, String> {
    let mut candidates = Vec::new();
    if let Some(path) = env::var_os("SAAA_WHISPER_PATH").filter(|value| !value.is_empty()) {
        candidates.push(PathBuf::from(path));
    }
    candidates.extend([
        PathBuf::from("whisper-cli"),
        PathBuf::from("whisper.cpp"),
        PathBuf::from("main"),
    ]);
    for candidate in candidates {
        if candidate.is_absolute() && !candidate.is_file() {
            continue;
        }
        let child = Command::new(&candidate)
            .arg("--help")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        let Ok(child) = child else {
            continue;
        };
        let mut process = ProcessGuard::new(child);
        let started = std::time::Instant::now();
        loop {
            match process.child_mut().try_wait() {
                Ok(Some(status)) if status.success() => return Ok(candidate),
                Ok(Some(_)) | Err(_) => break,
                Ok(None) if started.elapsed() < Duration::from_secs(2) => {
                    thread::sleep(Duration::from_millis(20));
                }
                Ok(None) => break,
            }
        }
    }
    Err("Local whisper executable was not found. Set SAAA_WHISPER_PATH to whisper-cli.".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn temporary_audio_workspace_is_removed_after_success_and_failure() {
        let successful = temporary_workspace().expect("workspace");
        let successful_path = successful.path().to_path_buf();
        fs::write(successful_path.join("input.wav"), b"audio").expect("fixture writes");
        drop(successful);
        assert!(!successful_path.exists());

        let failed = temporary_workspace().expect("workspace");
        let failed_path = failed.path().to_path_buf();
        fs::write(failed_path.join("input.wav"), b"audio").expect("fixture writes");
        drop(failed);
        assert!(!failed_path.exists());
    }

    #[test]
    fn partial_output_and_transcript_files_are_bounded() {
        let mut fallback = String::new();
        let mut count = 0;
        let mut emitted = 0;
        for _ in 0..(MAX_PARTIAL_EVENTS + 10) {
            collect_partial(
                "[00:00:00.000 --> 00:00:01.000] sample",
                &mut fallback,
                &mut count,
                &mut |_| emitted += 1,
            );
        }
        assert_eq!(emitted, MAX_PARTIAL_EVENTS);
        assert!(fallback.chars().count() <= MAX_TRANSCRIPT_CHARS);

        let directory = tempfile::tempdir().expect("temporary directory");
        let oversized = directory.path().join("oversized.txt");
        fs::write(
            &oversized,
            vec![b'a'; MAX_TRANSCRIPT_FILE_BYTES as usize + 1],
        )
        .expect("oversized fixture writes");
        assert!(read_transcript_file(&oversized).is_err());
    }
}
