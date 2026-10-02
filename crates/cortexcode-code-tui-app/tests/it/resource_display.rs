//! `show_loaded_resources` against `fixtures/resource-display-gold.json`
//! (hoocode's `showLoadedResources` over the same data, from the pin).

use crate::support::lock;
use cortexcode_code_resources::context_files::{ContextFile, ContextFileSize};
use cortexcode_code_resources::diagnostics::{
    DiagnosticType, ResourceCollision, ResourceDiagnostic,
};
use cortexcode_code_resources::source_info::{SourceInfo, SourceOrigin, SourceScope};
use cortexcode_code_tui_app::resource_display::*;
use serde_json::Value;

fn source_info(v: &Value) -> Option<SourceInfo> {
    let scope = match v["scope"].as_str()? {
        "user" => SourceScope::User,
        "project" => SourceScope::Project,
        _ => SourceScope::Temporary,
    };
    Some(SourceInfo {
        path: String::new(),
        source: v["source"].as_str()?.into(),
        scope,
        origin: SourceOrigin::TopLevel,
        base_dir: v["baseDir"].as_str().map(str::to_string),
    })
}

fn items(v: &Value) -> Vec<ListedItem> {
    v.as_array()
        .map(|a| {
            a.iter()
                .map(|i| ListedItem {
                    name: i[0].as_str().unwrap().into(),
                    path: i[1].as_str().unwrap().into(),
                    source_info: source_info(&i[2]),
                    display_name: None,
                })
                .collect()
        })
        .unwrap_or_default()
}

fn diagnostics(v: &Value) -> Vec<ResourceDiagnostic> {
    v.as_array()
        .map(|a| {
            a.iter()
                .map(|d| ResourceDiagnostic {
                    kind: match d["type"].as_str().unwrap() {
                        "collision" => DiagnosticType::Collision,
                        "error" => DiagnosticType::Error,
                        _ => DiagnosticType::Warning,
                    },
                    message: d["message"].as_str().unwrap().into(),
                    path: d["path"].as_str().map(str::to_string),
                    collision: d.get("collision").map(|c| ResourceCollision {
                        resource_type: c["resourceType"].as_str().unwrap().into(),
                        name: c["name"].as_str().unwrap().into(),
                        winner_path: c["winnerPath"].as_str().unwrap().into(),
                        loser_path: c["loserPath"].as_str().unwrap().into(),
                        winner_source: None,
                        loser_source: None,
                    }),
                })
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn renders_the_listing_like_hoocode() {
    let _g = lock(Some("dark"));
    let gold: Value =
        serde_json::from_str(include_str!("../fixtures/resource-display-gold.json")).unwrap();
    std::env::set_var("HOME", gold["home"].as_str().unwrap());
    for (name, case) in gold["cases"].as_object().unwrap() {
        let c = &case["case"];
        let listing = ResourceListing {
            cwd: "/w".into(),
            skills: items(&c["skills"]),
            skill_diagnostics: diagnostics(&c["skillDiagnostics"]),
            templates: items(&c["templates"]),
            prompt_diagnostics: diagnostics(&c["promptDiagnostics"]),
            context_files: c["context"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .map(|f| ContextFile {
                            path: f[0].as_str().unwrap().into(),
                            content: String::new(),
                            tokens: f[1].as_u64(),
                            size: match f[2].as_str() {
                                Some("large") => Some(ContextFileSize::Large),
                                Some("truncated") => Some(ContextFileSize::Truncated),
                                _ => None,
                            },
                        })
                        .collect()
                })
                .unwrap_or_default(),
            context_warnings: serde_json::from_value(c["warnings"].clone()).unwrap_or_default(),
            agents: c["agents"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .map(|x| (x[0].as_str().unwrap().into(), x[1].as_str().unwrap().into()))
                        .collect()
                })
                .unwrap_or_default(),
            mcp: c["mcp"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .map(|m| McpServerStatus {
                            name: m["name"].as_str().unwrap().into(),
                            authorizing: m["state"] == "authorizing",
                            tool_count: m["toolCount"].as_u64().unwrap() as usize,
                            background: m["background"].as_bool().unwrap_or(false),
                            deferred: m["deferred"].as_bool().unwrap_or(false),
                        })
                        .collect()
                })
                .unwrap_or_default(),
            columns: c["columns"].as_u64().map(|n| n as usize),
            quiet_startup: c["quiet"].as_bool().unwrap_or(false),
            expanded: c["expanded"].as_bool().unwrap_or(false),
            ..Default::default()
        };
        let lines: Vec<String> = show_loaded_resources(&listing, false, true)
            .into_iter()
            .flat_map(|h| h.borrow_mut().render(100))
            .collect();
        let expected: Vec<String> = serde_json::from_value(case["lines"].clone()).unwrap();
        assert_eq!(lines, expected, "{name}");
    }
}
