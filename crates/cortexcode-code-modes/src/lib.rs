//! The mode system of hoocode's built-in `hoo-core` extension, ported natively:
//! ask / plan / build / debug prompts, `hoo-config.json`, and the `/mode`,
//! `/plan`, `/grill`, `/goal`, `/approve` commands.
//!
//! Ports `extensions/core/{modes,config}.ts` and `core/mode-prompts.ts`
//! (hoocode v0.5.89).

pub mod config;
pub mod extension;
pub mod plan;
pub mod prompts;

pub use extension::{
    append_mode_prompt, build_mode_system_prompt, resolve_active_mode, ActiveMode, ModeAction,
    ModeSession, ModesExtension, NotifyLevel, KNOWN_MODES, MODE_COMMANDS,
};
pub use plan::{
    build_approve_message, build_goal_messages, build_grill_message, parse_goal_args,
    parse_grill_target, parse_plan_sections, GoalInvocation, GoalMessages, GrillTarget,
    PlanSections, AUTO_LOOP_DONE_TOKEN,
};
pub use prompts::{DEFAULT_MODE, DEFAULT_MODE_PROMPTS};
