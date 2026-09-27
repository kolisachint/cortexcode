//! App-level TLS CA trust for cortex's own outbound traffic (provider calls,
//! GitHub API, downloads): port of hoocode `utils/tls-ca.ts`.
//!
//! Lets cortex work behind TLS-intercepting proxies with certificate
//! validation kept on.
//!
//! Invariants:
//! - Verification is never disabled.
//! - Trust is additive to the bundled roots (webpki-roots here, Node's
//!   `rootCertificates` in hoocode): a custom CA extends the set.
//! - Fail closed: a missing/invalid CA source warns once and is skipped.
//!
//! This does not cover the `webfetch`/`websearch` tools, which shell out to a
//! separate binary with its own TLS stack.
//!
//! Every reqwest client in the workspace is built from [`http_client_builder`],
//! which applies the trust set installed by [`configure_global_tls`] (the
//! analog of hoocode's global HTTPS agent + undici dispatcher).

use std::collections::HashSet;
use std::io::IsTerminal;
use std::sync::{Mutex, OnceLock, RwLock};

/// Where the trust set comes from: the argv pre-scan and the environment.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TlsSources {
    /// `--ca-cert <path>` / `--ca-cert=<path>`.
    pub ca_cert_flag: Option<String>,
    /// `CORTEX_CA_CERT`.
    pub ca_cert_env: Option<String>,
    /// `NODE_EXTRA_CA_CERTS`.
    pub node_extra_ca_certs: Option<String>,
    /// `--use-system-ca` or `CORTEX_USE_SYSTEM_CA=1|true|yes`.
    pub use_system_ca: bool,
    /// `NODE_TLS_REJECT_UNAUTHORIZED=0` is set (warned about, never honored).
    pub reject_unauthorized_disabled: bool,
}

/// `readArgValue`: scan argv for `--flag value` or `--flag=value`.
fn read_arg_value(argv: &[String], flag: &str) -> Option<String> {
    let with_eq = format!("{flag}=");
    for (i, current) in argv.iter().enumerate() {
        if current == flag {
            return argv
                .get(i + 1)
                .filter(|next| !next.starts_with('-'))
                .map(|next| next.trim().to_string())
                .filter(|v| !v.is_empty());
        }
        if let Some(value) = current.strip_prefix(&with_eq) {
            let value = value.trim();
            return (!value.is_empty()).then(|| value.to_string());
        }
    }
    None
}

impl TlsSources {
    /// Read the sources from `argv` (a pre-scan, since clients may be built
    /// before argument parsing) and `env`.
    pub fn from_args_and_env(argv: &[String], env: impl Fn(&str) -> Option<String>) -> Self {
        let non_empty = |key: &str| {
            env(key)
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        };
        let system_env = env("CORTEX_USE_SYSTEM_CA")
            .map(|v| v.trim().to_ascii_lowercase())
            .is_some_and(|v| v == "1" || v == "true" || v == "yes");
        Self {
            ca_cert_flag: read_arg_value(argv, "--ca-cert"),
            ca_cert_env: non_empty("CORTEX_CA_CERT"),
            node_extra_ca_certs: non_empty("NODE_EXTRA_CA_CERTS"),
            use_system_ca: argv.iter().any(|a| a == "--use-system-ca") || system_env,
            reject_unauthorized_disabled: env("NODE_TLS_REJECT_UNAUTHORIZED").as_deref()
                == Some("0"),
        }
    }

    /// From the real process arguments and environment.
    pub fn from_process() -> Self {
        let argv: Vec<String> = std::env::args().collect();
        Self::from_args_and_env(&argv, |k| std::env::var(k).ok())
    }

    /// `resolveExplicitCAPath`: `--ca-cert` > `CORTEX_CA_CERT` >
    /// `NODE_EXTRA_CA_CERTS`; only the first configured source is used.
    pub fn explicit_ca_path(&self) -> Option<&str> {
        self.ca_cert_flag
            .as_deref()
            .or(self.ca_cert_env.as_deref())
            .or(self.node_extra_ca_certs.as_deref())
    }
}

/// One trusted certificate beyond the bundled roots.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtraCa {
    /// An explicit PEM bundle's text (may hold several certificates).
    Pem(String),
    /// A certificate from the OS trust store.
    SystemDer(Vec<u8>),
}

/// The additive trust set: the bundled roots (always) plus [`ExtraCa`]s.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TrustedCas {
    pub extra: Vec<ExtraCa>,
}

fn warned() -> &'static Mutex<HashSet<String>> {
    static WARNED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    WARNED.get_or_init(Mutex::default)
}

/// `warnOnce`: `console.warn(chalk.yellow("[tls] ..."))`, deduplicated by
/// message for the life of the process.
fn warn_once(message: &str) {
    if !warned().lock().unwrap().insert(message.to_string()) {
        return;
    }
    let color =
        std::io::stdout().is_terminal() && std::env::var("TERM").map_or(true, |t| t != "dumb");
    if color {
        eprintln!("\x1b[33m[tls] {message}\x1b[39m");
    } else {
        eprintln!("[tls] {message}");
    }
}

