//! Image handling for the cortex coding agent.
//!
//! Ports hoocode's `utils/mime.ts` (format sniffing) and `utils/image-resize.ts`
//! (fit an image within the model's inline limits). This crate owns the `image`
//! dependency; callers only see base64 strings and plain structs.

use base64::Engine as _;
use image::{DynamicImage, ImageDecoder, ImageReader};
use std::io::Cursor;

/// How many leading bytes `detectSupportedImageMimeTypeFromFile` sniffs.
pub const FILE_TYPE_SNIFF_BYTES: usize = 4100;

/// `IMAGE_MIME_TYPES`: the formats models accept inline.
pub const SUPPORTED_IMAGE_MIME_TYPES: [&str; 4] =
    ["image/jpeg", "image/png", "image/gif", "image/webp"];

/// PNG vs APNG, following `file-type`: walk the chunks until `IDAT` (PNG) or
/// `acTL` (APNG) shows up. `None` for an invalid stream.
fn sniff_png(buf: &[u8]) -> Option<&'static str> {
    const MAXIMUM_PNG_CHUNK_COUNT: usize = 512;
    let size = buf.len();
    let mut pos = 8usize;
    let mut seen_ihdr = false;
    let mut count = 0usize;
    loop {
        count += 1;
        if count > MAXIMUM_PNG_CHUNK_COUNT {
            break;
        }
        if pos + 8 > size {
            // Reading the chunk header ran off the end of the sample.
            return None;
        }
        let length = i32::from_be_bytes([buf[pos], buf[pos + 1], buf[pos + 2], buf[pos + 3]]);
        let kind = &buf[pos + 4..pos + 8];
        let previous = pos;
        pos += 8;
        if length < 0 {
            return None;
        }
        if kind == b"IHDR" {
            if length != 13 {
                return None;
            }
            seen_ihdr = true;
        }
        match kind {
            b"IDAT" => return Some("image/png"),
            b"acTL" => return Some("image/apng"),
            _ => {
                if !seen_ihdr && kind != b"CgBI" {
                    return None;
                }
                let skip = length as usize + 4;
                if pos + skip > size {
                    // The chunk runs past the sample: it is a PNG as far as we can tell.
                    return Some("image/png");
                }
                pos += skip;
            }
        }
        if pos <= previous || pos + 8 >= size {
            break;
        }
    }
    Some("image/png")
}

/// The MIME type `file-type` reports for the four image signatures we care
/// about (anything else is `None`).
fn sniff(buf: &[u8]) -> Option<&'static str> {
    // file-type strips a UTF-8 BOM and detects again.
    if let Some(rest) = buf.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return sniff(rest);
    }
    if buf.starts_with(&[0x47, 0x49, 0x46]) {
        return Some("image/gif");
    }
    if buf.starts_with(&[0xFF, 0xD8, 0xFF]) {
        // JPG7/SOF55 is JPEG-LS.
        return if buf.get(3) == Some(&0xF7) {
            Some("image/jls")
        } else {
            Some("image/jpeg")
        };
    }
    if buf.starts_with(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]) {
        return sniff_png(buf);
    }
    if buf.starts_with(b"RIFF") && buf.len() >= 12 && &buf[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    None
}

/// `detectSupportedImageMimeTypeFromFile` on the first bytes of a file (at
/// most [`FILE_TYPE_SNIFF_BYTES`]): the image MIME type if the magic bytes are
/// jpeg/png/gif/webp, otherwise `None` (the extension is never consulted).
pub fn detect_supported_image_mime_type(head: &[u8]) -> Option<&'static str> {
    if head.is_empty() {
        return None;
    }
    let head = &head[..head.len().min(FILE_TYPE_SNIFF_BYTES)];
    sniff(head).filter(|mime| SUPPORTED_IMAGE_MIME_TYPES.contains(mime))
}

/// `ImageResizeOptions`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ImageResizeOptions {
    pub max_width: u32,
    pub max_height: u32,
    /// Limit on the base64 payload size.
    pub max_bytes: usize,
    pub jpeg_quality: u8,
}

/// 4.5MB of base64 payload: headroom below Anthropic's 5MB limit.
pub const DEFAULT_MAX_IMAGE_BYTES: usize = 4_718_592;

impl Default for ImageResizeOptions {
    fn default() -> Self {
        Self {
            max_width: 2000,
            max_height: 2000,
            max_bytes: DEFAULT_MAX_IMAGE_BYTES,
            jpeg_quality: 80,
        }
    }
}

/// `ResizedImage`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResizedImage {
    /// Base64 data.
    pub data: String,
    pub mime_type: String,
    pub original_width: u32,
    pub original_height: u32,
    pub width: u32,
    pub height: u32,
    pub was_resized: bool,
}

struct Candidate {
    data: String,
    mime_type: &'static str,
}

