#[allow(clippy::module_inception)]
mod editor;
mod word_wrap;

pub use editor::{Editor, EditorOptions, EditorTheme};
pub use word_wrap::{word_wrap_line, TextChunk};
