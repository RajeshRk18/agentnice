# agentnice

One quota scheduler for all the Claude Code sessions on your machine.

## Why

Say you run five Claude Code sessions at once. They all share one 5-hour quota and one 7-day quota, but no session knows about the others. A side job can use up the quota your main work needs.

There is a second problem. When the 5-hour window resets, every session that was waiting starts at the same moment. The first few work. The rest fail with "Server is temporarily limiting requests" ([claude-code#53922](https://github.com/anthropics/claude-code/issues/53922)).

## What it does

- You give each session a priority: `low`, `normal` or `high`.
- A low session stops at 80% of a window. A normal session stops at 95%. A high session runs to 100%.
- A stopped session waits inside its hook. When the window resets, it goes on by itself.
- If the reset is more than 5 hours 10 minutes away (a 7-day cap), the session ends. The message tells you when the quota resets.
- New sessions, and sessions that waited, start one at a time, 3 seconds apart. High priority goes first.
- If the daemon is not running, the hook does nothing. agentnice never blocks your work because it is broken.

## Set it up

```bash
cargo install --path .
agentnice daemon            # keep this running, for example in a systemd user unit
```

Add this to `~/.claude/settings.json`:

```json
{
  "statusLine": { "type": "command", "command": "agentnice statusline" },
  "hooks": {
    "SessionStart":     [{ "hooks": [{ "type": "command", "command": "agentnice hook", "timeout": 21600 }] }],
    "UserPromptSubmit": [{ "hooks": [{ "type": "command", "command": "agentnice hook", "timeout": 21600 }] }],
    "PreToolUse":       [{ "hooks": [{ "type": "command", "command": "agentnice hook", "timeout": 21600 }] }]
  }
}
```

The `timeout` is 6 hours, so a hook can wait out a 5-hour window. Claude Code kills a hook at its timeout and then lets the session go on.

If you use [abtop](https://github.com/graykode/abtop), keep its status line instead of `agentnice statusline`. Both write the same file, `~/.claude/abtop-rate-limits.json`, so abtop and agentnice see the same numbers.

Then start each session with a priority:

```bash
AGENTNICE_PRIORITY=low claude      # a side job
claude                             # normal
AGENTNICE_PRIORITY=high claude     # the work that matters
```

## Commands

| Command | What it does |
| --- | --- |
| `agentnice daemon [--low PCT] [--normal PCT] [--gap SECS] [--max-wait SECS]` | Runs the scheduler. Defaults: 80, 95, 3, 18600. |
| `agentnice hook` | The hook. Reads the hook event from stdin. |
| `agentnice statusline` | The status line. Saves the quota numbers and prints `5h 42% · 7d 13%`. |
| `agentnice status` | Prints what the daemon sees: usage, caps, and how many sessions wait. |

The daemon and the hook find each other through `agentnice.sock` in `$CLAUDE_CONFIG_DIR` (default `~/.claude`). Each config dir is one account, so each one gets its own daemon. `AGENTNICE_SOCK` sets another socket path.

## How it works

See [docs/agentnice.html](docs/agentnice.html) for a diagram.

1. Claude Code runs `agentnice statusline` and gives it the quota numbers. It writes them to the shared file.
2. Before a session starts, before each prompt, and before each tool call, Claude Code runs `agentnice hook`.
3. The hook asks the daemon for a turn and waits for the answer.
4. The daemon reads the shared file. If this session's priority is over its cap, the answer waits until the window resets. Starts are let through one at a time.
5. If the hook gets killed while it waits, the daemon sees the closed socket within a second and drops it from the line.

## Limits

- The hook runs before prompts and tool calls, not before each model call. A session over its cap can make one more model call before its next tool call is held.
- The numbers come from the status line, so they are only fresh while some session runs.
- Claude Code gives these numbers only to Pro and Max plans, and only after a session's first reply.
- A stop at `SessionStart` does not end the session. It ends at the next prompt or tool call.
- The status line has no per-model weekly limits (Opus, Sonnet), so agentnice cannot see them.

## Development

`scripts/harness-check.sh quick` runs fmt, clippy and the tests. See [AGENTS.md](AGENTS.md).
