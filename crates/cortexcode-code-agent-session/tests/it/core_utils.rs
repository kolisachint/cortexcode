//! `core/exec.ts`, `core/event-bus.ts` and `core/output-guard.ts` (hoocode has
//! no dedicated tests for these; the cases pin the TS behavior).

use cortexcode_ai_types::AbortSignal;
use cortexcode_code_agent_session::event_bus::EventBus;
use cortexcode_code_agent_session::exec::{exec_command, ExecOptions, ExecResult};
use cortexcode_code_agent_session::output_guard;
use serde_json::{json, Value};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn sh(script: &str) -> Vec<String> {
    vec!["-c".into(), script.into()]
}

#[tokio::test]
async fn exec_collects_stdout_stderr_and_code() {
    let dir = tempfile::tempdir().unwrap();
    let r = exec_command(
        "sh",
        &sh("pwd; printf 'h\\303\\251' ; echo err >&2; exit 3"),
        dir.path(),
        ExecOptions::default(),
    )
    .await;
    let cwd = dir.path().canonicalize().unwrap();
    assert_eq!(r.stdout, format!("{}\nhé", cwd.display()));
    assert_eq!(r.stderr, "err\n");
    assert_eq!(r.code, 3);
    assert!(!r.killed);
}

#[tokio::test]
async fn exec_passes_args_without_a_shell() {
    let r = exec_command(
        "echo",
        &["$HOME".to_string(), "a b".to_string()],
        Path::new("."),
        ExecOptions::default(),
    )
    .await;
    assert_eq!(r.stdout, "$HOME a b\n");
}

#[tokio::test]
async fn exec_spawn_failure_resolves_with_code_1() {
    let r = exec_command(
        "definitely-not-a-command-cortex",
        &[],
        Path::new("."),
        ExecOptions::default(),
    )
    .await;
    assert_eq!(
        r,
        ExecResult {
            code: 1,
            ..ExecResult::default()
        }
    );
}

#[tokio::test]
async fn exec_timeout_kills_with_sigterm() {
    let start = Instant::now();
    let r = exec_command(
        "sh",
        &sh("echo started; sleep 10"),
        Path::new("."),
        ExecOptions {
            timeout_ms: Some(200),
            ..ExecOptions::default()
        },
    )
    .await;
    assert!(start.elapsed() < Duration::from_secs(5));
    assert!(r.killed);
    // Killed by a signal: Node reports `code ?? 0`.
    assert_eq!(r.code, 0);
    assert_eq!(r.stdout, "started\n");
}

#[tokio::test]
async fn exec_escalates_to_sigkill_when_sigterm_is_ignored() {
    let args = sh("trap '' TERM; echo ready; while :; do sleep 0.05; done");
    let task = exec_command(
        "sh",
        &args,
        Path::new("."),
        ExecOptions {
            timeout_ms: Some(100),
            ..ExecOptions::default()
        },
    );
    // Real time: SIGTERM at 100ms is ignored, SIGKILL follows 5s later.
    let r = tokio::time::timeout(Duration::from_secs(15), task)
        .await
        .expect("SIGKILL after the 5s grace");
    assert!(r.killed);
}

#[tokio::test]
async fn exec_abort_signal_kills_the_command() {
    let signal = AbortSignal::new();
    let s2 = signal.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(150)).await;
        s2.abort();
    });
    let r = exec_command(
        "sleep",
        &["10".to_string()],
        Path::new("."),
        ExecOptions {
            signal: Some(signal),
            ..ExecOptions::default()
        },
    )
    .await;
    assert!(r.killed);
}

#[tokio::test]
async fn exec_already_aborted_signal_kills_immediately() {
    let signal = AbortSignal::new();
    signal.abort();
    let start = Instant::now();
    let r = exec_command(
        "sleep",
        &["10".to_string()],
        Path::new("."),
        ExecOptions {
            signal: Some(signal),
            ..ExecOptions::default()
        },
    )
    .await;
    assert!(r.killed);
    assert!(start.elapsed() < Duration::from_secs(5));
}

#[tokio::test]
async fn exec_does_not_hang_on_pipes_held_by_detached_descendants() {
    let start = Instant::now();
    let r = exec_command(
        "sh",
        &sh("(sleep 5 &) ; echo done"),
        Path::new("."),
        ExecOptions::default(),
    )
    .await;
    assert_eq!(r.stdout, "done\n");
    assert_eq!(r.code, 0);
    assert!(start.elapsed() < Duration::from_secs(3));
}

#[test]
fn event_bus_delivers_per_channel_in_order_and_unsubscribes() {
    let bus = EventBus::new();
    let seen: Arc<Mutex<Vec<String>>> = Arc::default();
    let s1 = seen.clone();
    let sub = bus.on("a", move |v| {
        s1.lock().unwrap().push(format!("1:{v}"));
        Ok(())
    });
    let s2 = seen.clone();
    bus.on("a", move |v| {
        s2.lock().unwrap().push(format!("2:{v}"));
        Ok(())
    });
    let s3 = seen.clone();
    bus.on("b", move |v| {
        s3.lock().unwrap().push(format!("b:{v}"));
        Ok(())
    });
    bus.emit("a", &json!(1));
    sub.call();
    bus.emit("a", &json!(2));
    bus.emit("b", &Value::Null);
    bus.emit("nobody", &json!(3));
    assert_eq!(*seen.lock().unwrap(), ["1:1", "2:1", "2:2", "b:null"]);
    bus.clear();
    bus.emit("a", &json!(4));
    bus.emit("b", &json!(4));
    assert_eq!(seen.lock().unwrap().len(), 4);
}

#[test]
fn event_bus_handler_errors_do_not_stop_other_handlers() {
    let bus = EventBus::new();
    let hits = Arc::new(Mutex::new(0));
    bus.on("x", |_| Err("boom".into()));
    let h = hits.clone();
    bus.on("x", move |_| {
        *h.lock().unwrap() += 1;
        Ok(())
    });
    bus.emit("x", &json!({}));
    assert_eq!(*hits.lock().unwrap(), 1);
}

#[test]
fn event_bus_handlers_may_reenter_the_bus() {
    let bus = EventBus::new();
    let inner = bus.clone();
    let seen: Arc<Mutex<Vec<Value>>> = Arc::default();
    let s = seen.clone();
    bus.on("ping", move |v| {
        inner.emit("pong", v);
        Ok(())
    });
    let b2 = bus.clone();
    bus.on("pong", move |v| {
        s.lock().unwrap().push(v.clone());
        // Subscribing during an emit takes effect for the next emit only.
        let s = s.clone();
        b2.on("pong", move |v| {
            s.lock().unwrap().push(json!({"late": v}));
            Ok(())
        });
        Ok(())
    });
    bus.emit("ping", &json!("hi"));
    assert_eq!(*seen.lock().unwrap(), [json!("hi")]);
}

#[test]
fn output_guard_takeover_toggles() {
    assert!(!output_guard::is_stdout_taken_over());
    output_guard::take_over_stdout();
    output_guard::take_over_stdout();
    assert!(output_guard::is_stdout_taken_over());
    output_guard::write_stdout("");
    output_guard::write_raw_stdout("");
    output_guard::flush_raw_stdout().unwrap();
    output_guard::restore_stdout();
    assert!(!output_guard::is_stdout_taken_over());
}
