//! One-shot composer images. The original PNG is never written. Staging WebP lives only until
//! claim, cancel, or the startup sweep. Provider bytes stay in memory and are redacted before
//! any generation receipt is stored.

use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::time::Duration;

use base64::Engine;
use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::PngEncoder;
use image::imageops::FilterType;
use image::{ExtendedColorType, GenericImageView, ImageEncoder, ImageReader, Limits};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[path = "commands.rs"]
pub(crate) mod commands;

pub(crate) const MAX_INPUT_BYTES: usize = 12_000_000;
pub(crate) const MAX_PIXELS: u64 = 12_000_000;
pub(crate) const MAX_EDGE: u32 = 8_192;
pub(crate) const RESIZE_AT: u32 = 2_048;
pub(crate) const TARGET_LONG_EDGE: u32 = 1_920;
pub(crate) const MIN_LONG_EDGE: u32 = 1_280;
const QUALITIES: [f32; 3] = [75.0, 65.0, 60.0];
const MAX_ENCODE_ATTEMPTS: usize = 4;
pub(crate) const MAX_WEBP_BYTES: usize = 1_500_000;
/// Separate from the 96_000 byte text envelope. Includes Base64 expansion of one image.
pub(crate) const MAX_IMAGE_REQUEST_BYTES: usize = 3_000_000;
const SWEEP_AFTER: Duration = Duration::from_secs(24 * 60 * 60);
#[allow(dead_code)]
pub(crate) const IMAGE_ONLY_PROMPT: &str = "この画像の内容を説明してください";

