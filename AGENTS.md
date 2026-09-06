# AGENTS.md

Instructions for coding agents working in this repository. Human-facing product docs live in `README.md` and `README.zh-CN.md`. This file is the contract for how to change the code.

## What this repo is

Never Sleep (`never-sleep`) keeps a MacBook **awake with the display off** so remote clients (ChatGPT / Codex and similar) can connect. The menu bar is the primary UI; the CLI talks to the same process over a Unix socket, or occupies the foreground when no menu bar is running.

Hard product rules (do not weaken these):

1. One-click start sleeps the display after ~1.5s; the system stays running.
2. Never fight a person at the keyboard. HID idle + lid state decide “user present”.
3. Remote / synthetic input waking the panel is fine; reassert display sleep while the user is away.
4. Always provide a way back: `⌥⌘P` and the menu.
5. **Do not rewrite Energy Saver.** Only in-process IOKit assertions. No `pmset disablesleep`. Quit / crash / relaunch must restore clamshell flags.
6. Agent contract: `never-sleep status --json` field names stay English and stable. `never-sleep on --for 8h` must keep working.

JSON output, CLI flag names, and IPC `cmd` tags are an Agent API. Changing them is a breaking change.

## Layout

```
crates/never-sleep-core   policy only (Engine, config, i18n, duration). Unit-tested on Linux.
crates/never-sleep        CLI + macOS menu bar + IOKit platform layer.
packaging/                Info.plist, localizations, optional AppIcon.icns
scripts/                  package-macos.sh, bump-version.sh, icon generation
.github/workflows/ci.yml  fmt + clippy + tests on Linux; test/build/package on macOS
```

`never-sleep-core` must not depend on AppKit, IOKit, tao, or tray-icon. The engine only emits `Effect`s (`ApplyPower`, `ReleasePower`, `SleepDisplay`, `LockSession`, `Notify`). The platform crate applies them.

On Linux CI the binary uses `StubPlatform` (prints, does not touch power). Menu bar / IOKit / AppKit code is `#[cfg(target_os = "macos")]`.

## Menu-bar panel hotspot

`crates/never-sleep/src/native_panel.rs` is the AppKit panel (Liquid Glass via `NSGlassEffectView`, `NSVisualEffectView` fallback). Two open PRs that both touch it will conflict. Panel copy and navigation that Linux can lock live in `crates/never-sleep/src/panel.rs`.

Before editing `native_panel.rs` or popover wiring in `gui.rs`:

1. `git fetch origin main` and start from (or merge) the **latest remote** `main`. Cloud snapshots are often behind; do not skip this for UI work.
2. Do not run a second agent on the panel while another panel PR is still open. Land or rebase the first.
3. Keep `PanelView::Main` / `Settings` / `Help`. Centered coin, Start pill, More Settings | Quit. The grouped card lives on Settings (duration + switches). Glass belongs on the **panel chrome**; cards stay opaque. Anchor under the status item and hide on focus loss. Do not gray out End Standby or Sleep Display Now.
4. After merging `main` into an in-flight UI PR, re-apply only the intended delta.

## Toolchain

- Rust **1.88.0** (`rust-toolchain.toml`). Do not bump it casually; CI pins the same version.
- Edition 2021.
- Format with default `rustfmt`. Do not add rustfmt overrides unless a file cannot be formatted otherwise.

