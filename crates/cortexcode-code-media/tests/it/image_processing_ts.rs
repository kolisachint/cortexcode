//! Case-for-case port of the pin's `coding-agent/test/image-processing.test.ts`,
//! with its fixtures. `convertToPng` takes base64 and passes PNG through; the
//! Rust `convert_to_png` takes bytes and its caller (clipboard paste) only
//! sends it formats a model cannot take, so the PNG case checks that a PNG
//! still converts to a PNG.

use base64::Engine as _;
use cortexcode_code_media::clipboard_image::convert_to_png;
use cortexcode_code_media::{
    format_dimension_note, resize_image, ImageResizeOptions, ResizedImage,
};

const TINY_PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAIAAAACAQMAAABIeJ9nAAAAIGNIUk0AAHomAACAhAAA+gAAAIDoAAB1MAAA6mAAADqYAAAXcJy6UTwAAAAGUExURf8AAP///0EdNBEAAAABYktHRAH/Ai3eAAAAB3RJTUUH6gEOADM5Ddoh/wAAAAxJREFUCNdjYGBgAAAABAABJzQnCgAAACV0RVh0ZGF0ZTpjcmVhdGUAMjAyNi0wMS0xNFQwMDo1MTo1NyswMDowMOnKzHgAAAAldEVYdGRhdGU6bW9kaWZ5ADIwMjYtMDEtMTRUMDA6NTE6NTcrMDA6MDCYl3TEAAAAKHRFWHRkYXRlOnRpbWVzdGFtcAAyMDI2LTAxLTE0VDAwOjUxOjU3KzAwOjAwz4JVGwAAAABJRU5ErkJggg==";
const TINY_JPEG: &str = "/9j/4AAQSkZJRgABAQAAAQABAAD/2wBDAAMCAgMCAgMDAwMEAwMEBQgFBQQEBQoHBwYIDAoMDAsKCwsNDhIQDQ4RDgsLEBYQERMUFRUVDA8XGBYUGBIUFRT/2wBDAQMEBAUEBQkFBQkUDQsNFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBT/wAARCAACAAIDAREAAhEBAxEB/8QAFAABAAAAAAAAAAAAAAAAAAAACf/EABQQAQAAAAAAAAAAAAAAAAAAAAD/xAAVAQEBAAAAAAAAAAAAAAAAAAAGCf/EABQRAQAAAAAAAAAAAAAAAAAAAAD/2gAMAwEAAhEDEQA/AD3VTB3/2Q==";
const MEDIUM_PNG_100X100: &str = "iVBORw0KGgoAAAANSUhEUgAAAGQAAABkCAAAAABVicqIAAAAAmJLR0QA/4ePzL8AAAAHdElNRQfqAQ4AMzkN2iH/AAAAP0lEQVRo3u3NQQEAAAQEMASXXYrz2gqst/Lm4ZBIJBKJRCKRSCQSiUQikUgkEolEIpFIJBKJRCKRSCQSiSTsAP1cAUZeKtreAAAAJXRFWHRkYXRlOmNyZWF0ZQAyMDI2LTAxLTE0VDAwOjUxOjU3KzAwOjAw6crMeAAAACV0RVh0ZGF0ZTptb2RpZnkAMjAyNi0wMS0xNFQwMDo1MTo1NyswMDowMJiXdMQAAAAodEVYdGRhdGU6dGltZXN0YW1wADIwMjYtMDEtMTRUMDA6NTE6NTcrMDA6MDDPglUbAAAAAElFTkSuQmCC";
const LARGE_PNG_200X200: &str = "iVBORw0KGgoAAAANSUhEUgAAAMgAAADIEAIAAAByquWKAAAAIGNIUk0AAHomAACAhAAA+gAAAIDoAAB1MAAA6mAAADqYAAAXcJy6UTwAAAAGYktHRP///////wlY99wAAAAHdElNRQfqAQ4AMzqU03BFAAAEEElEQVR42u3dvWqUURSF4XXMRAyIhSAWQvwnWtjaWusFx9bW1kKD/wELESxEiBjDjJdwysOC57mCXW32+zGcGbtdACpsdi+T8Xz1GABzm91xMl6sHgNgblzcT/Y+rh4DYG6cv0/2H64eA2BOEgI1fHQHaoyzbXIwVo8BMCcJgRqSEKgxft9Lrn5aPQbA3Ph1klw7Wj0GwJwkBGpYWECN8fMiub63egyAORcWUMPCAmqMH3eTG59XjwEwN76/S24+Wj0GwJwkBGpYWECN8e1fcmuzegyAORcWUMPCAmqM09vJ4dfVYwDMjS9vkzuPV48BMCcJgRqeSAZqjA/nyYP91WMAzElCoIYkBGqMk8Pk6HT1GABzm+3L5JIkBApIQqCGj+5AjfHmb/Lk8uoxAOYkIVBDEgI1LCygxmZ7nFyShEABFxZQw8ICaozXf5KnV1aPATDnwgJqWFhADQsLqOFnDUANFxZQw8ICaoxXZ8mzg9VjAMy5sIAaFhZQw3tYQA1/QgHUkIRADUkI1JCEQA1JCNSQhEANFxZQwzcsoIYkBGpIQqCGJARqSEKghiQEalhYQA1/QgHUcGEBNSwsoIYkBGq4sIAaFhZQw8ICaviGBdRwYQE1LCyghiQEariwgBoWFlDDe1hADS+OAjUkIVBDEgI1JCFQQxICNSQhUMOFBdTwDQuoIQmBGpIQqCEJgRqSEKghCYEaFhZQw4ujQA0XFlDDwgJqSEKghgsLqGFhATUsLKCGb1hADRcWUMPCAmpIQqCGCwuoYWEBNbyHBdTw4ihQQxICNSQhUEMSAjUkIVBDEgI1XFhADd+wgBqSEKghCYEakhCoIQmBGpIQqGFhATW8OArUcGEBNSwsoIYkBGq4sIAaFhZQw8ICaviGBdRwYQE1LCyghiQEariwgBoWFlDDe1hADS+OAjUkIVBDEgI1JCFQQxICNSQhUMOFBdTwDQuoIQmBGpIQqCEJgRqSEKghCYEa49lB8ups9RgAc14cBWpIQqCGhQXUkIRADRcWUMPCAmqMp1eS139WjwEw5xsWUEMSAjUsLKCGJARquLCAGhYWUGM8uZy8+bt6DIA5L44CNSQhUMMTyUCNcXSanByuHgNgThICNSQhUGM82E8+nK8eA2DOzxqAGpIQqDHuPE6+vF09BsDcOPyanN5ePQbAnCQEavgdFlBj3Nok3/6tHgNgzoujQA1JCNQYNx8l39+tHgNgbtz4nPy4u3oMgDlJCNSwsIAa4/pe8vNi9RgAc37WANSQhECNce0o+XWyegyAuXH1U/L73uoxAOYkIVDDwgJqjIORnG1XjwEw508ogBqSEKgx9h8m5+9XjwEwN/Y+Jhf3V48BMCcJgRpjPE+2x6vHAJgbSbLbrR4DYO4/GqiSgXN+ksgAAAAldEVYdGRhdGU6Y3JlYXRlADIwMjYtMDEtMTRUMDA6NTE6NTcrMDA6MDDpysx4AAAAJXRFWHRkYXRlOm1vZGlmeQAyMDI2LTAxLTE0VDAwOjUxOjU3KzAwOjAwmJd0xAAAACh0RVh0ZGF0ZTp0aW1lc3RhbXAAMjAyNi0wMS0xNFQwMDo1MTo1NyswMDowMM+CVRsAAAAASUVORK5CYII=";

