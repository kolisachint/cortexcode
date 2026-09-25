//! Model catalog data, generated from the pinned hoocode by
//! `scripts/convert_models_to_json.py`. Volatile by design: every pin bump
//! regenerates these files and nothing else.

/// `MODELS` from `models.generated.ts`: a JSON array of hoocode `Model`
/// objects, grouped by provider in hoocode's order.
pub const MODELS_JSON: &str = include_str!("../data/models.json");

/// `IMAGE_MODELS` from `image-models.generated.ts`, in the same shape.
pub const IMAGE_MODELS_JSON: &str = include_str!("../data/image-models.json");

/// `{"hoocodeVersion", "hoocodeCommit"}` the data was generated from.
pub const PIN_JSON: &str = include_str!("../data/pin.json");

#[cfg(test)]
mod tests {
    use super::*;

    /// The catalog must be regenerated whenever the workspace pin moves.
    #[test]
    fn catalog_was_generated_at_the_workspace_pin() {
        let manifest =
            std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../Cargo.toml"))
                .unwrap();
        let pinned_commit = manifest
            .lines()
            .find_map(|l| l.strip_prefix("hoocode-commit = "))
            .expect("workspace pin")
            .trim_matches('"');
        assert!(
            PIN_JSON.contains(&format!("\"hoocodeCommit\": \"{pinned_commit}\"")),
            "catalog pin {PIN_JSON} != workspace pin {pinned_commit}; run scripts/convert_models_to_json.py"
        );
    }

    #[test]
    fn catalog_files_are_json_arrays() {
        for data in [MODELS_JSON, IMAGE_MODELS_JSON] {
            assert!(data.trim_start().starts_with('['));
            assert!(data.trim_end().ends_with(']'));
        }
    }
}