fn encode(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn decode_oriented(bytes: &[u8]) -> Option<DynamicImage> {
    let reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    let mut decoder = reader.into_decoder().ok()?;
    let orientation = decoder.orientation().ok();
    let mut img = DynamicImage::from_decoder(decoder).ok()?;
    if let Some(orientation) = orientation {
        img.apply_orientation(orientation);
    }
    Some(img)
}

fn try_encodings(
    img: &DynamicImage,
    width: u32,
    height: u32,
    qualities: &[u8],
) -> Option<Vec<Candidate>> {
    let resized = img.resize_exact(width, height, image::imageops::FilterType::Lanczos3);
    let mut png = Vec::new();
    resized
        .write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
        .ok()?;
    let mut candidates = vec![Candidate {
        data: encode(&png),
        mime_type: "image/png",
    }];
    let rgb = DynamicImage::ImageRgb8(resized.to_rgb8());
    for &quality in qualities {
        let mut jpeg = Vec::new();
        let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, quality);
        rgb.write_with_encoder(encoder).ok()?;
        candidates.push(Candidate {
            data: encode(&jpeg),
            mime_type: "image/jpeg",
        });
    }
    Some(candidates)
}

/// JS `Math.round` for the non-negative values used here.
fn js_round(x: f64) -> u32 {
    (x + 0.5).floor() as u32
}

/// Resize an image (base64 `data` of type `mime_type`) to fit within the max
/// dimensions and encoded size (`resizeImage`). `None` if it cannot be decoded
/// or cannot be brought below `max_bytes`.
///
/// Strategy: resize to max width/height, try PNG and JPEG (picking the first
/// that fits), then lower JPEG quality, then shrink dimensions by 0.75 steps
/// down to 1x1.
pub fn resize_image(
    data: &str,
    mime_type: Option<&str>,
    options: ImageResizeOptions,
) -> Option<ResizedImage> {
    let input = base64::engine::general_purpose::STANDARD
        .decode(data)
        .ok()?;
    let img = decode_oriented(&input)?;
    let original_width = img.width();
    let original_height = img.height();

    if original_width <= options.max_width
        && original_height <= options.max_height
        && data.len() < options.max_bytes
    {
        let format = mime_type.and_then(|m| m.split('/').nth(1)).unwrap_or("png");
        return Some(ResizedImage {
            data: data.to_string(),
            mime_type: mime_type
                .map(str::to_string)
                .unwrap_or_else(|| format!("image/{format}")),
            original_width,
            original_height,
            width: original_width,
            height: original_height,
            was_resized: false,
        });
    }

    let mut target_width = original_width;
    let mut target_height = original_height;
    if target_width > options.max_width {
        target_height = js_round(
            f64::from(target_height) * f64::from(options.max_width) / f64::from(target_width),
        );
        target_width = options.max_width;
    }
    if target_height > options.max_height {
        target_width = js_round(
            f64::from(target_width) * f64::from(options.max_height) / f64::from(target_height),
        );
        target_height = options.max_height;
    }

    let mut qualities: Vec<u8> = Vec::new();
    for q in [options.jpeg_quality, 85, 70, 55, 40] {
        if !qualities.contains(&q) {
            qualities.push(q);
        }
    }

    let mut width = target_width;
    let mut height = target_height;
    loop {
        for candidate in try_encodings(&img, width, height, &qualities)? {
            if candidate.data.len() < options.max_bytes {
                return Some(ResizedImage {
                    data: candidate.data,
                    mime_type: candidate.mime_type.to_string(),
                    original_width,
                    original_height,
                    width,
                    height,
                    was_resized: true,
                });
            }
        }
        if width == 1 && height == 1 {
            break;
        }
        let next_width = if width == 1 {
            1
        } else {
            ((f64::from(width) * 0.75).floor() as u32).max(1)
        };
        let next_height = if height == 1 {
            1
        } else {
            ((f64::from(height) * 0.75).floor() as u32).max(1)
        };
        if next_width == width && next_height == height {
            break;
        }
        width = next_width;
        height = next_height;
    }
    None
}

/// `n / d` formatted like JS `(n / d).toFixed(2)` (ties round up).
fn to_fixed_2(n: u32, d: u32) -> String {
    let n = u64::from(n);
    let d = u64::from(d);
    let hundredths = (n * 200 + d) / (2 * d);
    format!("{}.{:02}", hundredths / 100, hundredths % 100)
}