fn b64(data: &str) -> Vec<u8> {
    base64::engine::general_purpose::STANDARD
        .decode(data)
        .unwrap()
}

fn limits(max_width: u32, max_height: u32, max_bytes: usize) -> ImageResizeOptions {
    ImageResizeOptions {
        max_width,
        max_height,
        max_bytes,
        ..Default::default()
    }
}

fn is_png(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0x89, 0x50, 0x4e, 0x47])
}

#[test]
fn convert_png_input_stays_png() {
    assert!(is_png(&convert_to_png(&b64(TINY_PNG)).unwrap()));
}

#[test]
fn converts_jpeg_to_png() {
    assert!(is_png(&convert_to_png(&b64(TINY_JPEG)).unwrap()));
}

#[test]
fn returns_original_image_within_limits() {
    let r = resize_image(TINY_PNG, Some("image/png"), limits(100, 100, 1024 * 1024)).unwrap();
    assert!(!r.was_resized);
    assert_eq!(r.data, TINY_PNG);
    assert_eq!(
        (r.original_width, r.original_height, r.width, r.height),
        (2, 2, 2, 2)
    );
}

#[test]
fn resizes_image_exceeding_dimension_limits() {
    let r = resize_image(
        MEDIUM_PNG_100X100,
        Some("image/png"),
        limits(50, 50, 1024 * 1024),
    )
    .unwrap();
    assert!(r.was_resized);
    assert_eq!((r.original_width, r.original_height), (100, 100));
    assert!(r.width <= 50 && r.height <= 50);
}

#[test]
fn resizes_image_exceeding_byte_limit() {
    let original = b64(LARGE_PNG_200X200).len();
    let max = (LARGE_PNG_200X200.len() as f64 * 0.9).floor() as usize;
    let r = resize_image(
        LARGE_PNG_200X200,
        Some("image/png"),
        limits(2000, 2000, max),
    )
    .unwrap();
    assert!(b64(&r.data).len() < original);
    assert!(r.data.len() < LARGE_PNG_200X200.len());
}

#[test]
fn returns_none_when_it_cannot_get_below_max_bytes() {
    assert!(resize_image(LARGE_PNG_200X200, Some("image/png"), limits(2000, 2000, 1)).is_none());
}

#[test]
fn handles_jpeg_input() {
    let r = resize_image(TINY_JPEG, Some("image/jpeg"), limits(100, 100, 1024 * 1024)).unwrap();
    assert!(!r.was_resized);
    assert_eq!((r.original_width, r.original_height), (2, 2));
}

fn resized(ow: u32, oh: u32, w: u32, h: u32, was_resized: bool) -> ResizedImage {
    ResizedImage {
        data: String::new(),
        mime_type: "image/png".into(),
        original_width: ow,
        original_height: oh,
        width: w,
        height: h,
        was_resized,
    }
}

#[test]
fn no_dimension_note_for_non_resized_images() {
    assert_eq!(
        format_dimension_note(&resized(100, 100, 100, 100, false)),
        None
    );
}

#[test]
fn dimension_note_for_resized_images() {
    let note = format_dimension_note(&resized(2000, 1000, 1000, 500, true)).unwrap();
    assert!(note.contains("original 2000x1000"));
    assert!(note.contains("displayed at 1000x500"));
    assert!(note.contains("2.00"));
}
