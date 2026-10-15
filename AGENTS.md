# agentnice

One quota scheduler for all Claude Code sessions on a machine. A daemon on a Unix socket holds low-priority sessions near the 5-hour and 7-day caps, and lets starts through one at a time. Claude Code hooks are the client. See README.md for the user view.

## Commands

`scripts/harness-check.sh` is the only place that lists the check commands. CI calls it too. Change commands there, not in CI or here.

```bash
scripts/harness-check.sh quick      # cargo fmt --check, clippy -D warnings, cargo test
scripts/harness-check.sh ci-linux   # quick plus --locked and a release build
cargo test --test cli               # the tests that run the binary
```

## Map

```
src/main.rs    CLI: daemon, hook, statusline, status. Paths and the hook client.
src/daemon.rs  socket server, one thread per client, one shared Gate
src/gate.rs    the cap and pace rule. Pure, no clock or I/O.
src/usage.rs   reads and writes abtop-rate-limits.json
tests/cli.rs   runs the real binary against a real daemon
docs/          research notes and the diagram
```

## Rules

1. Fail open. If the daemon is down or broken, the hook exits 0 with no output. Never block a session because agentnice failed. Never use exit code 2 in the hook.
2. Hook errors exit 1, which Claude Code treats as a non-blocking error.
3. Keep `abtop-rate-limits.json` in abtop's format. Write it with a temp file and a rename.
4. `gate.rs` takes the time as an argument. Keep it pure so tests can use fixed times.
5. A client that leaves while it waits must leave the line. The daemon checks at least once a second.

## Known failure patterns

- Test fixtures live under `CARGO_TARGET_TMPDIR`. Socket paths have a length limit (108 bytes on Linux, 104 on macOS), so keep fixture names short.
- If you copy files into the repo with `rsync -a`, it keeps old mtimes, and cargo may not rebuild. Use `rsync -c` without `-t`, or `touch` the files.
- A test that kills a waiting hook must hold it on a cap. A paced wait ends by itself within one gap, so it hides a broken leave check.
