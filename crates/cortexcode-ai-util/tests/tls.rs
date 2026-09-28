//! Port of hoocode `test/tls-ca.test.ts`. The bundled roots (webpki-roots)
//! are implicit in cortex, so "exactly the bundled roots" is an empty extra set.

use cortexcode_ai_util::tls::{
    http_client_builder, resolve_trusted_cas, ExtraCa, TlsSources, TrustedCas,
};
use std::collections::HashMap;
use std::path::Path;

const FAKE_CA_PEM: &str =
    "-----BEGIN CERTIFICATE-----\nCORTEX_TEST_FAKE_CA_DO_NOT_TRUST\n-----END CERTIFICATE-----\n";

fn sources(argv: &[&str], env: &[(&str, &Path)]) -> TlsSources {
    let argv: Vec<String> = argv.iter().map(|s| s.to_string()).collect();
    let env: HashMap<String, String> = env
        .iter()
        .map(|(k, v)| (k.to_string(), v.display().to_string()))
        .collect();
    TlsSources::from_args_and_env(&argv, |k| env.get(k).cloned())
}

fn pem(text: &str) -> ExtraCa {
    ExtraCa::Pem(text.to_string())
}

struct Fixture {
    _dir: tempfile::TempDir,
    ca_path: std::path::PathBuf,
    dir: std::path::PathBuf,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let ca_path = dir.path().join("corporate-ca.pem");
    std::fs::write(&ca_path, FAKE_CA_PEM).unwrap();
    Fixture {
        dir: dir.path().to_path_buf(),
        _dir: dir,
        ca_path,
    }
}

#[test]
fn always_includes_the_bundled_roots_and_nothing_else_by_default() {
    assert_eq!(
        resolve_trusted_cas(&sources(&[], &[])),
        TrustedCas::default()
    );
}

#[test]
fn merges_a_custom_pem_from_cortex_ca_cert_additively() {
    let f = fixture();
    let cas = resolve_trusted_cas(&sources(&[], &[("CORTEX_CA_CERT", &f.ca_path)]));
    assert_eq!(cas.extra, vec![pem(FAKE_CA_PEM)]);
}

#[test]
fn deduplicates_when_the_same_pem_comes_from_two_sources() {
    let f = fixture();
    let cas = resolve_trusted_cas(&sources(
        &[],
        &[
            ("CORTEX_CA_CERT", &f.ca_path),
            ("NODE_EXTRA_CA_CERTS", &f.ca_path),
        ],
    ));
    assert_eq!(cas.extra, vec![pem(FAKE_CA_PEM)]);
}

#[test]
fn falls_back_to_node_extra_ca_certs() {
    let f = fixture();
    let cas = resolve_trusted_cas(&sources(&[], &[("NODE_EXTRA_CA_CERTS", &f.ca_path)]));
    assert_eq!(cas.extra, vec![pem(FAKE_CA_PEM)]);
}

#[test]
fn a_missing_ca_file_keeps_the_defaults_and_does_not_fail() {
    let f = fixture();
    let missing = f.dir.join("does-not-exist.pem");
    let cas = resolve_trusted_cas(&sources(&[], &[("CORTEX_CA_CERT", &missing)]));
    assert!(cas.extra.is_empty());
    // A directory is not a regular file either.
    let cas = resolve_trusted_cas(&sources(&[], &[("CORTEX_CA_CERT", &f.dir)]));
    assert!(cas.extra.is_empty());
}

#[test]
fn honors_precedence_flag_then_cortex_env_then_node_env() {
    let f = fixture();
    let flag_path = f.dir.join("flag-ca.pem");
    let flag_pem = "-----BEGIN CERTIFICATE-----\nCORTEX_TEST_FLAG_CA\n-----END CERTIFICATE-----\n";
    std::fs::write(&flag_path, flag_pem).unwrap();
    let other_path = f.dir.join("other-ca.pem");
    std::fs::write(
        &other_path,
        "-----BEGIN CERTIFICATE-----\nCORTEX_TEST_OTHER_CA\n-----END CERTIFICATE-----\n",
    )
    .unwrap();
    let flag = flag_path.display().to_string();
    let env = [
        ("CORTEX_CA_CERT", f.ca_path.as_path()),
        ("NODE_EXTRA_CA_CERTS", other_path.as_path()),
    ];
    let cas = resolve_trusted_cas(&sources(&["node", "cli.js", "--ca-cert", &flag], &env));
    assert_eq!(cas.extra, vec![pem(flag_pem)]);
    let eq = format!("--ca-cert={flag}");
    let cas = resolve_trusted_cas(&sources(&["cli.js", &eq], &env));
    assert_eq!(cas.extra, vec![pem(flag_pem)]);
}

#[test]
fn ca_cert_flag_value_rules_match_the_argv_prescan() {
    // A following flag is not a value, and a blank value is no value.
    let s = sources(&["--ca-cert", "--verbose"], &[]);
    assert_eq!(s.ca_cert_flag, None);
    let s = sources(&["--ca-cert=  "], &[]);
    assert_eq!(s.ca_cert_flag, None);
    let s = sources(&["--ca-cert", " /x.pem "], &[]);
    assert_eq!(s.ca_cert_flag.as_deref(), Some("/x.pem"));
}

#[test]
fn excludes_the_system_store_unless_opted_in() {
    assert!(resolve_trusted_cas(&sources(&[], &[])).extra.is_empty());
    // Opt-in never fails; the store may be empty or overlap the bundled roots.
    let with_flag = resolve_trusted_cas(&sources(&["--use-system-ca"], &[]));
    assert!(with_flag
        .extra
        .iter()
        .all(|ca| matches!(ca, ExtraCa::SystemDer(_))));
    let env = TlsSources::from_args_and_env(&[], |k| {
        (k == "CORTEX_USE_SYSTEM_CA").then(|| " Yes ".to_string())
    });
    assert!(env.use_system_ca);
    let env = TlsSources::from_args_and_env(&[], |k| {
        (k == "CORTEX_USE_SYSTEM_CA").then(|| "0".to_string())
    });
    assert!(!env.use_system_ca);
}

#[test]
fn real_pem_bundles_become_certificates_and_invalid_ones_are_skipped() {
    let real = include_str!("fixtures/isrg-root-x1.pem");
    let cas = TrustedCas {
        extra: vec![pem(real), pem(&format!("{real}{real}")), pem(FAKE_CA_PEM)],
    };
    // One from the first bundle, two from the second, none from the fake.
    assert_eq!(cas.certificates().len(), 3);
}

#[test]
fn client_builder_builds_with_the_installed_trust_set() {
    let f = fixture();
    let real = f.dir.join("real.pem");
    std::fs::write(&real, include_str!("fixtures/isrg-root-x1.pem")).unwrap();
    let cas =
        cortexcode_ai_util::tls::configure_global_tls(&sources(&[], &[("CORTEX_CA_CERT", &real)]));
    assert_eq!(cas.extra.len(), 1);
    http_client_builder().build().unwrap();
}
