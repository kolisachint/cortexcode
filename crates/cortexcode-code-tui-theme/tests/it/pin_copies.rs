//! The bundled themes are copies of the pinned hoocode's
//! `coding-agent/src/modes/interactive/theme/*.json`. When the pinned checkout
//! is present, check them byte for byte so a pin bump can't leave them stale.

use std::path::Path;

#[test]
fn bundled_themes_match_the_pin() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let pin =
        manifest.join("../../target/hoocode-pin/packages/coding-agent/src/modes/interactive/theme");
    if !pin.is_dir() {
        eprintln!("pinned hoocode not checked out: skipping the copy check");
        return;
    }
    let mut stale = Vec::new();
    for entry in std::fs::read_dir(manifest.join("themes")).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let name = path.file_name().unwrap();
        let ours = std::fs::read(&path).unwrap();
        match std::fs::read(pin.join(name)) {
            Ok(theirs) if theirs == ours => {}
            _ => stale.push(name.to_string_lossy().into_owned()),
        }
    }
    assert!(
        stale.is_empty(),
        "themes differ from the pin (copy them from {}): {stale:?}",
        pin.display()
    );
}
