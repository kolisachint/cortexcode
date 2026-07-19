//! Shared color-function type used across components in place of
//! TypeScript's `(text: string) => string` (usually a `chalk` style function).

pub type ColorFn = Box<dyn Fn(&str) -> String>;

/// A `ColorFn` that returns its input unchanged.
pub fn identity_color() -> ColorFn {
    Box::new(|s: &str| s.to_string())
}
