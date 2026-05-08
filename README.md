# CCPilot

> Opinionated Windows launcher for Claude Code: multi-session manager with subscription quota monitor, customer feedback inbox, scripts, and per-project todos.

A single ~10 MB Windows executable for power users who run several Claude Code sessions in parallel and want a tray-resident dashboard instead of jumping between terminals.

**Status:** personal project, public for portfolio reasons. Windows-only. No support promised.

---

## Why

If you only ever run one Claude Code session at a time, you don't need this. CCPilot is built around a workflow where you have **3–10 sessions open across different projects**, each in its own Windows Terminal tab/window, and you keep losing track of which is which, which is stuck, which is finished, and how much of your weekly subscription quota you've already burned.

The CLI's own `/usage`, `/resume` pickers, and `wt` tab juggling work fine in isolation but don't compose well at scale.

---

## Features

- **Project + session list** — auto-discovered from `~/.claude/projects/`, with first-message preview, message counts, last-activity timestamps, starring, archiving, search, and time/state filters.
- **One-click resume / focus / close** — single-session projects show a play button; multi-session ones expand into a per-session list. Running sessions surface their actual `wt.exe` window via HWND tracking; clicking focuses it.
- **Session preview popover** — peek at the last few user messages of any session before resuming, by reading its `.jsonl` directly.
- **Custom scripts** (`.ccpscript`) per project — small PowerShell snippets you can run, view, edit, and AI-generate via Claude. Output captured back into the launcher.
- **Customer feedback inbox** per project — title + description + screenshot/file attachments, each entry stored as a folder with `feedback.md`. Right-click → "Send to Claude" spawns a session with the feedback as the first prompt. Workflow states: `pending` / `sent` / `applied` / `ignored`.
- **Per-project todos + global todos window** — drag-reorderable, with notes-in-modal editing. Global todos lives in a separate window with multi-tab independent lists.
- **Desktop notifications** when a running session finishes.
- **System tray** — closing the main window minimizes to tray; right-click for `Quit`.
- **Status bar at the bottom of the main window:**
  - **App uptime** (live ticker)
  - **Node version + nvm switcher** — clickable menu listing all `nvm list` versions; picking one runs `nvm use`
  - **Proxy latency** (every 30 s, `curl https://www.gstatic.com/generate_204` through your local proxy)
  - **Proxy IP / country / region** (every 30 min, `curl https://ipinfo.io/json` through your local proxy)
  - **Claude Code subscription quota** — `5h: X% · 7d: Y%`, the same numbers you see in `/usage`. See [How quota monitoring works](#how-quota-monitoring-works) below.

---

## Install

### Pre-built (recommended)

Grab the latest from the [Releases page](../../releases). Pick one:

- `ccpilot.exe` — single-file portable, double-click to run.
- `CCPilot_*_x64-setup.exe` — NSIS installer, registers Start Menu entry.
- `CCPilot_*_x64_en-US.msi` — MSI installer.

Requires **WebView2 Runtime** (preinstalled on Windows 11; most updated Windows 10 too).

### From source

```bash
git clone git@github.com:binfenhulian/cc-pilot.git
cd cc-pilot
npm install
npm run tauri build
```

Output: `src-tauri/target/release/ccpilot.exe`.

> **Note on `rustc` version:** `rustc 1.94.1` has a Windows-specific stack/heap regression that crashes when compiling the `windows` and `tauri-utils` crates. If you hit `STATUS_STACK_BUFFER_OVERRUN` or weird "Sized not in scope" errors, pin to `1.92.0`:
> ```bash
> rustup install 1.92.0
> rustup override set 1.92.0  # run inside src-tauri/
> ```

---

## How quota monitoring works

Claude Code passes a `rate_limits` field (the same data backing `/usage`) on stdin to any registered statusline command. CCPilot ships a tiny ~30-line Node shim that:

1. Reads stdin
2. Writes `five_hour` + `seven_day` percentages and reset times to `%LOCALAPPDATA%\CCPilot\usage-snapshot.json`
3. Outputs nothing to stdout (so your `claude` prompt looks unchanged)

Click **📊 enable usage** in the status bar — CCPilot writes the shim and adds itself as `statusLine` in `~/.claude/settings.json`. Right-click the same pill to disable (cleanly removes the entry).

The snapshot updates every time Claude Code renders its statusline, which is roughly once per assistant turn. CCPilot polls the file every 30 s and shows "stale" if the snapshot is older than 10 minutes (i.e., no Claude session has been active recently).

**Caveat:** if you already use [claude-hud](https://github.com/jarrodwatts/claude-hud) or another statusline tool, enabling CCPilot's monitor will overwrite that config. Chaining into a pre-existing statusline is not implemented.

---

## Configuration

Most things are zero-config. The few things that aren't:

- **Proxy** — currently hardcoded to `127.0.0.1:10809` (v2ray HTTP default). Port `10808` is auto-detected as SOCKS5. Edit `src/routes/+page.svelte` if you need different defaults; a settings UI is not implemented.
- **Project root** — sessions and projects come from `~/.claude/projects/` (Claude Code's default). Custom paths are not supported.

---

## Tech stack

- **[Tauri 2](https://tauri.app/)** — Rust backend, WebView2 frontend, single-file binary
- **[SvelteKit](https://kit.svelte.dev/) (SPA mode) + Svelte 5** — `$state` runes, `adapter-static`
- **TypeScript** + plain CSS (no Tailwind, no UI library)
- **Lucide-style SVG icons** via a single `Icon.svelte` component

---

## What's intentionally missing

- macOS / Linux builds. Won't be added unless someone ports it.
- Unit tests. There aren't any.
- Settings UI. Most config is in code.
- Session search by message content. Only first-message search.
- Multi-user / team features. Single-user tool by design.
- Internationalization. Mixed English UI / Chinese comments.

---

## License

MIT. Use it, fork it, ship it. PRs welcome but unlikely to be reviewed quickly — see *Status* above.
