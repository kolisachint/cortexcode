//! `--list-models [search]`: port of hoocode `packages/coding-agent/src/cli/list-models.ts`.

use cortexcode_ai_types::Model;
use cortexcode_code_models::{locale_compare, AuthLookup, ModelRegistry};
use std::io::Write;

/// `Number.prototype.toFixed(1)`: ties round away from zero.
fn to_fixed_1(value: f64) -> String {
    let scaled = value * 10.0;
    let rounded = if scaled.fract() == 0.5 {
        scaled.ceil()
    } else {
        scaled.round()
    };
    format!("{:.1}", rounded / 10.0)
}

/// `formatTokenCount`: 200000 -> "200K", 1000000 -> "1M", 32768 -> "32.8K".
fn format_token_count(count: u64) -> String {
    let count = count as f64;
    if count >= 1_000_000.0 {
        let millions = count / 1_000_000.0;
        return if millions.fract() == 0.0 {
            format!("{millions}M")
        } else {
            format!("{}M", to_fixed_1(millions))
        };
    }
    if count >= 1_000.0 {
        let thousands = count / 1_000.0;
        return if thousands.fract() == 0.0 {
            format!("{thousands}K")
        } else {
            format!("{}K", to_fixed_1(thousands))
        };
    }
    format!("{count}")
}

/// `listModels`: the models with configured auth, fuzzy-filtered by `search`,
/// as an aligned table on `output`.
pub fn list_models(
    registry: &ModelRegistry,
    auth: &dyn AuthLookup,
    search: Option<&str>,
    color: bool,
    output: &mut dyn Write,
    err: &mut dyn Write,
) -> std::io::Result<()> {
    if let Some(error) = registry.error() {
        let text = format!("Warning: errors loading models.json:\n{error}");
        if color {
            writeln!(err, "\x1b[33m{text}\x1b[39m")?;
        } else {
            writeln!(err, "{text}")?;
        }
    }

    let models: Vec<Model> = registry.get_available(auth).into_iter().cloned().collect();
    if models.is_empty() {
        writeln!(
            output,
            "{}",
            cortexcode_code_auth::auth_guidance::format_no_models_available_message()
        )?;
        return Ok(());
    }

    let mut filtered = match search.filter(|s| !s.is_empty()) {
        Some(pattern) => cortexcode_tui_fuzzy::fuzzy_filter(&models, pattern, |m| {
            format!("{} {}", m.provider, m.id)
        }),
        None => models,
    };
    if filtered.is_empty() {
        writeln!(
            output,
            "No models matching \"{}\"",
            search.unwrap_or_default()
        )?;
        return Ok(());
    }
    filtered.sort_by(|a, b| {
        locale_compare(&a.provider, &b.provider).then_with(|| locale_compare(&a.id, &b.id))
    });

    let header = [
        "provider", "model", "context", "max-out", "thinking", "images",
    ];
    let rows: Vec<[String; 6]> = filtered
        .iter()
        .map(|m| {
            [
                m.provider.clone(),
                m.id.clone(),
                format_token_count(m.context_window),
                format_token_count(m.max_tokens),
                if m.reasoning { "yes" } else { "no" }.to_string(),
                if m.input.iter().any(|i| i == "image") {
                    "yes"
                } else {
                    "no"
                }
                .to_string(),
            ]
        })
        .collect();
    let width = |i: usize| {
        rows.iter()
            .map(|r| r[i].chars().count())
            .chain([header[i].len()])
            .max()
            .unwrap_or(0)
    };
    let widths: Vec<usize> = (0..6).map(width).collect();
    let line = |cells: [&str; 6]| {
        cells
            .iter()
            .zip(&widths)
            .map(|(cell, w)| format!("{cell:<w$}"))
            .collect::<Vec<_>>()
            .join("  ")
    };
    writeln!(output, "{}", line(header))?;
    for row in &rows {
        writeln!(output, "{}", line(row.each_ref().map(String::as_str)))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_counts_format_like_hoocode() {
        assert_eq!(format_token_count(200_000), "200K");
        assert_eq!(format_token_count(1_000_000), "1M");
        assert_eq!(format_token_count(1_048_576), "1.0M");
        assert_eq!(format_token_count(32_768), "32.8K");
        assert_eq!(format_token_count(1_250), "1.3K");
        assert_eq!(format_token_count(16_384), "16.4K");
        assert_eq!(format_token_count(999), "999");
    }

    #[test]
    fn lists_available_models_sorted_and_aligned() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("models.json");
        std::fs::write(
            &path,
            r#"{"providers": {"zeta": {"baseUrl": "http://x", "api": "openai-completions", "apiKey": "k", "models": [{"id": "z-1", "input": ["text", "image"], "contextWindow": 32768}]},
                             "mock": {"baseUrl": "http://x", "api": "openai-completions", "apiKey": "k", "models": [{"id": "mock-model", "reasoning": true}, {"id": "Mock-B"}]}}}"#,
        )
        .unwrap();
        let registry = ModelRegistry::create(path);
        let (mut out, mut err) = (Vec::new(), Vec::new());
        list_models(
            &registry,
            &cortexcode_code_models::NoAuth,
            None,
            false,
            &mut out,
            &mut err,
        )
        .unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "provider  model       context  max-out  thinking  images\n\
             mock      Mock-B      128K     16.4K    no        no    \n\
             mock      mock-model  128K     16.4K    yes       no    \n\
             zeta      z-1         32.8K    16.4K    no        yes   \n"
        );
        let mut out = Vec::new();
        list_models(
            &registry,
            &cortexcode_code_models::NoAuth,
            Some("nothing"),
            false,
            &mut out,
            &mut err,
        )
        .unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "No models matching \"nothing\"\n"
        );
        assert!(err.is_empty());
    }
}
