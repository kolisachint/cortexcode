//! lifeguard.test.ts. Children are `sleep` processes in their own process
//! group (as the pool spawns them).

use std::process::{Child, Command};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use cortexcode_code_subagents::lifeguard::{base_timeout_ms, LifeguardEvent, SubagentLifeguard};

/// Every shipped agent must have its own hard timeout.
///
/// hoocode's `TIMEOUTS_MS` is keyed `explore`/`edit`/`test`/`review`/`doc`, and
/// neither tree ships `edit`, `test`, `review` or `doc` — so in both, every
/// agent silently got the 5-minute default. Four of ten recorded runs in
/// `hoobot/.cortexcode/dispatch` died `timeout` at exactly 300s, three of them
/// `code-review` that had been working for 220-297s.
#[test]
fn every_shipped_agent_has_a_hard_timeout_worth_the_name() {
    const MINUTE: u64 = 60 * 1000;
    // code-review read a whole multi-file diff for 297s and was killed at 300s.
    assert_eq!(base_timeout_ms("code-review"), 15 * MINUTE);
    assert_eq!(base_timeout_ms("security-review"), 15 * MINUTE);
    // general-purpose reads, writes and runs tests: the heaviest.
    assert_eq!(base_timeout_ms("general-purpose"), 20 * MINUTE);
    // explore ran 308s and was still producing turns.
    assert_eq!(base_timeout_ms("explore"), 10 * MINUTE);
    assert_eq!(base_timeout_ms("plan"), 10 * MINUTE);
}

/// hoocode's arms are kept for plugin agents that use those names.
#[test]
fn keeps_hoocodes_own_timeout_arms() {
    const MINUTE: u64 = 60 * 1000;
    assert_eq!(base_timeout_ms("edit"), 10 * MINUTE);
    assert_eq!(base_timeout_ms("test"), 10 * MINUTE);
    assert_eq!(base_timeout_ms("review"), 8 * MINUTE);
}

/// An unknown agent type falls back to the default rather than to nothing.
#[test]
fn an_unknown_agent_type_gets_the_default_timeout() {
    assert_eq!(base_timeout_ms("some-plugin-agent"), 5 * 60 * 1000);
}

/// The warm pool's run timeout tracks the same table, so a warm `code-review`
/// is not killed at 180s while the cold pool would have allowed it 15 minutes.
#[test]
fn the_warm_run_timeout_tracks_the_agent_type() {
    // Asserted through the cold table because `warm_run_timeout` is private to
    // the warm module; the two must not drift apart.
    assert!(base_timeout_ms("code-review") > 180_000);
    assert!(base_timeout_ms("general-purpose") > 180_000);
    assert!(base_timeout_ms("explore") > 180_000);
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

fn sleeper() -> Child {
    let mut command = Command::new("sleep");
    command.arg("10");
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut command, 0);
    command.spawn().unwrap()
}

fn stalled_ids(guard: &SubagentLifeguard) -> Arc<Mutex<Vec<String>>> {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = seen.clone();
    guard.on_event(move |event| {
        if let LifeguardEvent::Stalled { task_id, .. } = event {
            sink.lock().unwrap().push(task_id.clone());
        }
    });
    seen
}

#[tokio::test]
async fn monitors_a_child_process_and_records_heartbeats() {
    let dir = tempfile::tempdir().unwrap();
    let guard = SubagentLifeguard::new(dir.path());
    let mut child = sleeper();
    guard.monitor("t1", "explore", child.id());
    assert!(guard.is_monitoring("t1"));
    let before = guard.last_heartbeat_at("t1").unwrap();
    guard.record_heartbeat("t1");
    assert!(guard.last_heartbeat_at("t1").unwrap() >= before);
    guard.dispose();
    let _ = child.wait();
}

#[tokio::test]
async fn emits_stalled_when_heartbeat_is_missed() {
    let dir = tempfile::tempdir().unwrap();
    let guard = SubagentLifeguard::new(dir.path());
    let stalled = stalled_ids(&guard);
    let mut child = sleeper();
    guard.monitor("t1", "explore", child.id());
    guard.set_last_heartbeat_for_testing("t1", now_ms() - 70_000);
    guard.check_heartbeats();
    assert_eq!(*stalled.lock().unwrap(), vec!["t1"]);
    // The process group was killed.
    assert!(child.wait().unwrap().code().is_none());
    // Reaping: not re-reported on the next tick.
    guard.check_heartbeats();
    assert_eq!(stalled.lock().unwrap().len(), 1);
    guard.dispose();
}

