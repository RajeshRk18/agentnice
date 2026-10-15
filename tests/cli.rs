//! Tests through the binary: a real daemon, the hook, and statusline.

use std::{
    fs,
    io::Write,
    os::unix::net::UnixStream,
    path::PathBuf,
    process::{Child, Command, Output, Stdio},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

struct Fixture {
    dir: PathBuf,
    daemon: Option<Child>,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Self { dir, daemon: None }
    }

    fn cmd(&self, args: &[&str]) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_agentnice"));
        c.args(args)
            .env("CLAUDE_CONFIG_DIR", &self.dir)
            .env_remove("AGENTNICE_SOCK")
            .env_remove("AGENTNICE_PRIORITY");
        c
    }

    fn start_daemon(&mut self, args: &[&str]) {
        let mut all = vec!["daemon"];
        all.extend(args);
        let child = self.cmd(&all).stderr(Stdio::null()).spawn().unwrap();
        self.daemon = Some(child);
        let deadline = Instant::now() + Duration::from_secs(5);
        while UnixStream::connect(self.dir.join("agentnice.sock")).is_err() {
            assert!(Instant::now() < deadline, "daemon did not start");
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn set_usage(&self, used: f64, resets_in_s: u64) {
        let json = format!(
            r#"{{"source":"claude","updated_at":0,"five_hour":{{"used_percentage":{used},"resets_at":{}}}}}"#,
            now_s() + resets_in_s
        );
        fs::write(self.dir.join("abtop-rate-limits.json"), json).unwrap();
    }

    fn hook(&self, event: &str, priority: &str) -> Output {
        let mut child = self
            .cmd(&["hook"])
            .env("AGENTNICE_PRIORITY", priority)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let input = format!(r#"{{"session_id":"s","hook_event_name":"{event}"}}"#);
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(d) = &mut self.daemon {
            let _ = d.kill();
            let _ = d.wait();
        }
    }
}

fn now_s() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

#[test]
fn hook_passes_when_no_daemon_runs() {
    let f = Fixture::new("no_daemon");
    let out = f.hook("PreToolUse", "low");
    assert!(out.status.success());
    assert_eq!(stdout(&out), "");
}

#[test]
fn hook_rejects_an_unknown_priority() {
    let f = Fixture::new("bad_priority");
    let out = f.hook("PreToolUse", "urgent");
    assert_eq!(out.status.code(), Some(1), "1 is a non-blocking hook error");
    assert!(String::from_utf8_lossy(&out.stderr).contains("AGENTNICE_PRIORITY"));
}

#[test]
fn low_waits_for_the_reset_and_normal_goes_now() {
    let mut f = Fixture::new("cap_wait");
    f.start_daemon(&[]);
    f.set_usage(85.0, 2);

    let t = Instant::now();
    let out = f.hook("PreToolUse", "normal");
    assert!(out.status.success());
    assert!(t.elapsed() < Duration::from_secs(1));

    let out = f.hook("PreToolUse", "low");
    assert!(out.status.success());
    assert_eq!(stdout(&out), "", "low goes on after the reset");
    assert!(t.elapsed() >= Duration::from_secs(1), "low waited");
}

#[test]
fn low_stops_when_the_reset_is_past_max_wait() {
    let mut f = Fixture::new("cap_stop");
    f.start_daemon(&["--max-wait", "60"]);
    f.set_usage(90.0, 3 * 3600);

    let out = f.hook("PreToolUse", "low");
    assert!(out.status.success());
    let reply: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(reply["continue"], false);
    let reason = reply["stopReason"].as_str().unwrap();
    assert!(reason.contains("five_hour quota is at 90%"), "{reason}");
    assert!(
        reason.contains("resets in 2h 59m") || reason.contains("resets in 3h 0m"),
        "{reason}"
    );
}

#[test]
fn starts_at_the_same_time_leave_one_gap_apart() {
    let mut f = Fixture::new("pace");
    f.start_daemon(&["--gap", "1"]);
    let t = Instant::now();
    thread::scope(|s| {
        let runs: Vec<_> = (0..3)
            .map(|_| s.spawn(|| f.hook("SessionStart", "normal")))
            .collect();
        for r in runs {
            assert!(r.join().unwrap().status.success());
        }
    });
    let took = t.elapsed();
    assert!(
        took >= Duration::from_secs(2),
        "3 starts need 2 gaps, took {took:?}"
    );
    assert!(took < Duration::from_secs(4), "took {took:?}");
}

#[test]
fn a_killed_hook_leaves_the_line() {
    let mut f = Fixture::new("leave");
    f.start_daemon(&[]);
    f.set_usage(85.0, 600);
    // Low is over its cap, so this hook waits 10 minutes unless killed.
    let mut waiting = f
        .cmd(&["hook"])
        .env("AGENTNICE_PRIORITY", "low")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    waiting
        .stdin
        .take()
        .unwrap()
        .write_all(br#"{"hook_event_name":"PreToolUse"}"#)
        .unwrap();
    let waiters = || {
        let out = f.cmd(&["status"]).output().unwrap();
        serde_json::from_slice::<serde_json::Value>(&out.stdout).unwrap()["waiting"].clone()
    };
    thread::sleep(Duration::from_millis(300));
    assert_eq!(waiters(), 1);
    waiting.kill().unwrap();
    waiting.wait().unwrap();
    thread::sleep(Duration::from_millis(1500));
    assert_eq!(waiters(), 0);
}

#[test]
fn statusline_writes_the_file_abtop_reads() {
    let f = Fixture::new("statusline");
    let mut child = f
        .cmd(&["statusline"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(
            br#"{"rate_limits":{"five_hour":{"used_percentage":42.4,"resets_at":1900000000},
                "seven_day":{"used_percentage":13,"resets_at":1900500000}}}"#,
        )
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(stdout(&out), "5h 42% · 7d 13%\n");

    let file = fs::read_to_string(f.dir.join("abtop-rate-limits.json")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&file).unwrap();
    assert_eq!(v["source"], "claude");
    assert_eq!(v["five_hour"]["used_percentage"], 42.4);
    assert_eq!(v["seven_day"]["resets_at"], 1900500000);
}
