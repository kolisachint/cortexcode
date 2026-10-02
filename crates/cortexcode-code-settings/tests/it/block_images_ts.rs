//! Port of the `SettingsManager` cases in hoocode `test/block-images.test.ts`
//! (v0.5.89). Its read tool cases are in `cortexcode-code-tools-fs`, its
//! `processFileArguments` cases in `cortexcode-code-cli/src/initial_message.rs`.

use cortexcode_code_settings::SettingsManager;
use serde_json::{json, Value};

fn in_memory(v: Value) -> SettingsManager {
    SettingsManager::in_memory(v.as_object().unwrap().clone())
}

#[test]
fn should_default_block_images_to_false() {
    assert!(!in_memory(json!({})).block_images());
}

#[test]
fn should_return_true_when_block_images_is_set_to_true() {
    assert!(in_memory(json!({ "images": { "blockImages": true } })).block_images());
}

#[test]
fn should_persist_block_images_setting_via_set_block_images() {
    let mut manager = in_memory(json!({}));
    assert!(!manager.block_images());
    manager.set_block_images(true);
    assert!(manager.block_images());
    manager.set_block_images(false);
    assert!(!manager.block_images());
}

#[test]
fn should_handle_block_images_alongside_auto_resize() {
    let manager = in_memory(json!({ "images": { "autoResize": true, "blockImages": true } }));
    assert!(manager.image_auto_resize());
    assert!(manager.block_images());
}