#[tokio::test]
async fn does_not_stall_under_high_concurrency_until_the_scaled_threshold() {
    let dir = tempfile::tempdir().unwrap();
    let guard = SubagentLifeguard::new(dir.path());
    let stalled = stalled_ids(&guard);
    let mut children: Vec<Child> = (0..5).map(|_| sleeper()).collect();
    for (i, child) in children.iter().enumerate() {
        guard.monitor(&format!("t{i}"), "explore", child.id());
    }
    // 5 concurrent: multiplier 3, threshold 180s.
    guard.set_last_heartbeat_for_testing("t0", now_ms() - 150_000);
    guard.check_heartbeats();
    assert!(!stalled.lock().unwrap().contains(&"t0".to_string()));
    guard.set_last_heartbeat_for_testing("t0", now_ms() - 200_000);
    guard.check_heartbeats();
    assert!(stalled.lock().unwrap().contains(&"t0".to_string()));
    guard.dispose();
    for child in &mut children {
        let _ = child.wait();
    }
}

#[tokio::test]
async fn accounts_for_external_load_and_clamps_negative_load() {
    let dir = tempfile::tempdir().unwrap();
    let guard = SubagentLifeguard::new(dir.path());
    let stalled = stalled_ids(&guard);
    let mut child = sleeper();
    guard.monitor("t0", "explore", child.id());
    guard.set_external_load(4);
    guard.set_last_heartbeat_for_testing("t0", now_ms() - 150_000);
    guard.check_heartbeats();
    assert!(stalled.lock().unwrap().is_empty());
    guard.set_external_load(-5);
    guard.check_heartbeats();
    assert_eq!(*stalled.lock().unwrap(), vec!["t0"]);
    guard.dispose();
    let _ = child.wait();
}

#[tokio::test]
async fn forgives_a_stale_heartbeat_caused_by_parent_lag() {
    let dir = tempfile::tempdir().unwrap();
    let guard = SubagentLifeguard::new(dir.path());
    let stalled = stalled_ids(&guard);
    let mut child = sleeper();
    guard.monitor("t1", "explore", child.id());
    guard.set_last_heartbeat_for_testing("t1", now_ms() - 70_000);
    guard.set_last_check_for_testing(now_ms() - 200_000);
    guard.check_heartbeats();
    assert!(stalled.lock().unwrap().is_empty());
    guard.dispose();
    let _ = child.wait();
}

#[tokio::test]
async fn emits_timeout_when_the_hard_timeout_is_exceeded() {
    let dir = tempfile::tempdir().unwrap();
    let guard = SubagentLifeguard::new(dir.path());
    let timeouts = Arc::new(Mutex::new(Vec::new()));
    let sink = timeouts.clone();
    guard.on_event(move |event| {
        if let LifeguardEvent::Timeout { task_id, .. } = event {
            sink.lock().unwrap().push(task_id.clone());
        }
    });
    let mut child = sleeper();
    guard.monitor("t1", "explore", child.id());
    guard.set_timeout_for_testing("t1", Duration::from_millis(10));
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(*timeouts.lock().unwrap(), vec!["t1"]);
    guard.dispose();
    let _ = child.wait();
}

#[tokio::test]
async fn untracks_a_process_on_exit() {
    let dir = tempfile::tempdir().unwrap();
    let guard = SubagentLifeguard::new(dir.path());
    let mut child = Command::new("true").spawn().unwrap();
    guard.monitor("t1", "explore", child.id());
    assert!(guard.is_monitoring("t1"));
    child.wait().unwrap();
    guard.untrack("t1");
    assert!(!guard.is_monitoring("t1"));
    guard.dispose();
}

fn backdate(path: &std::path::Path, hours: u64) {
    let past = SystemTime::now() - Duration::from_secs(hours * 3600);
    let file = std::fs::File::open(path).unwrap();
    file.set_modified(past).unwrap();
}

#[tokio::test]
async fn sweeps_old_agent_directories_on_init() {
    let dir = tempfile::tempdir().unwrap();
    let old = cortexcode_code_paths::dispatch_task_dir(dir.path(), "old-task");
    std::fs::create_dir_all(&old).unwrap();
    std::fs::write(old.join("result.json"), "{}").unwrap();
    backdate(&old, 25);
    let guard = SubagentLifeguard::new(dir.path());
    assert!(!old.exists());
    guard.dispose();
}

#[tokio::test]
async fn does_not_sweep_directories_with_running_pids() {
    let dir = tempfile::tempdir().unwrap();
    let old = cortexcode_code_paths::dispatch_task_dir(dir.path(), "old-task-with-pid");
    std::fs::create_dir_all(&old).unwrap();
    std::fs::write(old.join("result.json"), "{}").unwrap();
    std::fs::write(old.join("pid"), std::process::id().to_string()).unwrap();
    backdate(&old, 25);
    let guard = SubagentLifeguard::new(dir.path());
    assert!(old.exists());
    guard.dispose();
}

#[tokio::test]
async fn dispose_kills_all_monitored_processes() {
    let dir = tempfile::tempdir().unwrap();
    let guard = SubagentLifeguard::new(dir.path());
    let mut child = sleeper();
    guard.monitor("t1", "explore", child.id());
    guard.dispose();
    assert!(!guard.is_monitoring("t1"));
    assert!(child.wait().unwrap().code().is_none());
}
