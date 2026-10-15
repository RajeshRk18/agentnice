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
  statusline Claude Code statusLine command: saves rate limits
  status     print what the daemon sees";

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("daemon") => run_daemon(&args[1..]),
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
