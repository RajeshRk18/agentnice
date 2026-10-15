# agentnice research

Date: 2026-10-15. Primary sources only. "Not verified" means no primary source was found.

## 1. Rate-limit data that Claude Code exposes locally

### Facts

- The statusline command gets JSON on stdin. It has a `rate_limits` object. [statusline](https://code.claude.com/docs/en/statusline)
- Shape, from the doc example:
  ```json
  "rate_limits": {
    "five_hour": { "used_percentage": 23.5, "resets_at": 1738425600 },
    "seven_day": { "used_percentage": 41.2, "resets_at": 1738857600 },
    "spend_limit": { "used_percentage": 62.8, "resets_at": 1740787200,
                     "used_usd": 314.12, "limit_usd": 500, "period": "monthly" }
  }
  ```
- `used_percentage` is a number from 0 to 100. `resets_at` is "Unix epoch seconds". [statusline](https://code.claude.com/docs/en/statusline)
- `rate_limits` is present only for claude.ai Pro and Max subscribers (or behind a Claude apps gateway with a spend limit), and only after the first API response in the session. Each window can be absent. Claude Code drops a window after its `resets_at` time passes. [statusline](https://code.claude.com/docs/en/statusline)
- The statusline script runs on events (debounced 300 ms), on `refreshInterval` (seconds, min 1), and when a window reaches `resets_at`. A new update cancels a script that still runs. [statusline](https://code.claude.com/docs/en/statusline)
- The statusline JSON also has `session_id` and `transcript_path`. [statusline](https://code.claude.com/docs/en/statusline)
- Per-model weekly windows (Opus/Sonnet/Fable) are not in the statusline JSON. An open feature request asks for them. [#73770](https://github.com/anthropics/claude-code/issues/73770), [#92080](https://github.com/anthropics/claude-code/issues/92080)
- OAuth usage endpoint: `GET https://api.anthropic.com/api/oauth/usage` with `Authorization: Bearer <token>` and `anthropic-beta: oauth-2025-04-20`. It is undocumented by Anthropic. Source is a third-party plugin only. [claude-quotas lib.ts](https://github.com/FruityMaxine/claude-quotas/blob/main/src/lib.ts)
- Its response fields (per that plugin): `five_hour`, `seven_day`, `seven_day_opus`, `seven_day_sonnet`, each `{ utilization: number, resets_at: string | null }` (ISO string, not epoch). Also `extra_usage { is_enabled, monthly_limit, used_credits, utilization, currency }`. [claude-quotas lib.ts](https://github.com/FruityMaxine/claude-quotas/blob/main/src/lib.ts). Not verified against an Anthropic source.
- OAuth token location: macOS Keychain. Fallback and Linux: `~/.claude/.credentials.json` (mode 0600). `CLAUDE_CONFIG_DIR` changes both. [authentication](https://code.claude.com/docs/en/authentication)
- The plugin reads `claudeAiOauth.accessToken`, `expiresAt`, `subscriptionType`, `rateLimitTier` from `.credentials.json`. It does not read the macOS Keychain. [claude-quotas lib.ts](https://github.com/FruityMaxine/claude-quotas/blob/main/src/lib.ts)
- Headers: Claude Code reads `anthropic-ratelimit-unified-*` response headers into an internal `unifiedWindows` schema. `rateLimitType` values: `five_hour`, `seven_day`, `seven_day_opus`, `seven_day_sonnet`, `seven_day_overage_included`, `overage`. [#92080](https://github.com/anthropics/claude-code/issues/92080). The exact header names are not verified.
- Claude Code tells "Server is temporarily limiting requests" apart from a plan limit "by the absence of the unified quota headers". Since v2.1.199 it retries this error with backoff. [errors](https://code.claude.com/docs/en/errors#server-is-temporarily-limiting-requests)
- No official file under `~/.claude` holds quota data. Not verified that one exists.

## 2. Hook contract

### Facts

- Common stdin fields: `session_id`, `prompt_id`, `transcript_path`, `cwd`, `scratchpad_dir`, `permission_mode`, `effort.level`, `hook_event_name`, `agent_id`, `agent_type` (last two only in subagents). [hooks](https://code.claude.com/docs/en/hooks#common-input-fields)
- PreToolUse adds `tool_name`, `tool_input`, `tool_use_id`. [hooks](https://code.claude.com/docs/en/hooks)
- Exit 0: success, JSON output is applied. Exit 2: blocks the action; stderr goes to Claude as the deny reason. Other codes: non-blocking error, the action proceeds. [hooks](https://code.claude.com/docs/en/hooks#exit-code-output)
- JSON output: `continue` (default `true`; `false` stops Claude), `stopReason` (shown to user when `continue` is false), `systemMessage`, `suppressOutput` (no effect). [hooks](https://code.claude.com/docs/en/hooks#json-output)
- `hookSpecificOutput.permissionDecision`: `"allow"`, `"deny"`, `"ask"`, `"defer"`. Also `permissionDecisionReason`, `updatedInput`, `additionalContext`. Precedence: deny > defer > ask > allow. [hooks](https://code.claude.com/docs/en/hooks#pretooluse-decision-control)
- `"defer"` works only with `claude -p`. In interactive sessions Claude Code logs a warning and ignores it. [hooks](https://code.claude.com/docs/en/hooks#defer-a-tool-call-for-later)
- `timeout` unit is seconds. Default 600 for `command`/`http`/`mcp_tool`. 30 on `UserPromptSubmit`. SessionEnd hooks share a 1.5 s budget (up to 60 s if a hook sets a longer `timeout`). [hooks](https://code.claude.com/docs/en/hooks#common-fields)
- A timed-out PreToolUse command hook does NOT block the tool. The call continues through normal permissions. Set `"onFailure": "block"` (v2.1.295+) to block on fail or timeout. [hooks](https://code.claude.com/docs/en/hooks#timeouts)
- So a hook can sleep up to its `timeout`. The doc gives no maximum for `timeout`. Not verified that values far above 600 work.
- All matching hooks run in parallel. `async: true` runs in the background and Claude Code does not enforce `timeout` on it. [hooks](https://code.claude.com/docs/en/hooks)
- SessionStart: `source` = `startup` | `resume` | `clear` | `compact` | `fork`; optional `model`. [hooks](https://code.claude.com/docs/en/hooks#sessionstart-input)
- SessionStart has no blocking or decision control. But it runs in the background and "Claude's first response still waits for the hooks to finish". So a slow SessionStart hook delays the first API call. [hooks](https://code.claude.com/docs/en/hooks)
- SessionEnd: `reason` = `clear` | `resume` | `logout` | `prompt_input_exit` | `other`. No decision control. [hooks](https://code.claude.com/docs/en/hooks#sessionend)
- Stop: `stop_hook_active`, `last_assistant_message`, `background_tasks`, `session_crons`. [hooks](https://code.claude.com/docs/en/hooks#stop-input)
- UserPromptSubmit: `prompt`, `source`. Can block with top-level `decision: "block"` + `reason`. [hooks](https://code.claude.com/docs/en/hooks#userpromptsubmit)
- All of these events carry `session_id` (common field). [hooks](https://code.claude.com/docs/en/hooks#common-input-fields)

## 3. Issue anthropics/claude-code#53922

### Facts

- Title: "[BUG] Parallel Claude Code sessions started right after 5-hour limit resets — first 3–4 work, the rest fail with "Server is temporarily limiting requests (not your usage limit) · Rate limited"". [#53922](https://github.com/anthropics/claude-code/issues/53922)
- Opened 2026-04-27 by @NowatariSoma. Labels: `area:api`, `bug`, `external`, `has repro`, `platform:linux`, `stale`. [#53922](https://github.com/anthropics/claude-code/issues/53922)
- Status: closed as "not planned" on 2026-07-05 (stale). Locked 2026-09-07. No fix. [#53922](https://github.com/anthropics/claude-code/issues/53922)
- Key quote: "bulk-spawning ~10 Claude Code sessions back-to-back via a script ... the first 3–4 sessions start normally, but the next 5–6 sessions fail almost immediately with: `API Error: Server is temporarily limiting requests (not your usage limit) · Rate limited`". [#53922](https://github.com/anthropics/claude-code/issues/53922)
- Commenters (not Anthropic) say it looks like a per-account concurrent-stream cap. It does not scale with plan tier. [#53922](https://github.com/anthropics/claude-code/issues/53922)
- One commenter saw the retry signal as `x-should-retry: true`, not `Retry-After`, and saw `anthropic-ratelimit-*` headers missing on error responses. [#53922](https://github.com/anthropics/claude-code/issues/53922)
- Workaround in comments: stagger spawns by 2–5 s, or wait for session N to be ready before N+1. [#53922](https://github.com/anthropics/claude-code/issues/53922)

## 4. "Claude Quotas" plugin

### Facts

- Repo: [FruityMaxine/claude-quotas](https://github.com/FruityMaxine/claude-quotas). A Claude Code plugin with an MCP server that exposes one tool, `check_quota`. [README](https://github.com/FruityMaxine/claude-quotas/blob/main/README.md)
- Data: reads the OAuth token from `~/.claude/.credentials.json` (or `CLAUDE_CONFIG_DIR`), then calls `GET https://api.anthropic.com/api/oauth/usage`. [lib.ts](https://github.com/FruityMaxine/claude-quotas/blob/main/src/lib.ts)
- Pause method: the model pauses itself. A skill tells Claude to wrap up, commit, write `docs/progress/quota-resume.md`, then call `ScheduleWakeup` with `delaySeconds` = seconds until reset (capped at 3600 s, up to 6 relay segments). It does not use a hook to block tools. [README](https://github.com/FruityMaxine/claude-quotas/blob/main/README.md)
- 5-hour sleep thresholds by `rateLimitTier`: Pro >= 95%, Max 5x >= 98%, Max 20x >= 99%. [SKILL.md](https://github.com/FruityMaxine/claude-quotas/blob/main/skills/check-quota/SKILL.md)
- A `SessionStart` hook (matcher `resume`, `timeout` 5) re-fetches usage and injects it as `additionalContext`. [hooks.json](https://github.com/FruityMaxine/claude-quotas/blob/main/hooks/hooks.json), [hook-session-start.ts](https://github.com/FruityMaxine/claude-quotas/blob/main/src/hook-session-start.ts)
- On macOS the token is normally in the Keychain, not in `.credentials.json` ([authentication](https://code.claude.com/docs/en/authentication)). So this plugin may find no token on macOS. Not tested.

## 5. abtop

### Facts

- Repo: [graykode/abtop](https://github.com/graykode/abtop). Rust. "Like htop, but for AI coding agents". Monitors Claude Code and Codex CLI sessions, tokens, context, rate limits, ports.
- Source of Claude rate limits: a statusline script that `abtop --setup` installs at `~/.claude/abtop-statusline.sh` and sets as `statusLine.command` in `settings.json`. Setup fails if another `statusLine` exists. [setup.rs](https://github.com/graykode/abtop/blob/main/src/setup.rs)
- The script copies `rate_limits.five_hour` and `rate_limits.seven_day` from stdin into `$CLAUDE_CONFIG_DIR/abtop-rate-limits.json` (default `~/.claude/abtop-rate-limits.json`). [setup.rs](https://github.com/graykode/abtop/blob/main/src/setup.rs)
- File format:
  ```json
  {"source":"claude","updated_at":<epoch secs>,
   "five_hour":{"used_percentage":<f64>,"resets_at":<epoch secs>},
   "seven_day":{"used_percentage":<f64>,"resets_at":<epoch secs>}}
  ```
  The reader also accepts optional `window_minutes` (default 300 and 10080). [rate_limit.rs](https://github.com/graykode/abtop/blob/main/src/collector/rate_limit.rs)
- The reader checks `~/.claude`, `$CLAUDE_CONFIG_DIR`, and config dirs found from running Claude processes. [rate_limit.rs](https://github.com/graykode/abtop/blob/main/src/collector/rate_limit.rs)
- The installed script writes the file with a plain `open(..., 'w')`, not an atomic rename. [setup.rs](https://github.com/graykode/abtop/blob/main/src/setup.rs)
- The older `scripts/abtop-statusline.sh` reads `rate_limits.session` and `rate_limits.weekly`. These keys are not in the Claude Code doc. Only the `setup.rs` script matches the doc. [abtop-statusline.sh](https://github.com/graykode/abtop/blob/main/scripts/abtop-statusline.sh)

## Design implications for agentnice

1. Use the statusline `rate_limits` (epoch-second `resets_at`) as the main source. Read or write `~/.claude/abtop-rate-limits.json` in the same format, so abtop and agentnice share one file. Write it with temp file + rename.
2. Use `/api/oauth/usage` only as an optional fallback. It is undocumented, uses ISO `resets_at`, and needs the macOS Keychain token.
3. A PreToolUse hook gates tools, not model calls. To hold a session at reset, block in the hook (set `timeout` high and `"onFailure": "block"`), or deny with exit 2. `"defer"` does not work in interactive sessions.
4. For the burst after a reset (#53922), the daemon should release tokens one at a time with a 2–5 s gap. The first API call comes before any tool call, so also wait in `SessionStart` (the first response waits for it) and `UserPromptSubmit` (raise its 30 s `timeout`).
5. Use `session_id` from every hook to track holders. Release tokens on `Stop` and `SessionEnd` (1.5 s budget, so the client must be fast), and expire tokens with a lease in case a session dies.
