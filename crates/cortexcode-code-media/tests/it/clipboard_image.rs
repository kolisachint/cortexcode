//! Port of `test/clipboard-image.test.ts` and
//! `test/clipboard-image-bmp-conversion.test.ts`, with a scripted host in
//! place of the mocked `spawnSync` and native clipboard.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use cortexcode_code_media::clipboard::Platform;
use cortexcode_code_media::clipboard_image::{read_clipboard_image, ClipboardImageHost};

type Run = Box<dyn Fn(&Host, &str, &[&str]) -> Option<Vec<u8>>>;

struct Host {
    env: HashMap<String, String>,
    run: Run,
    /// `None`: the native clipboard must not be asked.
    native: Option<Option<Vec<u8>>>,
    tmp: PathBuf,
    calls: RefCell<Vec<String>>,
}

impl Host {
    fn new(env: &[(&str, &str)], run: Run) -> Self {
        let tmp = std::env::temp_dir().join(format!(
            "cortexcode-clip-test-{}-{:?}.png",
            std::process::id(),
            std::thread::current().id()
        ));
        Self {
            env: env
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            run,
            native: None,
            tmp,
            calls: RefCell::new(Vec::new()),
        }
    }
}

impl ClipboardImageHost for Host {
    fn platform(&self) -> Platform {
        Platform::Linux
    }
    fn env(&self, name: &str) -> Option<String> {
        self.env.get(name).cloned()
    }
    fn run(&self, command: &str, args: &[&str], _timeout: Duration) -> Option<Vec<u8>> {
        self.calls
            .borrow_mut()
            .push(format!("{command} {}", args.join(" ")));
        (self.run)(self, command, args)
    }
    fn proc_version(&self) -> Option<String> {
        Some("Linux version 6.0 (gcc)".into())
    }
    fn native_image(&self) -> Option<Vec<u8>> {
        self.native
            .clone()
            .expect("the native clipboard should not be asked here")
    }
    fn temp_png_path(&self) -> PathBuf {
        self.tmp.clone()
    }
}

#[test]
fn wayland_uses_wl_paste_and_never_the_native_clipboard() {
    let host = Host::new(
        &[("WAYLAND_DISPLAY", "1")],
        Box::new(|_, command, args| match (command, args[0]) {
            ("wl-paste", "--list-types") => Some(b"text/plain\nimage/png\n".to_vec()),
            ("wl-paste", "--type") => Some(vec![1, 2, 3]),
            _ => panic!("unexpected call: {command} {}", args.join(" ")),
        }),
    );
    let image = read_clipboard_image(&host).unwrap();
    assert_eq!(image.mime_type, "image/png");
    assert_eq!(image.bytes, vec![1, 2, 3]);
}

#[test]
fn wayland_falls_back_to_xclip_when_wl_paste_is_missing() {
    let host = Host::new(
        &[("XDG_SESSION_TYPE", "wayland")],
        Box::new(|_, command, args| {
            if command == "wl-paste" {
                return None;
            }
            if command == "xclip" && args.contains(&"TARGETS") {
                return Some(b"image/png\n".to_vec());
            }
            if command == "xclip" && args.contains(&"image/png") {
                return Some(vec![9, 8]);
            }
            Some(Vec::new())
        }),
    );
    let image = read_clipboard_image(&host).unwrap();
    assert_eq!(image.mime_type, "image/png");
    assert_eq!(image.bytes, vec![9, 8]);
}

#[test]
fn wsl_passes_the_powershell_path_directly() {
    let host = Host::new(
        &[("WSL_DISTRO_NAME", "Ubuntu")],
        Box::new(|host, command, args| match command {
            "wl-paste" | "xclip" => Some(Vec::new()),
            "wslpath" => {
                assert_eq!(PathBuf::from(args[1]), host.tmp);
                Some(b"C:\\Users\\O'Hare\\clip.png\n".to_vec())
            }
            "powershell.exe" => {
                assert!(args[2].contains("$path = 'C:\\Users\\O''Hare\\clip.png'"));
                std::fs::write(&host.tmp, [4, 5, 6]).unwrap();
                Some(b"ok\n".to_vec())
            }
            _ => panic!("unexpected call: {command}"),
        }),
    );
    let image = read_clipboard_image(&host).unwrap();
    assert_eq!(image.mime_type, "image/png");
    assert_eq!(image.bytes, vec![4, 5, 6]);
    assert!(!host.tmp.exists(), "the hand-off file is removed");
}

#[test]
fn non_wayland_uses_the_native_clipboard() {
    let mut host = Host::new(
        &[],
        Box::new(|_, command, _| panic!("no command expected, got {command}")),
    );
    host.native = Some(Some(vec![7]));
    let image = read_clipboard_image(&host).unwrap();
    assert_eq!(image.mime_type, "image/png");
    assert_eq!(image.bytes, vec![7]);
}

#[test]
fn non_wayland_returns_none_when_the_clipboard_has_no_image() {
    let mut host = Host::new(
        &[],
        Box::new(|_, command, _| panic!("no command expected, got {command}")),
    );
    host.native = Some(None);
    assert!(read_clipboard_image(&host).is_none());
    assert!(host.calls.borrow().is_empty());
}

fn tiny_bmp_1x1_red_24bpp() -> Vec<u8> {
    let mut b = vec![0u8; 58];
    b[0..2].copy_from_slice(b"BM");
    b[2..6].copy_from_slice(&58u32.to_le_bytes());
    b[10..14].copy_from_slice(&54u32.to_le_bytes());
    b[14..18].copy_from_slice(&40u32.to_le_bytes());
    b[18..22].copy_from_slice(&1i32.to_le_bytes());
    b[22..26].copy_from_slice(&1i32.to_le_bytes());
    b[26..28].copy_from_slice(&1u16.to_le_bytes());
    b[28..30].copy_from_slice(&24u16.to_le_bytes());
    b[34..38].copy_from_slice(&4u32.to_le_bytes());
    b[56] = 0xff;
    b
}

#[test]
fn converts_bmp_to_png_on_wayland() {
    let host = Host::new(
        &[("WAYLAND_DISPLAY", "wayland-0")],
        Box::new(|_, command, args| {
            if command == "wl-paste" && args.contains(&"--list-types") {
                return Some(b"image/bmp\n".to_vec());
            }
            if command == "wl-paste" && args.contains(&"image/bmp") {
                return Some(tiny_bmp_1x1_red_24bpp());
            }
            None
        }),
    );
    let image = read_clipboard_image(&host).unwrap();
    assert_eq!(image.mime_type, "image/png");
    assert_eq!(&image.bytes[..4], &[0x89, 0x50, 0x4e, 0x47]);
}
