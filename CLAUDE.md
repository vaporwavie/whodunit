# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

whodunit: blind trials for coding agents in the pi harness. An extension deals a hidden model per role, and the user ends each run with a score before the role and model are revealed. `README.md` is the spec (session flow, house rules, scoring); read it before changing behavior.

Four pieces live here; data lives under `~/.pi/agent/whodunit/`:

- `app/`: Rust desktop frontend (macOS, built on the `rune` UI crate pinned by git rev in `Cargo.toml`). It drives pi over `pi --mode rpc --model whodunit/auto`.
- `scoreboard/scoreboard.mjs`: standalone Node scoreboard (`node scoreboard/scoreboard.mjs --since 2026-11-09 [--until …] [--no-reference]`).
- `extension/index.ts`: the pi extension that does classification, decks, fan-out and `/done`. Its pure rules (score folding, the deck draw) live in `extension/rules.ts`. pi loads it through a symlink, `~/.pi/agent/extensions/whodunit` → `extension/`, so edits here are live on the next pi start. `extension/roles.example.json` is the sample config.
- `laws/`: the same rules in Bend2 (`rules.bend`), their claims (`LAWS.bend`) and proofs (`PROOF.bend`). `node laws/check.mjs` runs the proofs and compares `extension/rules.ts` with the compiled spec; run it after touching either. Needs `bend` (`~/.bend/bin/bend`).
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
cargo run --release -- --snap out.png --scene fanout [--theme light|dark] [--scale 2]   # without --theme it follows the system

# Live E2E tests: real pi, real models, a few cents each. Always single-threaded.
cargo test --release --test live_pi -- --ignored --nocapture --test-threads 1
cargo test --release --test live_pi a_fanout_shows_every_answer -- --ignored --nocapture --test-threads 1
```

There are no unit tests; `tests/live_pi.rs` is the only suite. Each test removes its own lines from `runs.jsonl`, deletes its session files, and restores `decks.json`, so the week's real data stays clean. Keep that cleanup in any new live test.

## Architecture

- **One shared `Core`** (`src/core.rs`, `Arc<Mutex<Core>>`) holds the pi process, transcript, dialogs, toasts and session list. The chat window (`ui/full.rs`) reads it, and a menu bar icon (`menu_bar.rs`) reopens the window. Shared view pieces are in `ui/parts.rs`. Views only read `Core` and call its command methods (`submit`, `end_run`, `abort`, `request_leave`, `answer_dialog`…).
- **pi RPC** (`src/pi.rs`): JSONL on stdin/stdout, split on LF only. A reader thread calls `Core::apply` per record, then fires the window wakers. `start_pi` bumps `generation` on every (re)start, and records from an older generation are dropped, so restarting in another folder can't mix sessions.
- **Extension ↔ app contract**: the extension writes custom session entries, `whodunit.deal`, `.done`, `.switch`, `.fanout-view` and `.fanout-pick` (constants in `src/transcript.rs`). Dialogs (role picker, end run, fan-out pick) arrive as `extension_ui_request` and are answered with `extension_ui_response`. The end-run select is recognized by its title (`END_TITLE`, "End this run") and its options read `5 · nailed it, great` down to `1 · failed`: a digit only highlights, ⏎ commits. Renaming any of these means changing the extension too.
- **The model and the role stay hidden** (the ground rule in `README.md`): `transcript.rs` never copies model or provider fields into the transcript. The role, size and agent show only via `Core::reveal()` after a `done` entry (toolbar `role_line`, sidebar subtitles, fan-out notes, the extension footer and `/role`). Don't add UI or logging that leaks either before `/done`.
- **Ending seals the session**: after `done`, `Core::submit` refuses everything but slash commands and the composer is disabled; the extension's `input` hook refuses prompts too, for plain pi.
- **Open-run guard** (house rule 4): leaving a session whose run hasn't ended goes through `request_leave` / `answer_leave` (`Leave::{New, Open}`), which can route through `end_run` first.
- **Session files** (`src/sessions.rs`): pi stores sessions under `~/.pi/agent/sessions/--<cwd with / \ : as ->--/` (`config::session_dir`). pi writes a file only after the first assistant message, so a fan-out session that hasn't been followed up is listed from memory.
- **Scoreboard exists twice**: `src/scoreboard.rs` (the app's page and `--scoreboard`) and `scoreboard/scoreboard.mjs`. Both join `runs.jsonl` with session-file metrics and add a Claude Code reference row from the last 30 days of `~/.claude/projects`. Both also show who decided each role (Jev, picker, `/role`, fallback) from the deal's `source`. Ranking rules (mean score vs fan-out win share, `TOO_FEW = 3`, 10-minute idle cap) come from `README.md`; a change to one implementation needs the same change in the other.
- **Environment**: a Finder-launched app gets launchd's bare env, so `config::login_env()` runs the login shell once (`$SHELL -lic`) to find `pi` and the API keys. These are kept in memory only, never written to disk.
- **Fixtures** (`src/fixtures.rs`) build fixed transcripts for the headless scenes (`chat`, `end`, `reveal`, `fanout`, `run`, `guard`, `long`, `busy`, `growing`, `board`). The README screenshots in `docs/` are the `run`, `end` and `fanout` scenes at `--scale 2`. Add a scene there and register it in `main.rs` when adding UI states.