## Commands

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
./scripts/bump-version.sh --check
```

macOS only (GUI + IOKit):

```bash
cargo build -p never-sleep --release
./scripts/package-macos.sh
```

Do not run `scripts/package-macos.sh` on Linux; it exits by design.

A change is not done until **fmt, clippy (`-D warnings`), and `cargo test --workspace`** are green. Linux is enough to prove core policy. Do not claim macOS GUI/IOKit behaviour is verified unless you actually built on a Mac.

## TDD is required

This repository is developed **test-first**. Do not start with production code for a behaviour change.

### Cycle

1. **Red** — Write one failing test that names the behaviour (a user-visible rule, an Agent JSON field, a parser case, an effect the engine must emit). Run `cargo test --workspace` and confirm it fails for the *right* reason (assertion, not a compile error you then “fix” by deleting the test).
2. **Green** — Write the smallest production change that makes that test pass. No extra refactors in the same step.
3. **Refactor** — Clean up with tests green. Keep public JSON / IPC / CLI contracts stable.
4. **Gate** — `cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`.

### Where tests go

| Behaviour | Put the test in |
| --- | --- |
| Sleep / stay-awake / battery / thermal / duration / “user present” | `crates/never-sleep-core/src/engine.rs` |
| Duration parsing, config defaults, serde | `crates/never-sleep-core/src/config.rs` |
| Wall-clock / format helpers | `crates/never-sleep-core/src/duration.rs` |
| `JsonStatus`, `HostSnapshot`, `ViewModel` | `crates/never-sleep-core/src/status.rs` |
| Language tags and bilingual strings | `crates/never-sleep-core/src/i18n.rs` |
| Effect application, assertion failure | `crates/never-sleep/src/apply.rs` |
| IPC / CLI protocol | `crates/never-sleep/src/protocol.rs`, `cli.rs` |
| Paths, XML, locale helpers | the module that owns them |

Drive host state through `HostSnapshot`. Do not hit real IOKit, `pmset`, or the user’s `~/Library/Application Support/Never Sleep/` from unit tests. Tests that call `try_send`, `save_config`, or `load_config` must install `paths::TestDataDir` so they use a unique temp directory on the current thread.

Prefer table-driven cases for parsers and language tags. Name tests after the product rule (`user_present_does_not_resleep`, not `test1`).

### What must have a test

- Any change to `Engine::handle` / `PowerPlan` / auto-stop conditions.
- Any new or changed `JsonStatus` field (assert the serde name).
- Any new CLI flag or duration syntax.
- Any new UI string that is part of the start/stop/status contract.
- Bug fixes: reproduce with a failing test first, then fix.

### What not to “TDD”

- Pixel-level menu-bar layout (`native_panel.rs`) unless you are changing logic that can be unit-tested via `ViewModel` / `PanelState`.
- IOKit / `pmset` bindings. Keep them thin. Policy belongs in core tests.
- Generated packaging (`dist/`, `.icns`).

If production code is the only way to learn the next test, stop and write the test you now know you need before adding more code.

## Code conventions

- **English-first UI**, Simplified Chinese when `Lang::Zh`. Process override: `--lang` / `NEVER_SLEEP_LANG`. JSON stays English.
- Do not add a third language unless the user asked. Do not change existing English keys to “more natural” phrasing without a product reason; agents and tests pin many strings.
- Keep `#[cfg(target_os = "macos")]` on GUI, IOKit, LaunchAgent, and hotkeys. Do not silence Linux `dead_code` with crate-wide `#![allow(dead_code)]` — cfg-gate macOS-only items or cover them with tests.
- Public Agent types live in `never-sleep-core` (`JsonStatus`, `DurationPref`, `StopReason` codes).
- `StopReason::code()` / `from_code()` are the stable IPC values (`battery_floor`, `thermal_emergency`, …). Human labels may be localized; codes must not be.
- Comments: English is preferred for new comments. Do not delete existing Chinese comments that encode product intent.

## Safety / non-goals

Never introduce:

- `sudo pmset -a disablesleep 1` (or any persistent `pmset` write) as a default path.
- `PreventUserIdleDisplaySleep` / `caffeinate -d` (keeps the panel lit; opposite of this product).
- A design that cannot restore clamshell sleep on quit, panic, or next launch (`session.lock`).

Closed-lid stay-awake is **best effort**. The reliable path is lid open + display sleep. UI copy and `doctor` text must keep saying that.

## IPC and paths

- Config: `~/Library/Application Support/Never Sleep/config.toml` (macOS) or `$XDG_DATA_HOME/never-sleep` (Linux stub). Override with `NEVER_SLEEP_DATA_DIR`.
- Socket: `ipc.sock` in that directory. Line-delimited JSON. Requests use `{"cmd":"on"|"off"|"toggle"|"status"|"quit"|"ping", ...}`.
- While the menu bar is running, CLI commands must talk to that process (`try_send`). Only `on` may fall back to a foreground session.

## Commits and PRs

- Imperative commit subject, product language: `Add battery-floor tests`, not `fix stuff`.
- One logical change per commit when practical (policy+tests can land together after the TDD cycle).
- PR description: what behaviour changed, which tests lock it, and that clippy is warning-free.
- Do not bump the version unless the user asked for a release. Use `./scripts/bump-version.sh x.y.z` — do not hand-edit `packaging/Info.plist` or the marketing-page version strings.
