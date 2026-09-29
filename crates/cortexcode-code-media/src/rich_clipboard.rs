//! Two flavours of the same thing on the clipboard at once, hoocode
//! `utils/rich-clipboard.ts`.
//!
//! Markdown on `text/plain`, HTML on the platform's rich flavour, from one
//! call. macOS takes both onto the general pasteboard (JXA, payloads read from
//! files so the script stays ASCII); Windows takes both through a
//! `DataObject` with the HTML in the CF_HTML envelope (PowerShell `-STA`).
//! Linux cannot offer two flavours from one owner without replacing the text
//! with HTML source, so it gets the markdown. Every path falls back to
//! [`copy_to_clipboard`], and the caller is told which flavour landed.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::clipboard::{copy_to_clipboard, ClipboardHost, Platform};

/// What actually reached the clipboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyFlavour {
    Rich,
    Text,
}

/// `RichPayload`.
#[derive(Debug, Clone)]
pub struct RichPayload {
    /// The `text/plain` flavour: markdown.
    pub text: String,
    /// The rich flavour: an HTML fragment, no `<html>` or `<body>`.
    pub html: String,
}

/// Past this the rich flavour is dropped: a session that large is an export,
/// not a paste.
const MAX_RICH_BYTES: usize = 2_000_000;

/// `wrapCfHtml`: the CF_HTML envelope Windows requires. The header carries
/// byte offsets into the string that contains it, as zero-padded ten-digit
/// fields so they are the width they will be when written.
pub fn wrap_cf_html(fragment: &str) -> String {
    let header = "Version:0.9\r\nStartHTML:%%%1\r\nEndHTML:%%%2\r\nStartFragment:%%%3\r\nEndFragment:%%%4\r\n";
    let open = "<html><body>\r\n<!--StartFragment-->";
    let close = "<!--EndFragment-->\r\n</body></html>";
    let pad = |value: usize| format!("{value:010}");
    let header_length = header.len() + 4 * (10 - 4);
    let start_html = header_length;
    let start_fragment = start_html + open.len();
    let end_fragment = start_fragment + fragment.len();
    let end_html = end_fragment + close.len();
    format!(
        "{}{open}{fragment}{close}",
        header
            .replace("%%%1", &pad(start_html))
            .replace("%%%2", &pad(end_html))
            .replace("%%%3", &pad(start_fragment))
            .replace("%%%4", &pad(end_fragment))
    )
}

/// Run a command to completion, quietly; false on any failure.
fn run(command: &str, args: &[&str]) -> bool {
    Command::new(command)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn write(path: &Path, contents: &str) -> std::io::Result<()> {
    std::fs::write(path, contents)
}

fn js_string(path: &Path) -> String {
    serde_json::to_string(&path.to_string_lossy()).unwrap_or_default()
}

fn copy_rich_darwin(payload: &RichPayload, dir: &Path) -> std::io::Result<bool> {
    let html_path = dir.join("clip.html");
    let text_path = dir.join("clip.txt");
    let script_path = dir.join("clip.js");
    write(&html_path, &payload.html)?;
    write(&text_path, &payload.text)?;
    let read = |path: &Path| {
        format!(
            "$.NSString.stringWithContentsOfFileEncodingError({}, $.NSUTF8StringEncoding, null)",
            js_string(path)
        )
    };
    let script = [
        "ObjC.import('AppKit');".to_string(),
        "var pb = $.NSPasteboard.generalPasteboard;".to_string(),
        "pb.clearContents;".to_string(),
        format!(
            "pb.setStringForType({}, $.NSPasteboardTypeHTML);",
            read(&html_path)
        ),
        format!(
            "pb.setStringForType({}, $.NSPasteboardTypeString);",
            read(&text_path)
        ),
    ]
    .join("\n");
    write(&script_path, &script)?;
    Ok(run(
        "osascript",
        &["-l", "JavaScript", &script_path.to_string_lossy()],
    ))
}

fn copy_rich_win32(payload: &RichPayload, dir: &Path) -> std::io::Result<bool> {
    let html_path = dir.join("clip.html");
    let text_path = dir.join("clip.txt");
    let script_path = dir.join("clip.ps1");
    write(&html_path, &wrap_cf_html(&payload.html))?;
    write(&text_path, &payload.text)?;
    let script = [
        "Add-Type -AssemblyName System.Windows.Forms".to_string(),
        format!(
            "$html = [System.IO.File]::ReadAllText({})",
            js_string(&html_path)
        ),
        format!(
            "$text = [System.IO.File]::ReadAllText({})",
            js_string(&text_path)
        ),
        "$data = New-Object System.Windows.Forms.DataObject".to_string(),
        "$data.SetData([System.Windows.Forms.DataFormats]::Html, $html)".to_string(),
        "$data.SetData([System.Windows.Forms.DataFormats]::UnicodeText, $text)".to_string(),
        "[System.Windows.Forms.Clipboard]::SetDataObject($data, $true)".to_string(),
    ]
    .join("\n");
    write(&script_path, &script)?;
    Ok(run(
        "powershell",
        &[
            "-NoProfile",
            "-STA",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
            &script_path.to_string_lossy(),
        ],
    ))
}

fn temp_dir() -> std::io::Result<PathBuf> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    let dir = std::env::temp_dir().join(format!("cortexcode-clip-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// `copyRichToClipboard`: copy `payload`, richly where the platform allows,
/// and say which flavour reached the clipboard.
pub fn copy_rich_to_clipboard(
    host: &dyn ClipboardHost,
    payload: &RichPayload,
) -> Result<CopyFlavour, String> {
    let oversized = payload.html.len() > MAX_RICH_BYTES;
    let platform = host.platform();
    if !oversized && matches!(platform, Platform::Darwin | Platform::Win32) {
        if let Ok(dir) = temp_dir() {
            let copied = if platform == Platform::Darwin {
                copy_rich_darwin(payload, &dir)
            } else {
                copy_rich_win32(payload, &dir)
            };
            let _ = std::fs::remove_dir_all(&dir);
            // A rich copy that failed still falls through to the text.
            if let Ok(true) = copied {
                return Ok(CopyFlavour::Rich);
            }
        }
    }
    copy_to_clipboard(host, &payload.text)?;
    Ok(CopyFlavour::Text)
}
