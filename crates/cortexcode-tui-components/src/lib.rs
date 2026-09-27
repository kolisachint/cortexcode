//! UI components for the cortex TUI.
//!
//! Ported from TypeScript `@kolisachint/hoocode-tui`'s `components/*.ts`
//! (plus `autocomplete.ts` and `editor-component.ts`, which live at the
//! package root but are tightly coupled to the `Editor` component).

mod autocomplete;
mod box_component;
mod cancellable_loader;
mod color;
mod editor;
mod frame;
mod image;
mod input;
mod loader;
mod markdown;
mod select_list;
mod settings_list;
mod spacer;
mod text;
mod truncated_text;

pub use autocomplete::{
    ApplyCompletionResult, AutocompleteItem, AutocompleteProvider, AutocompleteSuggestions,
    CombinedAutocompleteProvider, CommandEntry, SlashCommand,
};
pub use box_component::{BoxComponent, PaperFn, PaperSheet};
pub use cancellable_loader::{AbortSignal, CancellableLoader};
pub use color::{identity_color, ColorFn};
pub use cortexcode_tui_render::FlexSpacer;
pub use editor::{word_wrap_line, Editor, EditorOptions, EditorTheme, TextChunk};
pub use frame::{
    render_frame_edge, Frame, FrameBorderChars, FrameBorderCharsOverride, FrameBorderStyle,
    FrameEdge, FrameEdgeOptions, FrameLabel, FrameOptions, LABEL_RIGHT_INSET, MIN_LABEL_LEAD_IN,
};
pub use image::{Image, ImageOptions, ImageTheme};
pub use input::{Input, DEFAULT_INPUT_PROMPT};
pub use loader::{Loader, LoaderIndicatorOptions};
pub use markdown::{DefaultTextStyle, HeadingFn, HighlightCodeFn, Markdown, MarkdownTheme};
pub use select_list::{
    SelectItem, SelectList, SelectListLayoutOptions, SelectListTheme,
    SelectListTruncatePrimaryContext,
};
pub use settings_list::{
    SettingItem, SettingsList, SettingsListOptions, SettingsListTheme, SubmenuFactory,
    SubmenuOutcome,
};
pub use spacer::Spacer;
pub use text::Text;
pub use truncated_text::TruncatedText;
