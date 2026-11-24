<div align="center">
  <img src="assets/logo.svg" width="96" height="96" alt="bbstats logo" />
  <h1>bbstats</h1>
  <p><strong>Dynamic Island background process &amp; developer project telemetry monitor</strong></p>
  <p>
    <strong>English</strong> · <a href="README.tr.md">Türkçe</a> · <a href="CHANGELOG.md">Changelog</a> · <a href="SECURITY.md">Security</a> · <a href="CONTRIBUTING.md">Contributing</a> · <a href="https://kreosus.com/f0rextasy">Support on Kreosus</a>
  </p>
  <p>
    <img src="https://img.shields.io/badge/Rust-1.88%2B-orange?logo=rust" alt="Rust 1.88+" />
    <img src="https://img.shields.io/badge/Windows-10%2F11%20Native-0A84FF?logo=windows" alt="Windows 10/11" />
    <img src="https://img.shields.io/badge/macOS-arm64%20%26%20x86__64-34D399?logo=apple" alt="macOS Supported" />
    <img src="https://img.shields.io/badge/Linux-Terminal%20HUD-34D399?logo=linux" alt="Linux Supported" />
    <img src="https://img.shields.io/badge/License-MIT-34D399" alt="MIT License" />
    <a href="https://kreosus.com/f0rextasy"><img src="https://img.shields.io/badge/Kreosus-Support-E91E63" alt="Support on Kreosus" /></a>
  </p>
</div>

---

**bbstats** is a lightweight, **100% local (zero telemetry, zero cloud connections)** Dynamic Island overlay and developer process monitor with zero runtime dependencies. Instead of drowning you in hundreds of raw OS PIDs like Task Manager or Activity Monitor, it groups your active servers, workers, containers, and scheduled jobs by **project** and **logical workload**—showing live CPU/RAM curves, bound ports, log streams, crash alerts, and one-click safe controls.

> **100% Local & Privacy-First:** `bbstats` never phones home, sends analytics, or uploads snapshots. All inspection stays on your machine (Docker queries use only the local pipe/socket when Docker is running).

## Download & Install

### Prebuilt Binaries (GitHub Releases)

