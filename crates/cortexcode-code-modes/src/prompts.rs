//! `core/mode-prompts.ts` + the embedded `/grill` prompts: hoocode's
//! `templates/modes/<mode>/system.md` and `templates/prompts/grill-*.md`, verbatim.

/// `DEFAULT_MODE`.
pub const DEFAULT_MODE: &str = "build";

/// `DEFAULT_MODE_PROMPTS`: the plan prompt carries a `{{PLAN_PATH}}` token.
pub const DEFAULT_MODE_PROMPTS: &[(&str, &str)] = &[
    ("ask", include_str!("../templates/modes/ask/system.md")),
    ("build", include_str!("../templates/modes/build/system.md")),
    ("debug", include_str!("../templates/modes/debug/system.md")),
    ("plan", include_str!("../templates/modes/plan/system.md")),
];

/// The built-in prompt for a mode.
pub fn default_mode_prompt(mode: &str) -> Option<&'static str> {
    DEFAULT_MODE_PROMPTS
        .iter()
        .find(|(name, _)| *name == mode)
        .map(|(_, p)| *p)
}

/// `EMBEDDED_PROMPTS[name]` for the grill phases.
pub fn grill_prompt(name: &str) -> &'static str {
    match name {
        "grill-me" => include_str!("../templates/prompts/grill-me.md"),
        "grill-plan" => include_str!("../templates/prompts/grill-plan.md"),
        "grill-bridge" => include_str!("../templates/prompts/grill-bridge.md"),
        _ => "",
    }
}
