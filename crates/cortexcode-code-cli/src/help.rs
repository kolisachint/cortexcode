//! `printHelp` from hoocode `cli/args.ts`. The text itself is generated into
//! `help_text.rs` by `migration/tools/gen_help_text.py`.

use crate::help_text::HELP_SEGMENTS;
use std::io::Write;

/// One piece of the help template.
pub(crate) enum Seg {
    Plain(&'static str),
    /// `chalk.bold(...)`.
    Bold(&'static str),
    /// The `extensionFlagsText` slot (empty until extensions are ported).
    Ext,
}

/// Render the help text. `color` mirrors chalk: bold is emitted only when the
/// output supports color.
pub fn render_help(color: bool) -> String {
    let mut out = String::new();
    for seg in HELP_SEGMENTS {
        match seg {
            Seg::Plain(text) => out.push_str(text),
            Seg::Bold(text) if color => {
                out.push_str("\x1b[1m");
                out.push_str(text);
                out.push_str("\x1b[22m");
            }
            Seg::Bold(text) => out.push_str(text),
            Seg::Ext => {}
        }
    }
    out
}

/// `printHelp()`: `console.log` of the rendered text.
pub fn print_help(output: &mut dyn Write, color: bool) -> std::io::Result<()> {
    writeln!(output, "{}", render_help(color))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_lists_pinned_flags_with_cortex_branding() {
        let help = render_help(false);
        assert!(help.starts_with("cortex - AI coding assistant with read, bash, edit, write tools\n\nUsage:\n  cortex [options] [@files...] [messages...]\n"));
        for flag in [
            "--provider <name>",
            "--no-tools, -nt",
            "--no-slash-commands, -nsc",
            "--list-models [search]",
            "--offline",
            "--use-system-ca",
        ] {
            assert!(help.contains(flag), "missing {flag}");
        }
        assert!(!help.contains("hoocode"));
        assert!(help.contains(
            "CORTEX_CODING_AGENT_DIR          - Config directory (default: ~/.cortexcode/agent)"
        ));
        assert!(help.ends_with("  search - Ranked code search, keyword + semantic (read-only)\n"));
    }

    #[test]
    fn bold_headers_only_with_color() {
        assert!(render_help(true).contains("\x1b[1mOptions:\x1b[22m"));
        assert!(!render_help(false).contains('\x1b'));
    }
}
