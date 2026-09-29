//! Reading an image off the clipboard, hoocode `utils/clipboard-image.ts`.
//!
//! Linux: Wayland and WSL try `wl-paste` then `xclip`; WSL then asks
//! PowerShell for the Windows clipboard (screenshots never reach the Linux
//! one); X11 uses the native clipboard. Elsewhere the native clipboard only.
//! Formats a model cannot take (BMP from WSLg) are re-encoded as PNG.
//! Everything that touches the machine goes through [`ClipboardImageHost`].

use std::time::Duration;

use crate::clipboard::{is_wayland_session, Platform};

/// `ClipboardImage`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipboardImage {
    pub bytes: Vec<u8>,
    pub mime_type: String,
}

/// The formats kept as they are, in order of preference.
const SUPPORTED_IMAGE_MIME_TYPES: [&str; 4] =
    ["image/png", "image/jpeg", "image/webp", "image/gif"];

pub const DEFAULT_LIST_TIMEOUT: Duration = Duration::from_millis(1000);
pub const DEFAULT_READ_TIMEOUT: Duration = Duration::from_millis(3000);
pub const DEFAULT_POWERSHELL_TIMEOUT: Duration = Duration::from_millis(5000);

/// What the read needs from the machine.
pub trait ClipboardImageHost {
    fn platform(&self) -> Platform;
    fn env(&self, name: &str) -> Option<String>;
    /// `spawnSync`: the command's stdout when it exited 0, else `None`.
    fn run(&self, command: &str, args: &[&str], timeout: Duration) -> Option<Vec<u8>>;
    /// `/proc/version`, for the WSL check.
    fn proc_version(&self) -> Option<String>;
    /// The native clipboard's image as PNG bytes; `None` when there is none.
    fn native_image(&self) -> Option<Vec<u8>>;
    /// A fresh temp file path for the PowerShell hand-off.
    fn temp_png_path(&self) -> std::path::PathBuf;
}

fn base_mime_type(mime_type: &str) -> String {
    mime_type
        .split(';')
        .next()
        .unwrap_or(mime_type)
        .trim()
        .to_lowercase()
}

/// `extensionForImageMimeType`.
pub fn extension_for_image_mime_type(mime_type: &str) -> Option<&'static str> {
    match base_mime_type(mime_type).as_str() {
        "image/png" => Some("png"),
        "image/jpeg" => Some("jpg"),
        "image/webp" => Some("webp"),
        "image/gif" => Some("gif"),
        _ => None,
    }
}

fn select_preferred_image_mime_type(mime_types: &[String]) -> Option<String> {
    let normalized: Vec<(&str, String)> = mime_types
        .iter()
        .map(|t| t.trim())
        .filter(|t| !t.is_empty())
        .map(|t| (t, base_mime_type(t)))
        .collect();
    for preferred in SUPPORTED_IMAGE_MIME_TYPES {
        if let Some((raw, _)) = normalized.iter().find(|(_, base)| base == preferred) {
            return Some(raw.to_string());
        }
    }
    normalized
        .iter()
        .find(|(_, base)| base.starts_with("image/"))
        .map(|(raw, _)| raw.to_string())
}

fn is_supported_image_mime_type(mime_type: &str) -> bool {
    SUPPORTED_IMAGE_MIME_TYPES.contains(&base_mime_type(mime_type).as_str())
}

fn lines(bytes: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(bytes)
        .split('\n')
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect()
}

/// Any decodable image as PNG (the pin's Photon conversion).
pub fn convert_to_png(bytes: &[u8]) -> Option<Vec<u8>> {
    let image = image::load_from_memory(bytes).ok()?;
    let mut out = std::io::Cursor::new(Vec::new());
    image.write_to(&mut out, image::ImageFormat::Png).ok()?;
    Some(out.into_inner())
}

/// Raw RGBA pixels (what native clipboards hand over) as PNG.
pub fn rgba_to_png(width: u32, height: u32, rgba: Vec<u8>) -> Option<Vec<u8>> {
    let image = image::RgbaImage::from_raw(width, height, rgba)?;
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(image)
        .write_to(&mut out, image::ImageFormat::Png)
        .ok()?;
    Some(out.into_inner())
}

fn via_wl_paste(host: &dyn ClipboardImageHost) -> Option<ClipboardImage> {
    let list = host.run("wl-paste", &["--list-types"], DEFAULT_LIST_TIMEOUT)?;
    let selected = select_preferred_image_mime_type(&lines(&list))?;
    let data = host.run(
        "wl-paste",
        &["--type", &selected, "--no-newline"],
        DEFAULT_READ_TIMEOUT,
    )?;
    (!data.is_empty()).then(|| ClipboardImage {
        bytes: data,
        mime_type: base_mime_type(&selected),
    })
}

fn is_wsl(host: &dyn ClipboardImageHost) -> bool {
    let set = |name: &str| host.env(name).is_some_and(|v| !v.is_empty());
    if set("WSL_DISTRO_NAME") || set("WSLENV") {
        return true;
    }
    host.proc_version().is_some_and(|release| {
        let release = release.to_lowercase();
        release.contains("microsoft") || release.contains("wsl")
    })
}

