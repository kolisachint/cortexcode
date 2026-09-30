//! The `Image` component case of the pin's `test/terminal-image.test.ts`
//! ("restores the cursor to the reserved image row after Kitty rendering").

use cortexcode_tui_components::{Image, ImageOptions, ImageTheme};
use cortexcode_tui_images::{
    reset_capabilities_cache, set_capabilities, set_cell_dimensions, CellDimensions,
    ImageDimensions, ImageProtocol, TerminalCapabilities,
};
use cortexcode_tui_render::Component;

#[test]
fn restores_the_cursor_to_the_reserved_image_row_after_kitty_rendering() {
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
        ImageTheme {
            fallback_color: Box::new(|s| s.to_string()),
        },
        ImageOptions {
            max_width_cells: Some(2),
            ..Default::default()
        },
        Some(ImageDimensions {
            width_px: 20,
            height_px: 20,
        }),
    );
    let lines = image.render(4);
    let id = image.image_id().expect("kitty image id");
    assert_eq!(lines[..lines.len() - 1], [""]);
    assert!(lines[1].starts_with("\x1b[1A\x1b_G"), "{:?}", lines[1]);
    assert!(lines[1].contains(",C=1,"));
    assert!(lines[1].contains(&format!(",i={id}")));
    assert!(lines[1].ends_with("\x1b[1B"));
    reset_capabilities_cache();
    set_cell_dimensions(CellDimensions::default());
}
