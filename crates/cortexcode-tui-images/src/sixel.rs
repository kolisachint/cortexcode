//! Sixel graphics (DEC VT340; Windows Terminal, foot, mlterm, xterm, WezTerm,
//! Konsole): the encoder and the host-registered rasterizer from
//! `terminal-image.ts`.

use once_cell::sync::Lazy;
use std::collections::BTreeMap;
use std::sync::Mutex;

/// DCS with the parameters [`encode_sixel`] always writes: square pixels,
/// transparent background.
pub const SIXEL_PREFIX: &str = "\x1bP0;1;0q";

/// Decoded pixels, 4 bytes (RGBA) per pixel, row-major.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RgbaImage {
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// Decodes an encoded image and scales it to exactly `width_px` x `height_px`.
/// Sixel carries pixels, so the host registers a decoder; until it does, a
/// sixel terminal gets the text fallback.
pub type ImageRasterizer =
    Box<dyn Fn(&str, &str, u32, u32) -> Option<RgbaImage> + Send + Sync + 'static>;

static RASTERIZER: Lazy<Mutex<Option<ImageRasterizer>>> = Lazy::new(|| Mutex::new(None));

pub fn set_image_rasterizer(rasterizer: Option<ImageRasterizer>) {
    *RASTERIZER.lock().unwrap() = rasterizer;
}

pub fn has_image_rasterizer() -> bool {
    RASTERIZER.lock().unwrap().is_some()
}

pub(crate) fn rasterize(
    base64_data: &str,
    mime_type: &str,
    width_px: u32,
    height_px: u32,
) -> Option<RgbaImage> {
    let guard = RASTERIZER.lock().unwrap();
    let rasterize = guard.as_ref()?;
    // A panicking decoder is a failed decode, as a throw is in hoocode.
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rasterize(base64_data, mime_type, width_px, height_px)
    }))
    .ok()
    .flatten()
}

fn key_of(data: &[u8], offset: usize) -> usize {
    (((data[offset] >> 3) as usize) << 10)
        | (((data[offset + 1] >> 3) as usize) << 5)
        | ((data[offset + 2] >> 3) as usize)
}

fn channel(key: usize, c: usize) -> usize {
    (key >> (10 - c * 5)) & 31
}

struct PaletteBox {
    keys: Vec<usize>,
    channel: usize,
    range: i64,
}

fn measure(keys: Vec<usize>) -> PaletteBox {
    let mut best = 0;
    let mut best_range: i64 = -1;
    for c in 0..3 {
        let (mut lo, mut hi) = (31usize, 0usize);
        for &key in &keys {
            let v = channel(key, c);
            lo = lo.min(v);
            hi = hi.max(v);
        }
        let range = hi as i64 - lo as i64;
        if range > best_range {
            best_range = range;
            best = c;
        }
    }
    PaletteBox {
        keys,
        channel: best,
        range: best_range,
    }
}

/// JS `Math.round` for non-negative values.
fn round(x: f64) -> u32 {
    (x + 0.5).floor() as u32
}

