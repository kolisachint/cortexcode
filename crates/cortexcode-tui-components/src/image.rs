//! Inline image component, ported from `components/image.ts`.

use cortexcode_tui_images::{
    allocate_image_id, get_capabilities, get_image_dimensions, image_fallback, render_image,
    ImageDimensions, ImageProtocol, ImageRenderOptions,
};
use cortexcode_tui_render::Component;

use crate::color::ColorFn;

pub struct ImageTheme {
    pub fallback_color: ColorFn,
}

#[derive(Debug, Clone, Default)]
pub struct ImageOptions {
    pub max_width_cells: Option<u32>,
    pub max_height_cells: Option<u32>,
    pub filename: Option<String>,
    /// Kitty image ID. If provided, reuses this ID (for animations/updates).
    pub image_id: Option<u32>,
}

pub struct Image {
    base64_data: String,
    mime_type: String,
    dimensions: ImageDimensions,
    theme: ImageTheme,
    options: ImageOptions,
    image_id: Option<u32>,
}

impl Image {
    pub fn new(
        base64_data: impl Into<String>,
        mime_type: impl Into<String>,
        theme: ImageTheme,
        options: ImageOptions,
        dimensions: Option<ImageDimensions>,
    ) -> Self {
        let base64_data = base64_data.into();
        let mime_type = mime_type.into();
        let dimensions = dimensions
            .or_else(|| get_image_dimensions(&base64_data, &mime_type))
            .unwrap_or(ImageDimensions {
                width_px: 800,
                height_px: 600,
            });
        let image_id = options.image_id;
        Self {
            base64_data,
            mime_type,
            dimensions,
            theme,
            options,
            image_id,
        }
    }

    /// Get the Kitty image ID used by this image (if any).
    pub fn image_id(&self) -> Option<u32> {
        self.image_id
    }
}

impl Component for Image {
    fn render(&mut self, width: u16) -> Vec<String> {
        let max_width = (width as u32)
            .saturating_sub(2)
            .min(self.options.max_width_cells.unwrap_or(60));

        let caps = get_capabilities();

        let Some(protocol) = caps.images else {
            let fallback = image_fallback(
                &self.mime_type,
                Some(self.dimensions),
                self.options.filename.as_deref(),
            );
            return vec![(self.theme.fallback_color)(&fallback)];
        };

        if protocol == ImageProtocol::Kitty && self.image_id.is_none() {
            self.image_id = Some(allocate_image_id());
        }

        let result = render_image(
            &self.base64_data,
            self.dimensions,
            &ImageRenderOptions {
                max_width_cells: Some(max_width),
                image_id: self.image_id,
                move_cursor: Some(false),
                ..Default::default()
            },
        );

        let Some(result) = result else {
            let fallback = image_fallback(
                &self.mime_type,
                Some(self.dimensions),
                self.options.filename.as_deref(),
            );
            return vec![(self.theme.fallback_color)(&fallback)];
        };

        if let Some(id) = result.image_id {
            self.image_id = Some(id);
        }

        // Return `rows` lines so the TUI accounts for image height. The
        // first (rows-1) lines are empty and get cleared before the image
        // is drawn; the last line moves the cursor back up, draws the
        // image, then (for Kitty, since terminal-side cursor movement is
        // disabled above) moves back down so TUI cursor accounting stays
        // inside the scroll area.
        let mut lines = vec![String::new(); result.rows.saturating_sub(1) as usize];
        let row_offset = result.rows.saturating_sub(1);
        let move_up = if row_offset > 0 {
            format!("\x1b[{row_offset}A")
        } else {
            String::new()
        };
        let move_down = if protocol == ImageProtocol::Kitty && row_offset > 0 {
            format!("\x1b[{row_offset}B")
        } else {
            String::new()
        };
        lines.push(format!("{move_up}{}{move_down}", result.sequence));
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cortexcode_tui_images::{
        reset_capabilities_cache, set_capabilities, set_cell_dimensions, CellDimensions,
        TerminalCapabilities,
    };
    use std::sync::Mutex;

    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn theme() -> ImageTheme {
        ImageTheme {
            fallback_color: Box::new(|s: &str| s.to_string()),
        }
    }

    #[test]
    fn falls_back_to_text_without_image_support() {
        let _guard = TEST_LOCK.lock().unwrap();
        set_capabilities(TerminalCapabilities {
            images: None,
            true_color: false,
            hyperlinks: false,
        });
        let mut image = Image::new(
            "AAAA",
            "image/png",
            theme(),
            ImageOptions::default(),
            Some(ImageDimensions {
                width_px: 20,
                height_px: 20,
            }),
        );
        let lines = image.render(20);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("image/png"));
        reset_capabilities_cache();
    }

    #[test]
    fn restores_cursor_after_kitty_rendering() {
        let _guard = TEST_LOCK.lock().unwrap();
        set_capabilities(TerminalCapabilities {
            images: Some(ImageProtocol::Kitty),
            true_color: true,
            hyperlinks: true,
        });
        set_cell_dimensions(CellDimensions {
            width_px: 10,
            height_px: 10,
        });
        let mut image = Image::new(
            "AAAA",
            "image/png",
            theme(),
            ImageOptions::default(),
            Some(ImageDimensions {
                width_px: 20,
                height_px: 20,
            }),
        );
        let lines = image.render(4);
        let image_id = image.image_id();
        assert!(image_id.is_some());
        assert_eq!(&lines[..lines.len() - 1], &[String::new()]);
        assert!(lines[1].starts_with("\x1b[1A\x1b_G"));
        assert!(lines[1].contains(",C=1,"));
        assert!(lines[1].contains(&format!(",i={}", image_id.unwrap())));
        assert!(lines[1].ends_with("\x1b[1B"));
        reset_capabilities_cache();
        set_cell_dimensions(CellDimensions::default());
    }
}
