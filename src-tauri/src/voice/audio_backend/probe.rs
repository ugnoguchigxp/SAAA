use super::global;
use std::{
    env, fs,
    path::PathBuf,
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

pub fn run() -> i32 {
    let mut seconds = 3u64;
    let mut wav: Option<PathBuf> = None;
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--seconds" => {
                seconds = args
                    .next()
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(seconds);
            }
            "--wav" => wav = args.next().map(PathBuf::from),
            "--help" => {
                eprintln!("vpio_probe [--seconds N] [--wav out.wav]");
                return 0;
            }
            other => {
                eprintln!("unknown argument: {other}");
                return 2;
            }
        }
    }
    let backend = global();
    let status = backend.status();
    println!(
        "available={} macos={} ducking={} transport={:?} reason={:?}",
        status.available,
        status.macos_major,
        status.ducking_level,
        status.output_transport,
        status.reason
    );
    if !status.available {
        return 1;
    }
    let recorded = Arc::new(Mutex::new(Vec::<f32>::new()));
    let frames = recorded.clone();
    match backend.start_capture(
        super::VoiceProcessingConfig::default(),
        Arc::new(move |frame| {
            if let Ok(mut samples) = frames.lock() {
                samples.extend_from_slice(&frame);
            }
        }),
    ) {
        Ok(started) => println!(
            "started capture aec={} agc={} ducking={}",
            started.aec_active, started.agc_enabled, started.ducking_level
        ),
        Err(error) => {
            eprintln!("{error}");
            return 1;
        }
    }
    thread::sleep(Duration::from_secs(seconds));
    backend.stop_capture();
    if let Some(path) = wav {
        let samples = recorded.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if let Err(error) = write_wav(&path, &samples) {
            eprintln!("{error}");
            return 1;
        }
        println!("wrote {} ({} samples)", path.display(), samples.len());
    }
    0
}

fn write_wav(path: &PathBuf, samples: &[f32]) -> Result<(), String> {
    let data: Vec<u8> = samples
        .iter()
        .flat_map(|sample| {
            let clipped = sample.clamp(-1.0, 1.0);
            ((clipped * 32767.0) as i16).to_le_bytes()
        })
        .collect();
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36u32 + data.len() as u32).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&16_000u32.to_le_bytes());
    bytes.extend_from_slice(&32_000u32.to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&(data.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&data);
    fs::write(path, bytes).map_err(|_| "Could not write probe WAV".to_string())
}