fn via_powershell(host: &dyn ClipboardImageHost) -> Option<ClipboardImage> {
    let tmp_file = host.temp_png_path();
    let result = (|| {
        let win_path = host.run(
            "wslpath",
            &["-w", &tmp_file.to_string_lossy()],
            DEFAULT_LIST_TIMEOUT,
        )?;
        let win_path = String::from_utf8_lossy(&win_path).trim().to_string();
        if win_path.is_empty() {
            return None;
        }
        let quoted = win_path.replace('\'', "''");
        let script = [
            "Add-Type -AssemblyName System.Windows.Forms".to_string(),
            "Add-Type -AssemblyName System.Drawing".to_string(),
            format!("$path = '{quoted}'"),
            "$img = [System.Windows.Forms.Clipboard]::GetImage()".to_string(),
            "if ($img) { $img.Save($path, [System.Drawing.Imaging.ImageFormat]::Png); Write-Output 'ok' } else { Write-Output 'empty' }".to_string(),
        ]
        .join("; ");
        let output = host.run(
            "powershell.exe",
            &["-NoProfile", "-Command", &script],
            DEFAULT_POWERSHELL_TIMEOUT,
        )?;
        if String::from_utf8_lossy(&output).trim() != "ok" {
            return None;
        }
        let bytes = std::fs::read(&tmp_file).ok()?;
        (!bytes.is_empty()).then(|| ClipboardImage {
            bytes,
            mime_type: "image/png".into(),
        })
    })();
    let _ = std::fs::remove_file(&tmp_file);
    result
}

fn via_xclip(host: &dyn ClipboardImageHost) -> Option<ClipboardImage> {
    let candidates = host
        .run(
            "xclip",
            &["-selection", "clipboard", "-t", "TARGETS", "-o"],
            DEFAULT_LIST_TIMEOUT,
        )
        .map(|out| lines(&out))
        .unwrap_or_default();
    let preferred = if candidates.is_empty() {
        None
    } else {
        select_preferred_image_mime_type(&candidates)
    };
    let try_types = preferred
        .into_iter()
        .chain(SUPPORTED_IMAGE_MIME_TYPES.iter().map(|t| t.to_string()));
    for mime_type in try_types {
        if let Some(data) = host.run(
            "xclip",
            &["-selection", "clipboard", "-t", &mime_type, "-o"],
            DEFAULT_READ_TIMEOUT,
        ) {
            if !data.is_empty() {
                return Some(ClipboardImage {
                    bytes: data,
                    mime_type: base_mime_type(&mime_type),
                });
            }
        }
    }
    None
}

fn via_native(host: &dyn ClipboardImageHost) -> Option<ClipboardImage> {
    let bytes = host.native_image()?;
    (!bytes.is_empty()).then(|| ClipboardImage {
        bytes,
        mime_type: "image/png".into(),
    })
}

/// `readClipboardImage`.
pub fn read_clipboard_image(host: &dyn ClipboardImageHost) -> Option<ClipboardImage> {
    if host.env("TERMUX_VERSION").is_some_and(|v| !v.is_empty()) {
        return None;
    }

    let image = if host.platform() == Platform::Linux {
        let wsl = is_wsl(host);
        let wayland = is_wayland_session(|name| host.env(name));
        let mut image = None;
        if wayland || wsl {
            image = via_wl_paste(host).or_else(|| via_xclip(host));
        }
        if image.is_none() && wsl {
            image = via_powershell(host);
        }
        if image.is_none() && !wayland {
            image = via_native(host);
        }
        image
    } else {
        via_native(host)
    }?;

    // Formats a model cannot take (BMP from WSLg) become PNG.
    if !is_supported_image_mime_type(&image.mime_type) {
        return Some(ClipboardImage {
            bytes: convert_to_png(&image.bytes)?,
            mime_type: "image/png".into(),
        });
    }
    Some(image)
}

/// The machine itself; `native` reads the platform clipboard's image as PNG,
/// supplied by the crate that owns the clipboard library.
pub struct SystemClipboardImageHost {
    pub native: Option<Box<dyn Fn() -> Option<Vec<u8>>>>,
}

impl ClipboardImageHost for SystemClipboardImageHost {
    fn platform(&self) -> Platform {
        Platform::current()
    }

    fn env(&self, name: &str) -> Option<String> {
        std::env::var(name).ok()
    }

    fn run(&self, command: &str, args: &[&str], timeout: Duration) -> Option<Vec<u8>> {
        run_with_timeout(command, args, timeout)
    }

    fn proc_version(&self) -> Option<String> {
        std::fs::read_to_string("/proc/version").ok()
    }

    fn native_image(&self) -> Option<Vec<u8>> {
        self.native.as_ref().and_then(|read| read())
    }

    fn temp_png_path(&self) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        std::env::temp_dir().join(format!(
            "cortexcode-wsl-clip-{}-{nanos}.png",
            std::process::id()
        ))
    }
}

/// Run to completion within `timeout`; stdout when it exited 0.
fn run_with_timeout(command: &str, args: &[&str], timeout: Duration) -> Option<Vec<u8>> {
    use std::io::Read as _;
    use std::process::{Command, Stdio};
    let mut child = Command::new(command)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf);
        buf
    });
    let started = std::time::Instant::now();
    let status = loop {
        match child.try_wait().ok()? {
            Some(status) => break status,
            None if started.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            None => std::thread::sleep(Duration::from_millis(10)),
        }
    };
    let out = reader.join().ok()?;
    status.success().then_some(out)
}
