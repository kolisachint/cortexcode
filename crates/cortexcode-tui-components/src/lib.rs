//! UI components for the cortex TUI.
//!
//! Ported from TypeScript `@kolisachint/hoocode-tui`'s `components/*.ts`
//! (plus `autocomplete.ts` and `editor-component.ts`, which live at the
//! package root but are tightly coupled to the `Editor` component).

mod autocomplete;
mod box_component;
mod cancellable_loader;
mod color;
mod image;
mod input;
mod loader;
mod select_list;
mod spacer;
mod text;
mod truncated_text;

pub use autocomplete::{
    ApplyCompletionResult, AutocompleteItem, AutocompleteProvider, AutocompleteSuggestions,
    CombinedAutocompleteProvider, CommandEntry, SlashCommand,
};
pub use box_component::BoxComponent;
pub use cancellable_loader::{AbortSignal, CancellableLoader};
pub use color::{identity_color, ColorFn};
pub use image::{Image, ImageOptions, ImageTheme};
pub use input::Input;
pub use loader::{Loader, LoaderIndicatorOptions};
pub use select_list::{
    SelectItem, SelectList, SelectListLayoutOptions, SelectListTheme,
    SelectListTruncatePrimaryContext,
};
pub use spacer::Spacer;
pub use text::Text;
pub use truncated_text::TruncatedText;