/// `readCABundle`: a PEM bundle from a readable regular file, warning and
/// skipping on failure.
fn read_ca_bundle(path: &str) -> Option<String> {
    match std::fs::metadata(path) {
        Ok(meta) if !meta.is_file() => {
            warn_once(&format!(
                "CA certificate path is not a regular file, skipping: {path}"
            ));
            return None;
        }
        Ok(_) => {}
        Err(err) => {
            warn_once(&format!(
                "Could not read CA certificate file, skipping: {path} ({})",
                node_fs_error(&err, "stat", path)
            ));
            return None;
        }
    }
    match std::fs::read(path) {
        Ok(bytes) => Some(String::from_utf8_lossy(&bytes).into_owned()),
        Err(err) => {
            warn_once(&format!(
                "Could not read CA certificate file, skipping: {path} ({})",
                node_fs_error(&err, "open", path)
            ));
            None
        }
    }
}

/// A Node fs error message (`ENOENT: no such file or directory, stat '<path>'`).
fn node_fs_error(err: &std::io::Error, syscall: &str, path: &str) -> String {
    let (code, text) = match err.kind() {
        std::io::ErrorKind::NotFound => ("ENOENT", "no such file or directory"),
        std::io::ErrorKind::PermissionDenied => ("EACCES", "permission denied"),
        _ => return err.to_string(),
    };
    format!("{code}: {text}, {syscall} '{path}'")
}

/// `resolveTrustedCAs`: the explicit PEM bundle from the first configured
/// source (when set), then the OS store only when opted in. Never fails:
/// every failing source is warned once and skipped.
pub fn resolve_trusted_cas(sources: &TlsSources) -> TrustedCas {
    let mut extra = Vec::new();
    if let Some(path) = sources.explicit_ca_path() {
        if let Some(bundle) = read_ca_bundle(path) {
            extra.push(ExtraCa::Pem(bundle));
        }
    }
    if sources.use_system_ca {
        let result = rustls_native_certs::load_native_certs();
        if let Some(err) = result.errors.first() {
            if result.certs.is_empty() {
                warn_once(&format!(
                    "Could not read the system CA store, skipping: {err}"
                ));
            }
        }
        for cert in result.certs {
            extra.push(ExtraCa::SystemDer(cert.as_ref().to_vec()));
        }
    }
    // `dedupe`: by trimmed content, insertion order kept, empty entries dropped.
    let mut seen = HashSet::new();
    extra.retain(|ca| match ca {
        ExtraCa::Pem(pem) => {
            let key = pem.trim();
            !key.is_empty() && seen.insert(key.as_bytes().to_vec())
        }
        ExtraCa::SystemDer(der) => !der.is_empty() && seen.insert(der.clone()),
    });
    TrustedCas { extra }
}

impl TrustedCas {
    /// The reqwest certificates to add on top of the bundled roots. A PEM
    /// bundle that does not parse is warned once and skipped.
    pub fn certificates(&self) -> Vec<reqwest::Certificate> {
        let mut out = Vec::new();
        for ca in &self.extra {
            match ca {
                ExtraCa::Pem(pem) => match reqwest::Certificate::from_pem_bundle(pem.as_bytes()) {
                    Ok(certs) if !certs.is_empty() => out.extend(certs),
                    Ok(_) => warn_once("CA certificate bundle holds no certificates, skipping"),
                    Err(err) => warn_once(&format!(
                        "Could not parse CA certificate bundle, skipping ({err})"
                    )),
                },
                ExtraCa::SystemDer(der) => {
                    if let Ok(cert) = reqwest::Certificate::from_der(der) {
                        out.push(cert);
                    }
                }
            }
        }
        out
    }
}

fn global() -> &'static RwLock<Vec<reqwest::Certificate>> {
    static GLOBAL: OnceLock<RwLock<Vec<reqwest::Certificate>>> = OnceLock::new();
    GLOBAL.get_or_init(RwLock::default)
}

/// `configureGlobalTLS`: resolve the trust set and install it for every client
/// later built with [`http_client_builder`]. Warns once when
/// `NODE_TLS_REJECT_UNAUTHORIZED=0` is set (cortex never honors it).
pub fn configure_global_tls(sources: &TlsSources) -> TrustedCas {
    let cas = resolve_trusted_cas(sources);
    *global().write().unwrap() = cas.certificates();
    if sources.reject_unauthorized_disabled {
        warn_once(
            "NODE_TLS_REJECT_UNAUTHORIZED=0 disables all TLS certificate verification and is insecure. \
             Prefer --ca-cert <path> (or --use-system-ca) to trust your proxy's CA with verification kept on.",
        );
    }
    cas
}

/// A reqwest client builder carrying the installed trust set (bundled roots
/// plus any extra CAs). Every HTTP client in cortex starts here.
pub fn http_client_builder() -> reqwest::ClientBuilder {
    let mut builder = reqwest::Client::builder();
    for cert in global().read().unwrap().iter() {
        builder = builder.add_root_certificate(cert.clone());
    }
    builder
}

/// `reqwest::Client::new()` with the installed trust set.
pub fn http_client() -> reqwest::Client {
    http_client_builder()
        .build()
        .expect("reqwest client with default settings")
}
