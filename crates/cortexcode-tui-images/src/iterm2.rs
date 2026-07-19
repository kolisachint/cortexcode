//! iTerm2 inline image protocol encoding, ported from `terminal-image.ts`.

use base64::Engine;

#[derive(Debug, Clone, Default)]
pub struct ITerm2EncodeOptions {
    pub width: Option<String>,
    pub height: Option<String>,
    pub name: Option<String>,
    pub preserve_aspect_ratio: Option<bool>,
    pub inline: Option<bool>,
}

pub fn encode_iterm2(base64_data: &str, options: &ITerm2EncodeOptions) -> String {
    let mut params = vec![format!(
        "inline={}",
        if options.inline != Some(false) { 1 } else { 0 }
    )];

    if let Some(w) = &options.width {
        params.push(format!("width={w}"));
    }
    if let Some(h) = &options.height {
        params.push(format!("height={h}"));
    }
    if let Some(name) = &options.name {
        let name_base64 = base64::engine::general_purpose::STANDARD.encode(name.as_bytes());
        params.push(format!("name={name_base64}"));
    }
    if options.preserve_aspect_ratio == Some(false) {
        params.push("preserveAspectRatio=0".to_string());
    }

    format!("\x1b]1337;File={}:{base64_data}\x07", params.join(";"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_with_defaults() {
        let seq = encode_iterm2("AAAA", &ITerm2EncodeOptions::default());
        assert_eq!(seq, "\x1b]1337;File=inline=1:AAAA\x07");
    }

    #[test]
    fn encodes_with_width_height_name() {
        let seq = encode_iterm2(
            "AAAA",
            &ITerm2EncodeOptions {
                width: Some("80".into()),
                height: Some("auto".into()),
                name: Some("cat.png".into()),
                ..Default::default()
            },
        );
        assert!(seq.contains("width=80"));
        assert!(seq.contains("height=auto"));
        assert!(seq.contains("name="));
    }

    #[test]
    fn disables_preserve_aspect_ratio() {
        let seq = encode_iterm2(
            "AAAA",
            &ITerm2EncodeOptions {
                preserve_aspect_ratio: Some(false),
                ..Default::default()
            },
        );
        assert!(seq.contains("preserveAspectRatio=0"));
    }
}
