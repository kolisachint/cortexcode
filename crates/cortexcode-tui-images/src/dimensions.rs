//! Pixel-dimension sniffing for PNG/JPEG/GIF/WebP images from base64 data,
//! ported from `terminal-image.ts`'s `get*Dimensions` functions.

use base64::Engine;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageDimensions {
    pub width_px: u32,
    pub height_px: u32,
}

fn decode(base64_data: &str) -> Option<Vec<u8>> {
    base64::engine::general_purpose::STANDARD
        .decode(base64_data)
        .ok()
}

pub fn get_png_dimensions(base64_data: &str) -> Option<ImageDimensions> {
    let buf = decode(base64_data)?;
    if buf.len() < 24 {
        return None;
    }
    if buf[0..4] != [0x89, 0x50, 0x4e, 0x47] {
        return None;
    }
    let width = u32::from_be_bytes(buf[16..20].try_into().ok()?);
    let height = u32::from_be_bytes(buf[20..24].try_into().ok()?);
    Some(ImageDimensions {
        width_px: width,
        height_px: height,
    })
}

pub fn get_jpeg_dimensions(base64_data: &str) -> Option<ImageDimensions> {
    let buf = decode(base64_data)?;
    if buf.len() < 2 {
        return None;
    }
    if buf[0] != 0xff || buf[1] != 0xd8 {
        return None;
    }

    let mut offset = 2usize;
    while offset < buf.len().saturating_sub(9) {
        if buf[offset] != 0xff {
            offset += 1;
            continue;
        }
        let marker = buf[offset + 1];
        if (0xc0..=0xc2).contains(&marker) {
            let height = u16::from_be_bytes(buf[offset + 5..offset + 7].try_into().ok()?);
            let width = u16::from_be_bytes(buf[offset + 7..offset + 9].try_into().ok()?);
            return Some(ImageDimensions {
                width_px: width as u32,
                height_px: height as u32,
            });
        }
        if offset + 3 >= buf.len() {
            return None;
        }
        let length = u16::from_be_bytes(buf[offset + 2..offset + 4].try_into().ok()?) as usize;
        if length < 2 {
            return None;
        }
        offset += 2 + length;
    }
    None
}

pub fn get_gif_dimensions(base64_data: &str) -> Option<ImageDimensions> {
    let buf = decode(base64_data)?;
    if buf.len() < 10 {
        return None;
    }
    let sig = std::str::from_utf8(&buf[0..6]).ok()?;
    if sig != "GIF87a" && sig != "GIF89a" {
        return None;
    }
    let width = u16::from_le_bytes(buf[6..8].try_into().ok()?);
    let height = u16::from_le_bytes(buf[8..10].try_into().ok()?);
    Some(ImageDimensions {
        width_px: width as u32,
        height_px: height as u32,
    })
}

pub fn get_webp_dimensions(base64_data: &str) -> Option<ImageDimensions> {
    let buf = decode(base64_data)?;
    if buf.len() < 30 {
        return None;
    }
    let riff = std::str::from_utf8(&buf[0..4]).ok()?;
    let webp = std::str::from_utf8(&buf[8..12]).ok()?;
    if riff != "RIFF" || webp != "WEBP" {
        return None;
    }
    let chunk = std::str::from_utf8(&buf[12..16]).ok()?;
    match chunk {
        "VP8 " => {
            let width = u16::from_le_bytes(buf[26..28].try_into().ok()?) & 0x3fff;
            let height = u16::from_le_bytes(buf[28..30].try_into().ok()?) & 0x3fff;
            Some(ImageDimensions {
                width_px: width as u32,
                height_px: height as u32,
            })
        }
        "VP8L" => {
            if buf.len() < 25 {
                return None;
            }
            let bits = u32::from_le_bytes(buf[21..25].try_into().ok()?);
            let width = (bits & 0x3fff) + 1;
            let height = ((bits >> 14) & 0x3fff) + 1;
            Some(ImageDimensions {
                width_px: width,
                height_px: height,
            })
        }
        "VP8X" => {
            let width = (buf[24] as u32 | (buf[25] as u32) << 8 | (buf[26] as u32) << 16) + 1;
            let height = (buf[27] as u32 | (buf[28] as u32) << 8 | (buf[29] as u32) << 16) + 1;
            Some(ImageDimensions {
                width_px: width,
                height_px: height,
            })
        }
        _ => None,
    }
}

pub fn get_image_dimensions(base64_data: &str, mime_type: &str) -> Option<ImageDimensions> {
    match mime_type {
        "image/png" => get_png_dimensions(base64_data),
        "image/jpeg" => get_jpeg_dimensions(base64_data),
        "image/gif" => get_gif_dimensions(base64_data),
        "image/webp" => get_webp_dimensions(base64_data),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;

    fn b64(bytes: &[u8]) -> String {
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    #[test]
    fn png_dimensions() {
        let mut buf = vec![0x89, 0x50, 0x4e, 0x47, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        buf.extend_from_slice(&100u32.to_be_bytes());
        buf.extend_from_slice(&200u32.to_be_bytes());
        let dims = get_png_dimensions(&b64(&buf)).unwrap();
        assert_eq!(dims.width_px, 100);
        assert_eq!(dims.height_px, 200);
    }

    #[test]
    fn png_dimensions_rejects_bad_signature() {
        let buf = vec![0u8; 24];
        assert!(get_png_dimensions(&b64(&buf)).is_none());
    }

    #[test]
    fn gif_dimensions() {
        let mut buf = b"GIF89a".to_vec();
        buf.extend_from_slice(&320u16.to_le_bytes());
        buf.extend_from_slice(&240u16.to_le_bytes());
        buf.extend_from_slice(&[0u8; 2]);
        let dims = get_gif_dimensions(&b64(&buf)).unwrap();
        assert_eq!(dims.width_px, 320);
        assert_eq!(dims.height_px, 240);
    }

    #[test]
    fn webp_vp8x_dimensions() {
        let mut buf = vec![0u8; 30];
        buf[0..4].copy_from_slice(b"RIFF");
        buf[8..12].copy_from_slice(b"WEBP");
        buf[12..16].copy_from_slice(b"VP8X");
        // width-1 = 99 (little endian 24-bit), height-1 = 199
        buf[24] = 99;
        buf[25] = 0;
        buf[26] = 0;
        buf[27] = 199;
        buf[28] = 0;
        buf[29] = 0;
        let dims = get_webp_dimensions(&b64(&buf)).unwrap();
        assert_eq!(dims.width_px, 100);
        assert_eq!(dims.height_px, 200);
    }

    #[test]
    fn image_dimensions_dispatches_by_mime_type() {
        let mut buf = vec![0x89, 0x50, 0x4e, 0x47, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        buf.extend_from_slice(&10u32.to_be_bytes());
        buf.extend_from_slice(&20u32.to_be_bytes());
        let data = b64(&buf);
        assert!(get_image_dimensions(&data, "image/png").is_some());
        assert!(get_image_dimensions(&data, "image/tiff").is_none());
    }
}
