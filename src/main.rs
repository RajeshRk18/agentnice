//! agentnice: one quota scheduler for all Claude Code sessions on a machine.

mod usage;

use serde_json::Value;
use std::{
    env,
    io::{self, Read},
    path::PathBuf,
    process::ExitCode,
    time::{SystemTime, UNIX_EPOCH},
};

const USAGE: &str = "usage: agentnice statusline";

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("statusline") => statusline(),
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
