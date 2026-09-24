//! Port of hoocode `core/resolve-config-value.ts`: a config value is a shell command
//! (`!cmd`, stdout trimmed), else an environment variable name, else a literal.

use std::collections::HashMap;
use std::process::{Command, Stdio};

/// Resolve without caching (TS `resolveConfigValueUncached`).
pub fn resolve_config_value(config: &str) -> Option<String> {
    if let Some(command) = config.strip_prefix('!') {
        return execute_command(command);
    }
    match std::env::var(config) {
        Ok(v) if !v.is_empty() => Some(v),
        _ => Some(config.to_string()),
    }
}

/// `execSync(command)` with a 10s timeout and stdout captured; failures and empty
/// output resolve to `None`.
fn execute_command(command: &str) -> Option<String> {
    #[cfg(windows)]
    let mut cmd = {
        let mut c = Command::new("cmd");
        c.args(["/C", command]);
        c
    };
    #[cfg(not(windows))]
    let mut cmd = {
        let mut c = Command::new("/bin/sh");
        c.args(["-c", command]);
        c
    };
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    return None;
                }
                break;
            }
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            _ => {
                let _ = child.kill();
                return None;
            }
        }
    }
    let output = child.wait_with_output().ok()?;
    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!value.is_empty()).then_some(value)
}

/// TS `resolveConfigValueOrThrow`.
pub fn resolve_config_value_or_err(config: &str, description: &str) -> Result<String, String> {
    if let Some(value) = resolve_config_value(config) {
        return Ok(value);
    }
    match config.strip_prefix('!') {
        Some(command) => Err(format!(
            "Failed to resolve {description} from shell command: {command}"
        )),
        None => Err(format!("Failed to resolve {description}")),
    }
}

/// TS `resolveHeadersOrThrow`.
pub fn resolve_headers_or_err(
    headers: Option<&HashMap<String, String>>,
    description: &str,
) -> Result<Option<HashMap<String, String>>, String> {
    let Some(headers) = headers else {
        return Ok(None);
    };
    let mut resolved = HashMap::new();
    for (key, value) in headers {
        resolved.insert(
            key.clone(),
            resolve_config_value_or_err(value, &format!("{description} header \"{key}\""))?,
        );
    }
    Ok(Some(resolved))
}
