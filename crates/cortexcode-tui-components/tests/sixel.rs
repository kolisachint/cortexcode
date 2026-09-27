//! Port of the pin's `test/sixel.test.ts`. Everything here touches process-wide
//! capability/rasterizer/env state, so it runs as one serialized test.

use cortexcode_tui_components::{Image, ImageOptions, ImageTheme};
use cortexcode_tui_images::*;
use cortexcode_tui_render::Component;

fn solid(width: u32, height: u32, rgba: [u8; 4]) -> RgbaImage {
    RgbaImage {
        data: rgba.repeat((width * height) as usize),
        width,
        height,
    }
}

struct Decoded {
    width: usize,
    height: usize,
    pixels: Vec<i32>,
    palette: Vec<Option<String>>,
}

fn decode_sixel(sequence: &str) -> Decoded {
    let rest = sequence
        .strip_prefix("\x1bP0;1;0q\"1;1;")
        .expect("raster attributes");
    let digits = |s: &str| s.bytes().take_while(u8::is_ascii_digit).count();
    let n = digits(rest);
    let width: usize = rest[..n].parse().unwrap();
    let rest = &rest[n + 1..];
    let n = digits(rest);
    let height: usize = rest[..n].parse().unwrap();
    assert!(sequence.ends_with("\x1b\\"));
    let body = &rest[n..rest.len() - 2];
    let b = body.as_bytes();
    let mut pixels = vec![-1i32; width * height];
    let mut palette: Vec<Option<String>> = Vec::new();
    let (mut color, mut x, mut top, mut i) = (0usize, 0usize, 0usize, 0usize);
    let num = |i: &mut usize| {
        let start = *i;
        while *i < b.len() && b[*i].is_ascii_digit() {
            *i += 1;
        }
        body[start..*i].parse::<usize>().unwrap()
    };
    while i < b.len() {
        match b[i] {
            b'#' => {
                i += 1;
                color = num(&mut i);
                if body[i..].starts_with(";2;") {
                    i += 3;
                    let r = num(&mut i);
                    i += 1;
                    let g = num(&mut i);
                    i += 1;
                    let bl = num(&mut i);
                    if palette.len() <= color {
                        palette.resize(color + 1, None);
                    }
                    palette[color] = Some(format!("{r},{g},{bl}"));
                }
            }
            b'$' => {
                x = 0;
                i += 1;
            }
            b'-' => {
                x = 0;
                top += 6;
                i += 1;
            }
            _ => {
                let mut run = 1;
                if b[i] == b'!' {
                    i += 1;
                    run = num(&mut i);
                }
                let bits = b[i] as i32 - 63;
                assert!((0..64).contains(&bits));
                for _ in 0..run {
                    for dy in 0..6 {
                        if bits & (1 << dy) != 0 && top + dy < height {
                            pixels[(top + dy) * width + x] = color as i32;
                        }
                    }
                    x += 1;
                }
                i += 1;
            }
        }
    }
    Decoded {
        width,
        height,
        pixels,
        palette,
    }
}

fn caps(images: Option<ImageProtocol>) -> TerminalCapabilities {
    TerminalCapabilities {
        images,
        true_color: true,
        hyperlinks: false,
    }
}

fn reset() {
    set_image_rasterizer(None);
    reset_capabilities_cache();
    set_cell_dimensions(CellDimensions {
        width_px: 9,
        height_px: 18,
    });
}

const ENV_KEYS: [&str; 10] = [
    "TERM",
    "TERM_PROGRAM",
    "COLORTERM",
    "TMUX",
    "KITTY_WINDOW_ID",
    "GHOSTTY_RESOURCES_DIR",
    "WEZTERM_PANE",
    "ITERM_SESSION_ID",
    "WT_SESSION",
    "CORTEX_IMAGE_PROTOCOL",
];

fn with_env(overrides: &[(&str, &str)], f: impl FnOnce()) {
    let saved: Vec<_> = ENV_KEYS
        .iter()
        .map(|k| (*k, std::env::var(k).ok()))
        .collect();
    for k in ENV_KEYS {
        std::env::remove_var(k);
    }
    for (k, v) in overrides {
        std::env::set_var(k, v);
    }
    f();
    for (k, v) in saved {
        match v {
            Some(v) => std::env::set_var(k, v),
            None => std::env::remove_var(k),
        }
    }
}

