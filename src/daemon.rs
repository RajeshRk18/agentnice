//! The daemon. One Unix socket, one thread per client, one shared `Gate`.
//!
//! Protocol: the client sends one JSON line and gets one JSON line back.
//! - `{"op":"acquire","priority":"low","paced":true}` gets
//!   `{"go":true}` when the session may go on, or
//!   `{"go":false,"window":..,"used":..,"until":..}` when it must stop.
//!   The reply waits while the session is held.
//! - `{"op":"status"}` gets the usage, the caps and the number waiting.

use crate::gate::{Config, Gate, Priority, Step};
use crate::usage::{self, Window};
use serde_json::{Value, json};
use std::{
    fs,
    io::{self, BufRead, BufReader, Read, Write},
    os::unix::{
        fs::PermissionsExt,
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    sync::{Arc, Condvar, Mutex},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const MAX_LINE: u64 = 64 * 1024;

struct Daemon {
    gate: Mutex<Gate>,
    wake: Condvar,
    cfg: Config,
    usage_path: PathBuf,
}

pub fn run(sock: &Path, usage_path: PathBuf, cfg: Config) -> io::Result<()> {
    if UnixStream::connect(sock).is_ok() {
        return Err(io::Error::other(format!(
            "a daemon already listens on {}",
            sock.display()
        )));
    }
    // Nothing answered, so any file left there is stale.
    match fs::remove_file(sock) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    let listener = UnixListener::bind(sock)
        .map_err(|e| io::Error::other(format!("bind {}: {e}", sock.display())))?;
    fs::set_permissions(sock, fs::Permissions::from_mode(0o600))?;
    eprintln!("agentnice: listening on {}", sock.display());

    let d = Arc::new(Daemon {
        gate: Mutex::default(),
        wake: Condvar::new(),
        cfg,
        usage_path,
    });
    for conn in listener.incoming() {
        let conn = conn?;
        let d = Arc::clone(&d);
        thread::spawn(move || {
            if let Err(e) = d.serve(&conn) {
                eprintln!("agentnice: client: {e}");
            }
        });
    }
    Ok(())
}

impl Daemon {
    fn serve(&self, conn: &UnixStream) -> io::Result<()> {
        let mut line = String::new();
        if BufReader::new(conn.take(MAX_LINE)).read_line(&mut line)? == 0 {
            return Ok(()); // a probe, such as a second daemon checking this one
        }
        let req: Value = serde_json::from_str(&line)
            .map_err(|e| io::Error::other(format!("bad request: {e}")))?;
        let reply = match req["op"].as_str() {
            Some("acquire") => match self.acquire(conn, &req)? {
                Some(reply) => reply,
                None => return Ok(()), // the client left while it waited
            },
            Some("status") => self.status(),
            _ => json!({ "error": "unknown op" }),
        };
        writeln!(&*conn, "{reply}")
    }

    fn acquire(&self, conn: &UnixStream, req: &Value) -> io::Result<Option<Value>> {
        let p = req["priority"]
            .as_str()
            .and_then(Priority::parse)
            .ok_or_else(|| io::Error::other("priority must be low, normal or high"))?;
        let paced = req["paced"].as_bool().unwrap_or(false);
        let mut gate = self.gate.lock().expect("gate lock poisoned");
        let mut t = gate.ticket(p, paced);
        loop {
            let now = now_ms();
            match gate.step(&self.cfg, &mut t, &self.usage(), now) {
                Step::Go => {
                    self.wake.notify_all();
                    return Ok(Some(json!({ "go": true })));
                }
                Step::Stop(w) => {
                    self.wake.notify_all();
                    return Ok(Some(json!({
                        "go": false, "window": w.name, "used": w.used, "until": w.resets_at,
                    })));
                }
                Step::Wait(until) => {
                    // ponytail: wakes at least once a second to see if the
                    // client left. Fine for a few sessions. Use poll() on
                    // the sockets if this ever serves hundreds.
                    let d = Duration::from_millis(until.saturating_sub(now).clamp(20, 1000));
                    gate = self
                        .wake
                        .wait_timeout(gate, d)
                        .expect("gate lock poisoned")
                        .0;
                    if client_left(conn) {
                        gate.leave(&t);
                        self.wake.notify_all();
                        return Ok(None);
                    }
                }
            }
        }
    }

    fn status(&self) -> Value {
        let gate = self.gate.lock().expect("gate lock poisoned");
        let usage: Vec<Value> = self
            .usage()
            .iter()
            .map(|w| json!({ "window": w.name, "used": w.used, "resets_at": w.resets_at }))
            .collect();
        json!({
            "usage": usage,
            "waiting": gate.waiting(),
            "caps": { "low": self.cfg.caps[0], "normal": self.cfg.caps[1], "high": self.cfg.caps[2] },
            "gap_ms": self.cfg.gap_ms,
            "max_wait_s": self.cfg.max_wait_s,
        })
    }

    /// A bad file is logged and treated as no data. Sessions then pass,
    /// which is better than holding them on data we cannot read.
    fn usage(&self) -> Vec<Window> {
        usage::read(&self.usage_path).unwrap_or_else(|e| {
            eprintln!("agentnice: usage: {e}");
            Vec::new()
        })
    }
}

/// True when the peer closed the socket. A hook that Claude Code killed
/// for its timeout must not keep a place in the line.
fn client_left(conn: &UnixStream) -> bool {
    if conn.set_nonblocking(true).is_err() {
        return true;
    }
    // The client sends nothing after its request, so this read only sees
    // the end of the stream.
    let r = (&*conn).read(&mut [0]);
    if conn.set_nonblocking(false).is_err() {
        return true;
    }
    match r {
        Ok(0) => true,
        Ok(_) => false,
        Err(e) => e.kind() != io::ErrorKind::WouldBlock,
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock is before 1970")
        .as_millis() as u64
}
