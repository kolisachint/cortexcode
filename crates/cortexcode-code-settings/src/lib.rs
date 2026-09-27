//! Settings for the cortex coding agent: hoocode
//! `core/settings-{types,defaults,storage,manager}.ts` (v0.5.89).
//!
//! Settings stay the raw `settings.json` object, so keys this version does not
//! know (newer hoocode or cortex releases, the webtools binary's own block)
//! pass through reads and writes unchanged. [`SettingsManager`]'s getters
//! apply the defaults and the range checks and return typed values.

mod manager;
mod storage;
mod types;

pub use manager::{deep_merge_settings, migrate_settings, SettingsManager};
pub use storage::{
    Error, FileSettingsStorage, InMemorySettingsStorage, LockFn, SettingsError, SettingsScope,
    SettingsStorage,
};
pub use types::*;
