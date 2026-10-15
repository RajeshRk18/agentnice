//! agentnice: one quota scheduler for all Claude Code sessions on a machine.

mod daemon;
mod gate;
mod usage;

use serde_json::{Value, json};
use std::{
    env,
    io::{self, BufRead, BufReader, Read, Write},
    os::unix::net::UnixStream,
    path::PathBuf,
    process::ExitCode,
    time::{SystemTime, UNIX_EPOCH},
};

const USAGE: &str = "\
usage: agentnice <command>

  daemon [--low PCT] [--normal PCT] [--gap SECS] [--max-wait SECS]
             run the scheduler (defaults: 80, 95, 3, 18600)
  hook       Claude Code hook for SessionStart, UserPromptSubmit and
             PreToolUse. Set AGENTNICE_PRIORITY=low|normal|high per session.
  statusline Claude Code statusLine command: saves rate limits
  status     print what the daemon sees";

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("daemon") => run_daemon(&args[1..]),
        Some("hook") => hook(),
        Some("statusline") => statusline(),
        Some("status") => status(),
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("agentnice: {e}");
            ExitCode::FAILURE
        }
    }
}

/// `$CLAUDE_CONFIG_DIR`, or `~/.claude`. Each config dir is one account,
/// so it gets its own quota file and its own daemon.
fn config_dir() -> io::Result<PathBuf> {
    if let Some(dir) = env::var_os("CLAUDE_CONFIG_DIR") {
        return Ok(dir.into());
    }
    let home = env::var_os("HOME").ok_or_else(|| io::Error::other("HOME is not set"))?;
    Ok(PathBuf::from(home).join(".claude"))
}

fn usage_path() -> io::Result<PathBuf> {
    Ok(config_dir()?.join("abtop-rate-limits.json"))
}

/// `$AGENTNICE_SOCK`, or `agentnice.sock` in the config dir.
fn sock_path() -> io::Result<PathBuf> {
    match env::var_os("AGENTNICE_SOCK") {
        Some(p) => Ok(p.into()),
        None => Ok(config_dir()?.join("agentnice.sock")),
    }
}

fn now_s() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock is before 1970")
        .as_secs()
}

fn read_stdin_json() -> io::Result<Value> {
    let mut text = String::new();
    io::stdin().read_to_string(&mut text)?;
    serde_json::from_str(&text).map_err(|e| io::Error::other(format!("bad JSON on stdin: {e}")))
}

/// Claude Code's statusline command. Saves `rate_limits` for the daemon
/// and prints a short line such as `5h 42% · 7d 13%`.
fn statusline() -> io::Result<()> {
    let windows = usage::parse(&read_stdin_json()?["rate_limits"]);
    if windows.is_empty() {
        return Ok(());
    }
    usage::write(&usage_path()?, &windows, now_s())?;
    let parts: Vec<String> = windows
        .iter()
        .map(|w| {
            let label = if w.name == "five_hour" { "5h" } else { "7d" };
            format!("{label} {:.0}%", w.used)
        })
        .collect();
    println!("{}", parts.join(" · "));
    Ok(())
}

fn run_daemon(args: &[String]) -> io::Result<()> {
    let mut cfg = gate::Config::default();
    for pair in args.chunks(2) {
        let [flag, value] = pair else {
            return Err(io::Error::other(format!("{} needs a value", pair[0])));
        };
        let num = |what: &str| {
            value
                .parse::<f64>()
                .ok()
                .filter(|n| *n >= 0.0)
                .ok_or_else(|| io::Error::other(format!("{flag}: {value:?} is not {what}")))
        };
        match flag.as_str() {
            "--low" => cfg.caps[0] = num("a percent")?,
            "--normal" => cfg.caps[1] = num("a percent")?,
            "--gap" => cfg.gap_ms = (num("a number of seconds")? * 1000.0) as u64,
            "--max-wait" => cfg.max_wait_s = num("a number of seconds")? as u64,
            _ => return Err(io::Error::other(format!("unknown flag {flag}\n{USAGE}"))),
        }
    }
    daemon::run(&sock_path()?, usage_path()?, cfg)
}

/// Sends one request and reads one reply line.
fn ask(conn: &UnixStream, req: Value) -> io::Result<Value> {
    writeln!(&*conn, "{req}")?;
    let mut line = String::new();
    BufReader::new(conn).read_line(&mut line)?;
    serde_json::from_str(&line).map_err(|e| io::Error::other(format!("bad reply: {e}")))
}

fn status() -> io::Result<()> {
    let sock = sock_path()?;
    let conn = UnixStream::connect(&sock)
        .map_err(|e| io::Error::other(format!("no daemon on {}: {e}", sock.display())))?;
    let reply = ask(&conn, json!({ "op": "status" }))?;
    println!(
        "{}",
        serde_json::to_string_pretty(&reply).expect("Value serializes")
    );
    Ok(())
}

/// Waits while the daemon holds this session. If no daemon runs, it does
/// nothing: a dead scheduler must never block work.
fn hook() -> io::Result<()> {
    let event = read_stdin_json()?;
    let priority = env::var("AGENTNICE_PRIORITY").unwrap_or_else(|_| "normal".into());
    if gate::Priority::parse(&priority).is_none() {
        return Err(io::Error::other(format!(
            "AGENTNICE_PRIORITY={priority:?}: use low, normal or high"
        )));
    }
    // A start is about to call the API, so it waits for a slot in the line.
    let paced = matches!(
        event["hook_event_name"].as_str(),
        Some("SessionStart" | "UserPromptSubmit")
    );
    let Ok(conn) = UnixStream::connect(sock_path()?) else {
        return Ok(());
    };
    let reply = ask(
        &conn,
        json!({ "op": "acquire", "priority": priority, "paced": paced }),
    )?;
    if let Some(e) = reply.get("error") {
        return Err(io::Error::other(format!("daemon: {e}")));
    }
    if reply["go"] == false {
        let left = reply["until"].as_u64().unwrap_or(0).saturating_sub(now_s());
        let reason = format!(
            "agentnice: {priority} session stopped. {} quota is at {:.0}%. It resets in {}.",
            reply["window"].as_str().unwrap_or("?"),
            reply["used"].as_f64().unwrap_or(0.0),
            human(left),
        );
        println!("{}", json!({ "continue": false, "stopReason": reason }));
    }
    Ok(())
}

fn human(secs: u64) -> String {
    let (d, h, m) = (secs / 86400, secs % 86400 / 3600, secs % 3600 / 60);
    match (d, h) {
        (0, 0) => format!("{m}m"),
        (0, _) => format!("{h}h {m}m"),
        _ => format!("{d}d {h}h"),
    }
}
