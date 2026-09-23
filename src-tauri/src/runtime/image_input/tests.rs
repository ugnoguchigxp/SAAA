use super::*;
use image::{ImageBuffer, Rgba, RgbaImage};
use serde_json::json;

fn png(width: u32, height: u32, pixel: [u8; 4]) -> Vec<u8> {
    let image: RgbaImage = ImageBuffer::from_fn(width, height, |_, _| Rgba(pixel));
    let mut bytes = Vec::new();
    image::codecs::png::PngEncoder::new(&mut bytes)
        .write_image(
            image.as_raw(),
            width,
            height,
            image::ExtendedColorType::Rgba8,
        )
        .unwrap();
    bytes
}

#[test]
fn small_png_becomes_webp_and_original_is_not_stored() {
    let directory = tempfile::tempdir().unwrap();
    let source = png(32, 16, [20, 40, 200, 255]);
    let first = prepare(directory.path(), &source).unwrap();
    let second = prepare(directory.path(), &source).unwrap();
    assert_eq!(first.width, 32);
    assert_eq!(first.height, 16);
    assert_eq!(first.preview, second.preview);
    let staging = directory.path().join("image-input/staging");
    let stored = std::fs::read_dir(&staging).unwrap().count();
    assert_eq!(stored, 2);
    for entry in std::fs::read_dir(&staging).unwrap() {
        let bytes = std::fs::read(entry.unwrap().path()).unwrap();
        assert!(bytes
            .windows(8)
            .all(|window| window != b"\x89PNG\r\n\x1a\n"));
        assert!(!bytes.windows(6).any(|window| window == b"base64"));
    }
    discard(directory.path(), &first.id).unwrap();
    assert!(!directory
        .path()
        .join(format!("image-input/staging/{}.webp", first.id))
        .exists());
}

#[test]
fn long_edge_at_2048_shrinks_and_2047_does_not() {
    let directory = tempfile::tempdir().unwrap();
    let kept = prepare(directory.path(), &png(2047, 8, [255, 0, 0, 255])).unwrap();
    assert_eq!((kept.width, kept.height), (2047, 8));
    let shrunk = prepare(directory.path(), &png(2048, 8, [0, 180, 40, 255])).unwrap();
    assert_eq!(shrunk.width, TARGET_LONG_EDGE);
    assert!(shrunk.height >= 1);
}

#[test]
fn rejects_non_png_and_does_not_write() {
    let directory = tempfile::tempdir().unwrap();
    let error = prepare(directory.path(), b"not-a-png-but-long-enough").unwrap_err();
    assert!(error.to_string().contains("not a png"));
    assert!(
        std::fs::read_dir(directory.path().join("image-input/staging"))
            .map(|entries| entries.count() == 0)
            .unwrap_or(true)
    );
}

#[test]
fn provider_message_is_image_then_text_and_receipt_drops_bytes() {
    let directory = tempfile::tempdir().unwrap();
    let prepared = prepare(directory.path(), &png(12, 12, [10, 20, 30, 255])).unwrap();
    claim(directory.path(), &prepared.id, "run_image").unwrap();
    store_capability(
        directory.path(),
        "http://larm/v1",
        "gemma",
        SendFormat::Webp,
    )
    .unwrap();
    let mut messages = vec![json!({"role":"user","content":"画面の警告色は？"})];
    attach(
        directory.path(),
        &mut messages,
        "run_image",
        "画面の警告色は？",
        "http://larm/v1",
        "gemma",
    )
    .unwrap();
    let parts = messages[0]["content"].as_array().unwrap();
    assert_eq!(parts[0]["type"], "image_url");
    assert!(parts[0]["image_url"]["url"]
        .as_str()
        .unwrap()
        .starts_with("data:image/webp;base64,"));
    assert_eq!(parts[1]["text"], "画面の警告色は？");
    let body = serde_json::to_vec(&json!({"messages": messages})).unwrap();
    assert!(body.len() <= MAX_IMAGE_REQUEST_BYTES);
    let redacted = redact_provider_body(&body);
    let text = String::from_utf8(redacted).unwrap();
    assert!(!text.contains("base64"));
    assert!(text.contains("[image omitted]"));
}

#[test]
fn unconfirmed_provider_uses_jpeg_and_explicit_unsupported_refuses() {
    let directory = tempfile::tempdir().unwrap();
    let prepared = prepare(directory.path(), &png(8, 8, [1, 2, 3, 255])).unwrap();
    claim(directory.path(), &prepared.id, "run_plain").unwrap();
    let mut messages = vec![json!({"role":"user","content":"見て"})];
    attach(
        directory.path(),
        &mut messages,
        "run_plain",
        "見て",
        "http://local",
        "gemma",
    )
    .unwrap();
    assert!(messages[0]["content"][0]["image_url"]["url"]
        .as_str()
        .unwrap()
        .starts_with("data:image/jpeg;base64,"));
    store_capability(
        directory.path(),
        "http://local",
        "gemma",
        SendFormat::Unsupported,
    )
    .unwrap();
    let mut messages = vec![json!({"role":"user","content":"見て"})];
    let error = attach(
        directory.path(),
        &mut messages,
        "run_plain",
        "見て",
        "http://local",
        "gemma",
    )
    .unwrap_err();
    assert!(error.to_string().contains("does not accept images"));
}

#[test]
fn jpeg_transport_flattens_transparency_to_white() {
    let source = png(16, 16, [0, 0, 0, 0]);
    let encoded = encode_png(&source).unwrap();
    let (mime, jpeg) = transcode(&encoded.webp, SendFormat::Jpeg).unwrap();
    assert_eq!(mime, "image/jpeg");
    let decoded = image::load_from_memory_with_format(&jpeg, image::ImageFormat::Jpeg).unwrap();
    let pixel = decoded.to_rgb8().get_pixel(8, 8).0;
    assert!(pixel.iter().all(|channel| *channel > 245));
}