/// Median cut over a 15-bit histogram of the opaque pixels (`buildSixelPalette`).
fn build_sixel_palette(data: &[u8], max_colors: usize) -> (Vec<u32>, Vec<u16>) {
    let mut counts = vec![0u32; 32768];
    for offset in (0..data.len().saturating_sub(3)).step_by(4) {
        if data[offset + 3] < 128 {
            continue;
        }
        counts[key_of(data, offset)] += 1;
    }
    let keys: Vec<usize> = (0..counts.len()).filter(|&k| counts[k] > 0).collect();

    let mut boxes: Vec<PaletteBox> = if keys.is_empty() {
        Vec::new()
    } else {
        vec![measure(keys)]
    };
    while boxes.len() < max_colors {
        // Split the box spanning the widest range; boxes of one key cannot.
        let mut target: Option<usize> = None;
        for (i, b) in boxes.iter().enumerate() {
            if b.keys.len() > 1 && target.is_none_or(|t| b.range > boxes[t].range) {
                target = Some(i);
            }
        }
        let Some(target) = target else { break };
        let mut b = boxes.remove(target);
        let c = b.channel;
        // JS sort is stable, as is `sort_by_key`.
        b.keys.sort_by_key(|&k| channel(k, c));
        let total: u64 = b.keys.iter().map(|&k| counts[k] as u64).sum();
        let mut seen = 0u64;
        let mut split = 1;
        for i in 0..b.keys.len() - 1 {
            seen += counts[b.keys[i]] as u64;
            split = i + 1;
            if seen * 2 >= total {
                break;
            }
        }
        let right = b.keys.split_off(split);
        boxes.insert(target, measure(right));
        boxes.insert(target, measure(b.keys));
    }

    let mut palette = Vec::with_capacity(boxes.len() * 3);
    let mut lookup = vec![0u16; 32768];
    let expand = |v: usize| ((v << 3) | (v >> 2)) as f64;
    for (index, b) in boxes.iter().enumerate() {
        let (mut r, mut g, mut bl, mut n) = (0f64, 0f64, 0f64, 0f64);
        for &key in &b.keys {
            let w = counts[key] as f64;
            r += expand(channel(key, 0)) * w;
            g += expand(channel(key, 1)) * w;
            bl += expand(channel(key, 2)) * w;
            n += w;
            lookup[key] = index as u16;
        }
        palette.extend([round(r / n), round(g / n), round(bl / n)]);
    }
    (palette, lookup)
}

/// One sixel row of one colour: 6-bit column masks, run-length encoded.
fn encode_sixel_row(masks: &[u8]) -> String {
    let mut end = masks.len();
    while end > 0 && masks[end - 1] == 0 {
        end -= 1;
    }
    let mut out = String::new();
    let mut i = 0;
    while i < end {
        let value = masks[i];
        let mut run = 1;
        while i + run < end && masks[i + run] == value {
            run += 1;
        }
        let ch = char::from(63 + value);
        if run > 3 {
            out.push_str(&format!("!{run}{ch}"));
        } else {
            out.extend(std::iter::repeat_n(ch, run));
        }
        i += run;
    }
    out
}

/// Encode RGBA pixels as a Sixel image. Pixels with alpha below 128 are left
/// undrawn; at most `max_colors` palette registers are used (256 is what
/// Windows Terminal and xterm provide).
pub fn encode_sixel(image: &RgbaImage, max_colors: usize) -> String {
    let (width, height) = (image.width as usize, image.height as usize);
    let data = &image.data;
    let (palette, lookup) = build_sixel_palette(data, max_colors);

    let mut parts = vec![format!("{SIXEL_PREFIX}\"1;1;{width};{height}")];
    let pct = |v: u32| round(v as f64 * 100.0 / 255.0);
    for (i, rgb) in palette.chunks(3).enumerate() {
        parts.push(format!(
            "#{i};2;{};{};{}",
            pct(rgb[0]),
            pct(rgb[1]),
            pct(rgb[2])
        ));
    }

    let mut top = 0;
    while top < height {
        let band_height = 6.min(height - top);
        // Insertion order, like a JS Map.
        let mut order: Vec<u16> = Vec::new();
        let mut bands: BTreeMap<u16, Vec<u8>> = BTreeMap::new();
        for dy in 0..band_height {
            let bit = 1u8 << dy;
            let mut offset = (top + dy) * width * 4;
            for x in 0..width {
                if data[offset + 3] >= 128 {
                    let index = lookup[key_of(data, offset)];
                    let masks = bands.entry(index).or_insert_with(|| {
                        order.push(index);
                        vec![0u8; width]
                    });
                    masks[x] |= bit;
                }
                offset += 4;
            }
        }
        let rows: Vec<String> = order
            .iter()
            .map(|index| format!("#{index}{}", encode_sixel_row(&bands[index])))
            .collect();
        // `$` returns to the band start for the next colour; `-` moves down.
        parts.push(format!("{}-", rows.join("$")));
        top += 6;
    }

    parts.push("\x1b\\".to_string());
    parts.concat()
}
