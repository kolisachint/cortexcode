//! Kitty graphics protocol encoding, ported from `terminal-image.ts`.

const CHUNK_SIZE: usize = 4096;

#[derive(Debug, Clone, Default)]
pub struct KittyEncodeOptions {
    pub columns: Option<u32>,
    pub rows: Option<u32>,
    pub image_id: Option<u32>,
    /// Whether Kitty should apply its default cursor movement after placement (default: true).
    pub move_cursor: Option<bool>,
}

pub fn encode_kitty(base64_data: &str, options: &KittyEncodeOptions) -> String {
    let mut params: Vec<String> = vec!["a=T".into(), "f=100".into(), "q=2".into()];
    if options.move_cursor == Some(false) {
        params.push("C=1".into());
    }
    if let Some(c) = options.columns {
        params.push(format!("c={c}"));
    }
    if let Some(r) = options.rows {
        params.push(format!("r={r}"));
    }
    if let Some(i) = options.image_id {
        params.push(format!("i={i}"));
    }
    let params_joined = params.join(",");

    if base64_data.len() <= CHUNK_SIZE {
        return format!("\x1b_G{params_joined};{base64_data}\x1b\\");
    }

    let mut chunks = Vec::new();
    let mut offset = 0usize;
    let mut is_first = true;
    let bytes = base64_data.as_bytes();

    while offset < bytes.len() {
        let end = (offset + CHUNK_SIZE).min(bytes.len());
        let chunk = &base64_data[offset..end];
        let is_last = end >= bytes.len();

        if is_first {
            chunks.push(format!("\x1b_G{params_joined},m=1;{chunk}\x1b\\"));
            is_first = false;
        } else if is_last {
            chunks.push(format!("\x1b_Gm=0;{chunk}\x1b\\"));
        } else {
            chunks.push(format!("\x1b_Gm=1;{chunk}\x1b\\"));
        }

        offset = end;
    }

    chunks.join("")
}

/// Delete a Kitty graphics image by ID (uppercase `I` also frees image data).
pub fn delete_kitty_image(image_id: u32) -> String {
    format!("\x1b_Ga=d,d=I,i={image_id},q=2\x1b\\")
}

/// Delete all visible Kitty graphics images (uppercase `A` also frees image data).
pub fn delete_all_kitty_images() -> String {
    "\x1b_Ga=d,d=A,q=2\x1b\\".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_short_payload_in_one_sequence() {
        let seq = encode_kitty("AAAA", &KittyEncodeOptions::default());
        assert_eq!(seq, "\x1b_Ga=T,f=100,q=2;AAAA\x1b\\");
    }

    #[test]
    fn requests_no_cursor_movement() {
        let seq = encode_kitty(
            "AAAA",
            &KittyEncodeOptions {
                columns: Some(2),
                rows: Some(2),
                move_cursor: Some(false),
                ..Default::default()
            },
        );
        assert!(seq.starts_with("\x1b_Ga=T,f=100,q=2,C=1,c=2,r=2;"));
    }

    #[test]
    fn chunks_long_payloads() {
        let data = "A".repeat(CHUNK_SIZE * 2 + 10);
        let seq = encode_kitty(&data, &KittyEncodeOptions::default());
        assert!(seq.starts_with("\x1b_Ga=T,f=100,q=2,m=1;"));
        assert!(seq.contains("\x1b_Gm=1;"));
        assert!(seq.contains("\x1b_Gm=0;"));
        assert!(seq.ends_with("\x1b\\"));
    }

    #[test]
    fn delete_sequences_suppress_replies() {
        assert_eq!(delete_kitty_image(42), "\x1b_Ga=d,d=I,i=42,q=2\x1b\\");
        assert_eq!(delete_all_kitty_images(), "\x1b_Ga=d,d=A,q=2\x1b\\");
    }
}