#[derive(Debug)]
pub(crate) enum ImageError {
    Rejected(&'static str),
    Failed(&'static str),
}

impl std::fmt::Display for ImageError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Rejected(reason) | Self::Failed(reason) => formatter.write_str(reason),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum SendFormat {
    Webp,
    Png,
    Jpeg,
    Unsupported,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct CapabilityRecord {
    endpoint: String,
    model: String,
    format: SendFormat,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PreparedImage {
    pub(crate) id: String,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) byte_length: usize,
    pub(crate) preview: Vec<u8>,
}

pub(crate) fn prepare(data_directory: &Path, png: &[u8]) -> Result<PreparedImage, ImageError> {
    let encoded = encode_png(png)?;
    let id = crate::new_id("img");
    let path = staging_path(data_directory, &id)?;
    write_private(&path, &encoded.webp)?;
    let reread = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(_) => {
            let _ = fs::remove_file(&path);
            return Err(ImageError::Failed("temporary image could not be read"));
        }
    };
    if reread != encoded.webp {
        let _ = fs::remove_file(&path);
        return Err(ImageError::Failed("temporary image did not round-trip"));
    }
    if let Err(error) = decode_webp(&reread) {
        let _ = fs::remove_file(&path);
        return Err(error);
    }
    Ok(PreparedImage {
        id,
        width: encoded.width,
        height: encoded.height,
        byte_length: encoded.webp.len(),
        preview: reread,
    })
}

pub(crate) fn discard(data_directory: &Path, id: &str) -> Result<(), ImageError> {
    let path = staging_path(data_directory, id)?;
    if path.exists() {
        fs::remove_file(&path)
            .map_err(|_| ImageError::Failed("temporary image could not be removed"))?;
    }
    Ok(())
}

pub(crate) fn claim(data_directory: &Path, id: &str, run_id: &str) -> Result<(), ImageError> {
    validate_token(run_id)?;
    let from = staging_path(data_directory, id)?;
    if !from.is_file() {
        return Err(ImageError::Rejected("image is not ready"));
    }
    let to = claimed_path(data_directory, run_id)?;
    if to.exists() {
        let _ = fs::remove_file(&to);
    }
    fs::rename(&from, &to).map_err(|_| ImageError::Failed("image could not be claimed"))?;
    Ok(())
}

pub(crate) fn release(data_directory: &Path, run_id: &str) -> Result<(), ImageError> {
    let path = claimed_path(data_directory, run_id)?;
    if path.exists() {
        fs::remove_file(&path)
            .map_err(|_| ImageError::Failed("claimed image could not be removed"))?;
    }
    Ok(())
}

pub(crate) fn has_claimed(data_directory: &Path, run_id: &str) -> bool {
    claimed_path(data_directory, run_id).is_ok_and(|path| path.is_file())
}

pub(crate) fn sweep(data_directory: &Path) {
    let root = data_directory.join("image-input");
    for folder in ["staging", "claimed"] {
        let dir = root.join(folder);
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            let Ok(modified) = metadata.modified() else {
                continue;
            };
            if modified.elapsed().unwrap_or(Duration::ZERO) > SWEEP_AFTER {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
}

pub(crate) fn store_capability(
    data_directory: &Path,
    endpoint: &str,
    model: &str,
    format: SendFormat,
) -> Result<(), ImageError> {
    let path = capability_path(data_directory)?;
    let record = CapabilityRecord {
        endpoint: endpoint.to_string(),
        model: model.to_string(),
        format,
    };
    let bytes = serde_json::to_vec(&record)
        .map_err(|_| ImageError::Failed("capability could not be stored"))?;
    write_private(&path, &bytes)
}

pub(crate) fn attach(
    data_directory: &Path,
    messages: &mut Vec<Value>,
    run_id: &str,
    user_text: &str,
    endpoint: &str,
    model: &str,
) -> Result<(), ImageError> {
    let path = claimed_path(data_directory, run_id)?;
    if !path.is_file() {
        return Ok(());
    }
    let format = load_format(data_directory, endpoint, model)?;
    let webp =
        fs::read(&path).map_err(|_| ImageError::Failed("claimed image could not be read"))?;
    let (mime, bytes) = transcode(&webp, format)?;
    let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);
    let part = json!({
        "type": "image_url",
        "image_url": {"url": format!("data:{mime};base64,{encoded}")}
    });
    let text = json!({"type": "text", "text": user_text});
    let Some(message) = messages.iter_mut().rev().find(|message| {
        message["role"] == "user"
            && message["content"]
                .as_str()
                .is_some_and(|text| text.trim() == user_text.trim())
    }) else {
        return Err(ImageError::Rejected(
            "image could not be bound to the question",
        ));
    };
    message["content"] = json!([part, text]);
    drop(bytes);
    drop(encoded);
    Ok(())
}

pub(crate) fn body_has_image(body: &Value) -> bool {
    body["messages"]
        .as_array()
        .is_some_and(|messages| messages.iter().any(message_has_image))
}

pub(crate) fn redact_provider_body(payload: &[u8]) -> Vec<u8> {
    let Ok(mut value) = serde_json::from_slice::<Value>(payload) else {
        return br#"{"messages":[{"role":"user","content":"[image omitted]"}]}"#.to_vec();
    };
    if let Some(messages) = value["messages"].as_array_mut() {
        for message in messages {
            if message_has_image(message) {
                if let Some(parts) = message["content"].as_array_mut() {
                    for part in parts {
                        if part["type"] == "image_url" {
                            part["image_url"]["url"] = json!("[image omitted]");
                        }
                    }
                }
            }
        }
    }
    serde_json::to_vec(&value).unwrap_or_else(|_| {
        br#"{"messages":[{"role":"user","content":"[image omitted]"}]}"#.to_vec()
    })
}

fn message_has_image(message: &Value) -> bool {
    message["content"]
        .as_array()
        .is_some_and(|parts| parts.iter().any(|part| part["type"] == "image_url"))
}

struct Encoded {
    webp: Vec<u8>,
    width: u32,
    height: u32,
}

fn encode_png(png: &[u8]) -> Result<Encoded, ImageError> {
    if png.len() > MAX_INPUT_BYTES {
        return Err(ImageError::Rejected("image input is too large"));
    }
    if png.len() < 8 || &png[..8] != b"\x89PNG\r\n\x1a\n" {
        return Err(ImageError::Rejected("image is not a png"));
    }
    let mut header_limits = Limits::default();
    header_limits.max_image_width = Some(MAX_EDGE);
    header_limits.max_image_height = Some(MAX_EDGE);
    header_limits.max_alloc = Some(64 * 1024 * 1024);
    let mut reader = ImageReader::new(Cursor::new(png));
    reader.set_format(image::ImageFormat::Png);
    reader.limits(header_limits);
    let (width, height) = reader
        .into_dimensions()
        .map_err(|_| ImageError::Rejected("image could not be decoded"))?;
    if u64::from(width) * u64::from(height) > MAX_PIXELS {
        return Err(ImageError::Rejected("image has too many pixels"));
    }
    let mut decode_limits = Limits::default();
    decode_limits.max_image_width = Some(MAX_EDGE);
    decode_limits.max_image_height = Some(MAX_EDGE);
    decode_limits.max_alloc = Some(64 * 1024 * 1024);
    let mut reader = ImageReader::new(Cursor::new(png));
    reader.set_format(image::ImageFormat::Png);
    reader.limits(decode_limits);
    let image = reader
        .decode()
        .map_err(|_| ImageError::Rejected("image could not be decoded"))?;
    let mut rgba = image.to_rgba8();
    let (mut width, mut height) = rgba.dimensions();
    if width.max(height) >= RESIZE_AT {
        (width, height) = scaled(width, height, TARGET_LONG_EDGE);
        rgba = image::imageops::resize(&rgba, width, height, FilterType::Triangle);
    }
    let mut attempts = 0;
    let mut quality_index = 0;
    let mut shrunk = false;
    loop {
        if attempts >= MAX_ENCODE_ATTEMPTS {
            return Err(ImageError::Rejected("image is too large to send"));
        }
        attempts += 1;
        let quality = QUALITIES[quality_index.min(QUALITIES.len() - 1)];
        let webp = encode_webp(rgba.as_raw(), width, height, quality)?;
        decode_webp(&webp)?;
        if webp.len() <= MAX_WEBP_BYTES {
            return Ok(Encoded {
                webp,
                width,
                height,
            });
        }
        if quality_index + 1 < QUALITIES.len() {
            quality_index += 1;
            continue;
        }
        if !shrunk && width.max(height) > MIN_LONG_EDGE {
            shrunk = true;
            (width, height) = scaled(width, height, MIN_LONG_EDGE);
            rgba = image::imageops::resize(&rgba, width, height, FilterType::Triangle);
            continue;
        }
        return Err(ImageError::Rejected("image is too large to send"));
    }
}

fn encode_webp(rgba: &[u8], width: u32, height: u32, quality: f32) -> Result<Vec<u8>, ImageError> {
    let encoder = webp::Encoder::from_rgba(rgba, width, height);
    Ok(encoder.encode(quality).to_vec())
}

fn decode_webp(bytes: &[u8]) -> Result<(u32, u32), ImageError> {
    let image = image::load_from_memory_with_format(bytes, image::ImageFormat::WebP)
        .map_err(|_| ImageError::Rejected("processed image could not be decoded"))?;
    Ok(image.dimensions())
}

fn scaled(width: u32, height: u32, long_edge: u32) -> (u32, u32) {
    let long = width.max(height).max(1);
    let width = ((u64::from(width) * u64::from(long_edge)) / u64::from(long)).max(1) as u32;
    let height = ((u64::from(height) * u64::from(long_edge)) / u64::from(long)).max(1) as u32;
    (width, height)
}

fn transcode(webp: &[u8], format: SendFormat) -> Result<(&'static str, Vec<u8>), ImageError> {
    match format {
        SendFormat::Unsupported => {
            Err(ImageError::Rejected("this provider does not accept images"))
        }
        SendFormat::Webp => Ok(("image/webp", webp.to_vec())),
        SendFormat::Png | SendFormat::Jpeg => {
            let image = image::load_from_memory_with_format(webp, image::ImageFormat::WebP)
                .map_err(|_| ImageError::Failed("processed image could not be decoded"))?;
            let mut output = Cursor::new(Vec::new());
            if format == SendFormat::Png {
                PngEncoder::new(&mut output)
                    .write_image(
                        image.to_rgba8().as_raw(),
                        image.width(),
                        image.height(),
                        ExtendedColorType::Rgba8,
                    )
                    .map_err(|_| ImageError::Failed("image could not be converted"))?;
                Ok(("image/png", output.into_inner()))
            } else {
                // JPEG has no alpha channel. Composite transparent screenshot pixels
                // over white instead of silently dropping alpha to dark RGB values.
                let mut rgb = image.to_rgba8();
                for pixel in rgb.pixels_mut() {
                    let alpha = u16::from(pixel[3]);
                    for channel in &mut pixel.0[..3] {
                        *channel =
                            ((u16::from(*channel) * alpha + 255 * (255 - alpha) + 127) / 255) as u8;
                    }
                }
                let rgb = image::DynamicImage::ImageRgba8(rgb).to_rgb8();
                JpegEncoder::new_with_quality(&mut output, 85)
                    .write_image(
                        rgb.as_raw(),
                        image.width(),
                        image.height(),
                        ExtendedColorType::Rgb8,
                    )
                    .map_err(|_| ImageError::Failed("image could not be converted"))?;
                Ok(("image/jpeg", output.into_inner()))
            }
        }
    }
}

fn load_format(
    data_directory: &Path,
    endpoint: &str,
    model: &str,
) -> Result<SendFormat, ImageError> {
    let path = capability_path(data_directory)?;
    // No UI currently writes capability.json. Use the broadly supported compact image
    // format for the first request; the provider response is the actual capability check.
    // A confirmed record may override this for servers with WebP or PNG support.
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(SendFormat::Jpeg),
        Err(_) => return Err(ImageError::Failed("image capability could not be read")),
    };
    let record: CapabilityRecord = serde_json::from_slice(&bytes).map_err(|_| {
        ImageError::Rejected("image input has not been confirmed for this provider")
    })?;
    if record.endpoint != endpoint || record.model != model {
        return Ok(SendFormat::Jpeg);
    }
    if record.format == SendFormat::Unsupported {
        return Err(ImageError::Rejected("this provider does not accept images"));
    }
    Ok(record.format)
}

fn validate_token(value: &str) -> Result<(), ImageError> {
    if value.len() > 80
        || value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
    {
        return Err(ImageError::Rejected("image reference is invalid"));
    }
    Ok(())
}

fn staging_path(data_directory: &Path, id: &str) -> Result<PathBuf, ImageError> {
    validate_token(id)?;
    let dir = data_directory.join("image-input").join("staging");
    fs::create_dir_all(&dir)
        .map_err(|_| ImageError::Failed("image directory could not be created"))?;
    Ok(dir.join(format!("{id}.webp")))
}

fn claimed_path(data_directory: &Path, run_id: &str) -> Result<PathBuf, ImageError> {
    validate_token(run_id)?;
    let dir = data_directory.join("image-input").join("claimed");
    fs::create_dir_all(&dir)
        .map_err(|_| ImageError::Failed("image directory could not be created"))?;
    Ok(dir.join(format!("{run_id}.webp")))
}

fn capability_path(data_directory: &Path) -> Result<PathBuf, ImageError> {
    let dir = data_directory.join("image-input");
    fs::create_dir_all(&dir)
        .map_err(|_| ImageError::Failed("image directory could not be created"))?;
    Ok(dir.join("capability.json"))
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<(), ImageError> {
    use std::io::Write;
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|_| ImageError::Failed("temporary image could not be written"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(|_| ImageError::Failed("temporary image permissions could not be set"))?;
    }
    file.write_all(bytes)
        .map_err(|_| ImageError::Failed("temporary image could not be written"))?;
    Ok(())
}

#[cfg(test)]
mod tests;
