use super::*;

pub(super) fn write_wav(path: &PathBuf, samples: &[f32]) -> Result<(), String> {
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