Download the latest archive for **Windows (`x86_64`)**, **macOS (`arm64` / `x86_64`)**, or **Linux (`x86_64`)** from [GitHub Releases](https://github.com/F0Rextasy/bbstats/releases). Every release ships with `SHA256SUMS.txt` and GitHub build-provenance attestations (`gh attestation verify`).

> **Windows SmartScreen Note:** Release binaries are currently unsigned by an EV/organization certificate, so Windows SmartScreen may show an initial prompt on first launch; verify the archive hash against `SHA256SUMS.txt` before running.

### Install from Source (`cargo`)

```bash
cargo install --git https://github.com/F0Rextasy/bbstats.git --locked
```

## Screenshots

<div align="center">
  <p><strong>Collapsed Pill &amp; Hover Peek</strong></p>
  <img src="assets/screenshots/en/01-collapsed-pill.png" alt="bbstats collapsed pill" />
  <br /><br />
  <img src="assets/screenshots/en/02-hover-peek.png" alt="bbstats hover peek" />
</div>

<br />

<div align="center">
  <table>
    <tr>
      <td align="center" width="50%">
        <strong>Expanded Panel, Git Branches &amp; Live Sparklines</strong><br />
        <img src="assets/screenshots/en/03-expanded-panel.png" alt="Expanded panel" width="100%" />
      </td>
      <td align="center" width="50%">
        <strong>Job Detail, Logs &amp; Job-Scoped Subprocesses</strong><br />
        <img src="assets/screenshots/en/04-detail-terminal.png" alt="Detail terminal view" width="100%" />
      </td>
    </tr>
    <tr>
      <td align="center" width="50%">
        <strong>Crash Alert &amp; Status Ambient Glow</strong><br />
        <img src="assets/screenshots/en/05-crash-alert.png" alt="Crash alert and red glow" width="100%" />
      </td>
      <td align="center" width="50%">
        <strong>Settings (v0.4.0, EN / TR, WSL2 &amp; Thresholds)</strong><br />
        <img src="assets/screenshots/en/06-settings.png" alt="Settings view" width="100%" />
      </td>
    </tr>
  </table>
</div>

---

## Why bbstats? (vs. Task Manager & Classic Tools)

| Feature / Workflow | Windows Task Manager / Classic Monitors | **bbstats** |
|---|---|---|
| **Primary View** | Hundreds of flat raw PIDs (`node.exe`, `python.exe`, `cmd.exe`) | **Projects & Logical Jobs:** Resolves `package.json`, `Cargo.toml`, `pyproject.toml`, `.git`, Docker Compose (`acme-platform › api-gateway`) |
| **Wrapper Noise** | `cmd.exe`, `conhost.exe`, and venv shims clutter separate rows | Collapses parent wrappers and helper trees into a single job row (`×2`, `×3`) |
| **Bound Ports (`:port↗`)** | Requires manual `netstat -ano` correlation | Inline clickable `:port↗` chips right on every job (left-click opens browser, right-click copies URL) |
| **Crash Recovery** | Crashed processes vanish silently | Keeps `"Crashed"` card visible with exit code, fires notification, supports one-click `↻ Restart` and bounded `auto_restart` |
| **Live Logs** | Must hunt for buried terminal windows or open log files manually | Built-in live log, pipe, console buffer, and state-JSON viewer with `INFO`/`WARN`/`ERROR` highlighting |
| **Desktop Footprint** | Heavy standalone window requiring `Alt+Tab` | Always-accessible **Dynamic Island** capsule (top-center or right rail), auto-hides in fullscreen |

---

## Key Features

- **Dynamic Island Ergonomics:** Critically-damped spring physics (`60–120fps`), cursor-tracking top-rim specular reflection, status-aware top ambient glow (green/amber/crimson), and cascaded list/tab micro-transitions.
- **Project & Job Grouping:** Automatically resolves workspace roots (`package.json`, `Cargo.toml`, `pyproject.toml`, `.git`, Docker Compose) and collapses wrapper/trampoline processes (`cmd`, `conhost`, `venv` shims) into clean readable jobs.
- **Live Telemetry & Ports:** Per-project CPU/RAM sparklines with smoothed Catmull-Rom cubic Bezier curves, stacked project RAM allocation bar, clickable `:port↗` chips (left-click opens browser, right-click copies URL).
- **Terminal & Log Inspection:** Lazy tailing of live managed pipes, log files, CLI `stdout` redirects, structured state JSON files, or classic console buffers with severity highlighting.
- **Crash Detection & Bounded Auto-Restart:** Instant toast/banner notifications on unexpected non-zero exit codes, manual one-click `↻ Restart`, and optional bounded `auto_restart` (max 3 attempts per 10 minutes).
- **Privacy & Safety First:** Built-in CLI secret redaction (`--token`, `--api-key`, basic auth URLs) and strict process blacklist protection so system/self processes can never be killed accidentally.

---

## Platform Support

| Platform | Status | Interface & Rendering | Telemetry & Autostart |
|---|---|---|---|
| **Windows 10 / 11 (`x86_64`)** | Supported | Direct3D11 + Direct2D + DirectWrite + WinRT Composition Dynamic Island overlay (`WS_EX_TOOLWINDOW`, Acrylic backdrop) | NT PEB inspection, IP Helper TCP/UDP ports, Tray `Shell_NotifyIconW`, Registry Run |
| **macOS (`arm64` / `x86_64`)** | Supported | Cross-platform live ANSI terminal HUD (`bbstats`, `--watch`, `--status`, `--daemon`) + CLI tools | `ps` / `sysctl` / `vm_stat` / `lsof` port & CWD telemetry, `kqueue` exit watcher, Docker Unix socket, `~/Library/LaunchAgents` (`--autostart`) |
| **Linux (`x86_64`)** | Supported | Cross-platform live ANSI terminal HUD (`bbstats`, `--watch`, `--status`, `--daemon`) + CLI tools | `/proc` CPU/RAM/CMD/CWD + `lsof` port telemetry, Docker socket, XDG `.desktop` autostart (`--autostart`) |

---

## Build & Run

**Prerequisites:** Rust toolchain (`>= 1.88`). No Node/Electron or external runtime required (`build.rs` embeds the **`bb`** `.ico` & Win32 resources on Windows automatically).

### Windows

```powershell
# Run clippy checks & unit tests
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features --no-fail-fast

# Build optimized release executable (target\release\bbstats.exe)
cargo build --release --locked
```

### macOS & Linux

```bash
# Run clippy checks & unit tests
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features --no-fail-fast

# Build & run live ANSI terminal HUD
cargo run --release -- --watch

# Single-shot overview, headless daemon, or login autostart toggle
./target/release/bbstats --status
./target/release/bbstats --daemon
./target/release/bbstats --autostart on
```

---

## Developer CLI Commands & Diagnostic Flags

Run commands, inspect occupied ports, launch project job stacks, or inspect snapshots directly from the terminal without disturbing a running desktop overlay:

```powershell
# Run a command inheriting your shell environment while streaming live stdout/stderr
bbstats run -- npm run dev
bbstats run --cwd ./api -- cargo run

# Find which project, job, PID, and command is listening on a port (and optionally stop it safely)
bbstats port 3000
bbstats port 3000 --kill

# Discover and launch jobs from bbstats.toml, Procfile, or package.json scripts
bbstats launch --list
bbstats launch ./my-workspace

# Print live detected projects, jobs, ports, and last log lines to stdout
bbstats --dump-snapshot

# Single-shot formatted HUD table, daemon mode, or autostart toggle (macOS & Linux)
bbstats --status
bbstats --daemon
bbstats --autostart on

# Print version or CLI usage flags
bbstats --version
bbstats --help

# Render deterministic clean showcase screenshots to out\shots (--lang en | tr, Windows)
cargo run --features screenshots -- --dump-shot out\shots --lang en

# Render screenshots using your live running processes (Windows)
cargo run --features screenshots -- --dump-shot out\shots --live
```

---

## Configuration

### `config.toml`
- **Windows:** `%APPDATA%\bbstats\config.toml`
- **macOS:** `~/Library/Application Support/bbstats/config.toml`
- **Linux:** `~/.config/bbstats/config.toml`

Managed via the in-app **Settings** panel (Windows) or directly in TOML:

```toml
language = "en"                 # "en" | "tr"
dock_position = "top_center"    # "top_center" | "right_edge"
monitor_index = 0
always_on_top = true
acrylic_enabled = true
idle_opacity = 0.14             # 0.0 (transparent HUD) .. 0.95
glass_opacity = 0.88            # 0.25 .. 0.98
hide_on_fullscreen = true
notify_on_crash = true
hover_peek_delay_ms = 190
autostart = false
hotkey = "alt_shift_b"          # "alt_shift_b" | "ctrl_alt_b" | "alt_space" | "ctrl_shift_p" | "disabled"
sort_mode = "custom"            # "custom" | "cpu" | "ram" | "name" | "uptime"
pinned_projects = []
hidden_projects = []
```

### Custom Project Rules (`labels.toml`)
- **Windows:** `%APPDATA%\bbstats\labels.toml`
- **macOS:** `~/Library/Application Support/bbstats/labels.toml`
- **Linux:** `~/.config/bbstats/labels.toml`

Define custom naming rules, JSON PID/state file bindings (`pid_file`), workspace directory scanners (`scan_dir`), scheduled task / LaunchAgent monitors (`scheduled_task`), and bounded `auto_restart` policies using [`assets/labels.example.toml`](assets/labels.example.toml).

---

## Keyboard & Mouse Shortcuts

*(Shortcuts inside the Dynamic Island overlay require the panel to be active/focused via global hotkey or click, so they never intercept shortcuts inside your editor or terminal.)*

| Shortcut / Action | Behavior |
|---|---|
| `Alt+Shift+B` *(default)* | Toggle Dynamic Island expand / collapse (switchable in Settings: `Ctrl+Alt+B`, `Alt+Space`, `Ctrl+Shift+P`, `Disabled`) |
| `↑` / `↓` + `Enter` or `→` | Navigate job rows / Open Detail view (`Enter` also confirms an active kill bar) |
| `Esc` / `←` / `Backspace` *(Detail/Settings)* | Clear search / Back to overview / Collapse |
| `/` | Focus search bar |
| `Alt+P` / `Alt+K` | Pin/unpin selected project / Request stop confirmation for selected job |
| `Delete` (on selected row) | Request stop confirmation for selected job |
| `Ctrl+Backspace`, `Ctrl+A`, `Ctrl+U`, `Ctrl+C`, `Ctrl+V` | Fast text editing in Search, Rename, and Add Job inputs |
| `:port↗` Left click / Right click | Open URL in browser / Copy URL to clipboard |
| Right click project or job row | Inline rename saved to `labels.toml` |

---

## Architecture Overview

- `src/platform/` — Borderless overlay window loop, system tray **`bb`** icon & crash balloons, PerMonitorV2 DPI & monitor geometry, fullscreen detection, global hotkey.
- `src/engine/` — Single background telemetry thread (`collector`, `win_nt`), project & job inference pipeline (`project_resolver/`), user rules & watchers (`labels`, `known_jobs`), snapshot builder (`snapshot`), log tailing (`terminal`), and safe termination/restart (`actions`).
- `src/render/` — Direct3D11 + Direct2D1 + DirectWrite + WinRT Composition pipeline, theme tokens (`theme`), glass/Bezier primitives (`primitives`), and offscreen PNG exporter (`shot`).
- `src/ui/` — Critically-damped springs (`spring`), viewport/stagger math (`layout`), hit regions (`hit_test`), and views (`views/`).
- `src/cli.rs` — Developer CLI subcommands (`run`, `port`, `launch`).

---

## Contributing, Security & License

Contributions across Windows, macOS, and Linux are warmly welcome! Please read [`CONTRIBUTING.md`](CONTRIBUTING.md) before opening a PR, and [`SECURITY.md`](SECURITY.md) for local privacy guarantees and vulnerability reporting.

See the release history in [`CHANGELOG.md`](CHANGELOG.md). Licensed under the [MIT License](LICENSE).

If `bbstats` helps your daily workflow, you can support ongoing open-source development on [Kreosus](https://kreosus.com/f0rextasy).