#[test]
fn sixel_suite() {
    // Windows Terminal detection
    with_env(&[("WT_SESSION", "7c3b4b6e-0000")], || {
        let c = detect_capabilities();
        assert_eq!(c.images, Some(ImageProtocol::Sixel));
        assert!(c.true_color);
    });
    with_env(
        &[("WT_SESSION", "x"), ("TMUX", "/tmp/tmux-1000/default,1,0")],
        || {
            assert_eq!(detect_capabilities().images, None);
        },
    );
    with_env(&[("CORTEX_IMAGE_PROTOCOL", "sixel")], || {
        assert_eq!(detect_capabilities().images, Some(ImageProtocol::Sixel));
    });
    with_env(
        &[("WT_SESSION", "x"), ("CORTEX_IMAGE_PROTOCOL", "none")],
        || {
            assert_eq!(detect_capabilities().images, None);
        },
    );
    with_env(
        &[
            ("TERM_PROGRAM", "ghostty"),
            ("CORTEX_IMAGE_PROTOCOL", "bogus"),
        ],
        || {
            assert_eq!(detect_capabilities().images, Some(ImageProtocol::Kitty));
        },
    );

    // encodeSixel: round trip with a partial last band
    let mut image = solid(5, 8, [255, 0, 0, 255]);
    let at = ((7 * 5 + 4) * 4) as usize;
    image.data[at..at + 4].copy_from_slice(&[0, 0, 255, 255]);
    image.data[0..4].copy_from_slice(&[0, 0, 0, 0]);
    let sequence = encode_sixel(&image, 256);
    assert!(is_image_line(&sequence));
    let d = decode_sixel(&sequence);
    assert_eq!((d.width, d.height), (5, 8));
    assert_eq!(d.pixels[0], -1);
    let red = d.pixels[1] as usize;
    let blue = d.pixels[7 * 5 + 4] as usize;
    assert_eq!(d.palette[red].as_deref(), Some("100,0,0"));
    assert_eq!(d.palette[blue].as_deref(), Some("0,0,100"));
    for p in 1..39 {
        assert_eq!(d.pixels[p], red as i32, "pixel {p}");
    }
    // run-length encoding
    assert!(encode_sixel(&solid(200, 6, [10, 20, 30, 255]), 256).contains("!200~"));
    // palette reduction
    let (w, h) = (64u32, 64u32);
    let mut data = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let o = ((y * w + x) * 4) as usize;
            data[o..o + 4].copy_from_slice(&[
                (x * 4) as u8,
                (y * 4) as u8,
                ((x ^ y) * 4) as u8,
                255,
            ]);
        }
    }
    let d = decode_sixel(&encode_sixel(
        &RgbaImage {
            data,
            width: w,
            height: h,
        },
        16,
    ));
    assert!(d.palette.len() <= 16);
    assert!(d.pixels.iter().all(|p| (0..16).contains(p)));

    // renderImage with sixel
    reset();
    set_capabilities(caps(Some(ImageProtocol::Sixel)));
    let png = ImageRenderOptions {
        mime_type: Some("image/png".into()),
        ..Default::default()
    };
    let dims = |w, h| ImageDimensions {
        width_px: w,
        height_px: h,
    };
    assert!(render_image("AA==", dims(100, 100), &png).is_none());

    set_cell_dimensions(CellDimensions {
        width_px: 10,
        height_px: 20,
    });
    let requested = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let r2 = requested.clone();
    set_image_rasterizer(Some(Box::new(move |_, _, w, h| {
        r2.lock().unwrap().push((w, h));
        Some(solid(w, h, [0, 128, 0, 255]))
    })));
    let opts = ImageRenderOptions {
        max_width_cells: Some(40),
        ..png.clone()
    };
    let big = render_image("x", dims(1000, 500), &opts).unwrap();
    assert_eq!(requested.lock().unwrap()[0], (400, 200));
    assert_eq!(big.rows, 10);
    let small = render_image("x", dims(30, 45), &opts).unwrap();
    assert_eq!(requested.lock().unwrap()[1], (30, 45));
    assert_eq!(small.rows, 3);

    // Image reserves its rows and restores the cursor around the sixel
    set_image_rasterizer(Some(Box::new(|_, _, w, h| {
        Some(solid(w, h, [0, 0, 0, 255]))
    })));
    let theme = || ImageTheme {
        fallback_color: Box::new(|s: &str| s.to_string()),
    };
    let mut component = Image::new(
        "x",
        "image/png",
        theme(),
        ImageOptions {
            max_width_cells: Some(40),
            ..Default::default()
        },
        Some(dims(400, 100)),
    );
    let lines = component.render(80);
    assert_eq!(lines.len(), 5);
    assert_eq!(&lines[..4], &["", "", "", ""]);
    assert!(
        lines[4].starts_with("\x1b[4A\x1b7\x1bP0;1;0q"),
        "{:?}",
        &lines[4][..20]
    );
    assert!(lines[4].ends_with("\x1b\\\x1b8\x1b[4B"));

    // text fallback when the rasterizer fails
    set_image_rasterizer(Some(Box::new(|_, _, _, _| panic!("corrupt"))));
    let mut failing = Image::new(
        "x",
        "image/png",
        theme(),
        ImageOptions {
            filename: Some("a.png".into()),
            ..Default::default()
        },
        Some(dims(10, 10)),
    );
    assert_eq!(
        failing.render(80),
        vec!["[Image: a.png [image/png] 10x10]".to_string()]
    );
    reset();
}