/// A note that tells the model how resized coordinates map back to the
/// original image (`formatDimensionNote`). `None` when nothing was resized.
pub fn format_dimension_note(result: &ResizedImage) -> Option<String> {
    if !result.was_resized {
        return None;
    }
    Some(format!(
        "[Image: original {}x{}, displayed at {}x{}. Multiply coordinates by {} to map to original image.]",
        result.original_width,
        result.original_height,
        result.width,
        result.height,
        to_fixed_2(result.original_width, result.width)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PNG_1X1: &str =
        "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGNgYGD4DwABBAEAX+XDSwAAAABJRU5ErkJggg==";

    fn png_bytes(width: u32, height: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_fn(width, height, |x, y| {
            image::Rgba([(x % 256) as u8, (y % 256) as u8, ((x * y) % 256) as u8, 255])
        });
        let mut out = Vec::new();
        DynamicImage::ImageRgba8(img)
            .write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png)
            .unwrap();
        out
    }

    #[test]
    fn sniffs_the_supported_formats_by_magic() {
        let png = base64::engine::general_purpose::STANDARD
            .decode(PNG_1X1)
            .unwrap();
        assert_eq!(detect_supported_image_mime_type(&png), Some("image/png"));
        assert_eq!(
            detect_supported_image_mime_type(&[0xFF, 0xD8, 0xFF, 0xE0]),
            Some("image/jpeg")
        );
        assert_eq!(
            detect_supported_image_mime_type(b"GIF89a...."),
            Some("image/gif")
        );
        assert_eq!(
            detect_supported_image_mime_type(b"RIFF\0\0\0\0WEBPVP8 "),
            Some("image/webp")
        );
        assert_eq!(
            detect_supported_image_mime_type(b"definitely not a png"),
            None
        );
        assert_eq!(detect_supported_image_mime_type(b""), None);
        // JPEG-LS and a RIFF that isn't WebP are not supported images.
        assert_eq!(
            detect_supported_image_mime_type(&[0xFF, 0xD8, 0xFF, 0xF7]),
            None
        );
        assert_eq!(
            detect_supported_image_mime_type(b"RIFF\0\0\0\0WAVEfmt "),
            None
        );
        // A UTF-8 BOM is skipped, as file-type does.
        assert_eq!(
            detect_supported_image_mime_type(b"\xEF\xBB\xBFGIF89a"),
            Some("image/gif")
        );
    }

    #[test]
    fn apng_is_not_a_supported_png() {
        let mut apng = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        apng.extend_from_slice(&13u32.to_be_bytes());
        apng.extend_from_slice(b"IHDR");
        apng.extend_from_slice(&[0; 13 + 4]);
        apng.extend_from_slice(&8u32.to_be_bytes());
        apng.extend_from_slice(b"acTL");
        apng.extend_from_slice(&[0; 8 + 4]);
        apng.extend_from_slice(&[0; 16]);
        assert_eq!(detect_supported_image_mime_type(&apng), None);
        // The same stream with IDAT instead is a PNG.
        let png: Vec<u8> = apng
            .iter()
            .copied()
            .enumerate()
            .map(|(i, b)| {
                if (37..41).contains(&i) {
                    b"IDAT"[i - 37]
                } else {
                    b
                }
            })
            .collect();
        assert_eq!(detect_supported_image_mime_type(&png), Some("image/png"));
    }

    #[test]
    fn small_images_pass_through_untouched() {
        let resized =
            resize_image(PNG_1X1, Some("image/png"), ImageResizeOptions::default()).unwrap();
        assert!(!resized.was_resized);
        assert_eq!(resized.data, PNG_1X1);
        assert_eq!(resized.mime_type, "image/png");
        assert_eq!((resized.width, resized.height), (1, 1));
        assert_eq!(format_dimension_note(&resized), None);
    }

    #[test]
    fn large_images_are_scaled_to_fit() {
        let data = encode(&png_bytes(300, 150));
        let options = ImageResizeOptions {
            max_width: 100,
            max_height: 100,
            ..Default::default()
        };
        let resized = resize_image(&data, Some("image/png"), options).unwrap();
        assert!(resized.was_resized);
        assert_eq!(
            (resized.original_width, resized.original_height),
            (300, 150)
        );
        assert_eq!((resized.width, resized.height), (100, 50));
        assert_eq!(
            format_dimension_note(&resized).unwrap(),
            "[Image: original 300x150, displayed at 100x50. Multiply coordinates by 3.00 to map to original image.]"
        );
    }

    #[test]
    fn byte_limit_forces_jpeg_or_smaller_dimensions() {
        let data = encode(&png_bytes(200, 200));
        let options = ImageResizeOptions {
            max_bytes: 4000,
            ..Default::default()
        };
        let resized = resize_image(&data, Some("image/png"), options).unwrap();
        assert!(resized.was_resized);
        assert!(resized.data.len() < 4000);
        // Nothing fits: None.
        let options = ImageResizeOptions {
            max_bytes: 10,
            ..Default::default()
        };
        assert_eq!(resize_image(&data, Some("image/png"), options), None);
    }

    #[test]
    fn undecodable_data_is_none() {
        let data = encode(b"not an image at all");
        assert_eq!(
            resize_image(&data, Some("image/png"), ImageResizeOptions::default()),
            None
        );
    }

    #[test]
    fn to_fixed_2_rounds_ties_up() {
        assert_eq!(to_fixed_2(9, 8), "1.13");
        assert_eq!(to_fixed_2(4000, 2000), "2.00");
        assert_eq!(to_fixed_2(1, 3), "0.33");
    }
}
