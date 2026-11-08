# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

whodunit: blind trials for coding agents in the pi harness. An extension deals a hidden model per role, and the user rates each run before the model is revealed. `README.md` is the spec (session flow, house rules, scoring); read it before changing behavior.

Three pieces live here; data lives under `~/.pi/agent/whodunit/`:

- `app/`: Rust desktop frontend (macOS, built on the `rune` UI crate pinned by git rev in `Cargo.toml`). It drives pi over `pi --mode rpc --model whodunit/auto`.
- `scoreboard/scoreboard.mjs`: standalone Node scoreboard (`node scoreboard/scoreboard.mjs --since 2026-11-09 [--until …] [--no-reference]`).
- `extension/index.ts`: the pi extension that does classification, decks, fan-out and `/done`. pi loads it through a symlink, `~/.pi/agent/extensions/whodunit` → `extension/`, so edits here are live on the next pi start. `extension/roles.example.json` is the sample config.
- Data, outside the repo: `~/.pi/agent/whodunit/` (`roles.json`, `decks.json`, `runs.jsonl`, `app.json`).

## Commands (run from `app/`)

Toolchain is pinned to Rust 1.99.1 (edition 2024) via `rust-toolchain.toml`.

rune comes from the fork `vaporwavie/rune` (branch `whodunit`, pinned by rev), which adds `TextArea::plain`, `Button::show_shortcut`, `MarkdownStyle::prose_width` and the OpenCode theme's contrast fixes on top of upstream. Switch back to `anomalyco/rune` once those land there. To try rune changes locally, add a `[patch."https://github.com/vaporwavie/rune"]` pointing at a checkout, and don't commit it.

```sh
cargo build --release                      # build
cargo clippy && cargo fmt                  # lint / format
scripts/bundle.sh                          # release build, installs ~/Applications/Whodunit.app
cargo run --release -- -- --scoreboard     # print the scoreboard as Markdown and exit
cargo run --release -- -- --cwd ~/some/dir # run pi in that folder (app args go after a second `--`, read via rune::extra_args)

# Headless snapshot of a fixture scene, no pi process (scenes are registered in src/main.rs)
cargo run --release -- --snap out.png --scene fanout [--theme dark] [--scale 2]

# Live E2E tests: real pi, real models, a few cents each. Always single-threaded.
cargo test --release --test live_pi -- --ignored --nocapture --test-threads 1
cargo test --release --test live_pi a_fanout_shows_every_answer -- --ignored --nocapture --test-threads 1
```

There are no unit tests; `tests/live_pi.rs` is the only suite. Each test removes its own lines from `runs.jsonl`, deletes its session files, and restores `decks.json`, so the week's real data stays clean. Keep that cleanup in any new live test.

## Architecture

- **One shared `Core`** (`src/core.rs`, `Arc<Mutex<Core>>`) holds the pi process, transcript, dialogs, toasts and session list. The chat window (`ui/full.rs`) reads it, and a menu bar icon (`menu_bar.rs`) reopens the window. Shared view pieces are in `ui/parts.rs`. Views only read `Core` and call its command methods (`submit`, `rate`, `abort`, `request_leave`, `answer_dialog`…).
- **pi RPC** (`src/pi.rs`): JSONL on stdin/stdout, split on LF only. A reader thread calls `Core::apply` per record, then fires the window wakers. `start_pi` bumps `generation` on every (re)start, and records from an older generation are dropped, so restarting in another folder can't mix sessions.
- **Extension ↔ app contract**: the extension writes custom session entries, `whodunit.deal`, `.done`, `.switch`, `.fanout-view` and `.fanout-pick` (constants in `src/transcript.rs`). Dialogs (role picker, outcome, score, fan-out pick) arrive as `extension_ui_request` and are answered with `extension_ui_response`. Renaming any of these means changing the extension too.
- **The model stays hidden**: `transcript.rs` never copies model or provider fields into the transcript. The agent is shown only via `Core::reveal()` after a `done` entry. Don't add UI or logging that leaks the dealt model before `/done`.
- **Unrated-run guard** (house rule 4): leaving a session with an unrated answer goes through `request_leave` / `answer_leave` (`Leave::{New, Open}`), which can route through the rating first.
- **Session files** (`src/sessions.rs`): pi stores sessions under `~/.pi/agent/sessions/--<cwd with / \ : as ->--/` (`config::session_dir`). pi writes a file only after the first assistant message, so a fan-out session that hasn't been followed up is listed from memory.
- **Scoreboard exists twice**: `src/scoreboard.rs` (the app's page and `--scoreboard`) and `scoreboard/scoreboard.mjs`. Both join `runs.jsonl` with session-file metrics and add a Claude Code reference row from the last 30 days of `~/.claude/projects`. Ranking rules (mean score vs fan-out win share, `TOO_FEW = 3`, 10-minute idle cap) come from `README.md`; a change to one implementation needs the same change in the other.
- **Environment**: a Finder-launched app gets launchd's bare env, so `config::login_env()` runs the login shell once (`$SHELL -lic`) to find `pi` and the API keys. These are kept in memory only, never written to disk.
- **Fixtures** (`src/fixtures.rs`) build fixed transcripts for the headless scenes (`chat`, `rate`, `reveal`, `fanout`, `run`, `guard`, `long`, `busy`, `growing`, `board`). Add a scene there and register it in `main.rs` when adding UI states.
