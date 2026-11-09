# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

whodunit: blind trials for coding agents in the pi harness. An extension deals a hidden model per role, and the user ends each run with a score before the role and model are revealed. `README.md` is the spec (session flow, house rules, scoring) and `app/README.md` covers the app (parallel chats, keys, score card, sidebar states); read them before changing behavior.

Four pieces live here; data lives under `~/.pi/agent/whodunit/`:

- `app/`: Rust desktop frontend (macOS, built on the `rune` UI crate pinned by git rev in `Cargo.toml`). It drives pi over `pi --mode rpc --model whodunit/auto`.
- `scoreboard/scoreboard.mjs`: standalone Node scoreboard (`node scoreboard/scoreboard.mjs --since 2026-11-09 [--until …]`).
- `extension/index.ts`: the pi extension that does classification, session titles, decks, fan-out and `/done`. Its pure rules (score folding, the deck draw) live in `extension/rules.ts`. pi loads it through a symlink, `~/.pi/agent/extensions/whodunit` → `extension/`, so edits here are live on the next pi start. `extension/roles.example.json` is the sample config.
- `laws/`: the same rules in Bend2 (`rules.bend`), their claims (`LAWS.bend`) and proofs (`PROOF.bend`). `node laws/check.mjs` runs the proofs and compares `extension/rules.ts` with the compiled spec; run it after touching either. Needs `bend` (`~/.bend/bin/bend`).
- Data, outside the repo: `~/.pi/agent/whodunit/` (`roles.json`, `decks.json`, `runs.jsonl`, `app.json`).

## Commands (run from `app/`)

Toolchain is pinned to Rust 1.99.1 (edition 2024) via `rust-toolchain.toml`.

rune comes from the fork `vaporwavie/rune` (branch `whodunit`, pinned by rev), which adds `TextArea::plain`, `Button::show_shortcut`, `MarkdownStyle::prose_width`, `SplitView::sidebar_title` and the OpenCode theme's contrast fixes on top of upstream. Switch back to `anomalyco/rune` once those land there. To try rune changes locally, add a `[patch."https://github.com/vaporwavie/rune"]` pointing at a checkout, and don't commit it.

```sh
cargo build --release                      # build
cargo clippy && cargo fmt                  # lint / format
scripts/bundle.sh                          # release build, installs ~/Applications/Whodunit.app (macOS) or ~/.local/bin/whodunit + desktop entry (Linux)
cargo run --release -- -- --scoreboard     # print the scoreboard as Markdown and exit
cargo run --release -- -- --cwd ~/some/dir # run pi in that folder (app args go after a second `--`, read via rune::extra_args)

# Headless snapshot of a fixture scene, no pi process (scenes are registered in src/main.rs)
cargo run --release -- --snap out.png --scene fanout [--theme light|dark] [--scale 2]   # without --theme it follows the system

# Live E2E tests: real pi, real models, a few cents each. Always single-threaded.
cargo test --release --test live_pi -- --ignored --nocapture --test-threads 1
cargo test --release --test live_pi a_fanout_shows_every_answer -- --ignored --nocapture --test-threads 1
```

`cargo test` includes unit tests in `src/core/tests.rs` and offline session-isolation tests in `tests/parallel_tasks.rs`, using a deterministic RPC peer and a temporary HOME. `node --test extension/decks.test.mjs` checks concurrent deck draws. `tests/live_pi.rs` talks to real pi and models; each live test removes its own lines from `runs.jsonl`, deletes its session files, and restores `decks.json`, so the week's real data stays clean. Keep that cleanup in any new live test.

## Architecture

- **One shared `Core`** (`src/core.rs`, `Arc<Mutex<Core>>`) holds global config and the session list, the selected `Task`, and background tasks. Each task owns its pi process, folder, transcript, dialogs, queue, stats and toasts. `Core` dereferences to the selected task for view reads. The chat window (`ui/full.rs`) reads it, and a menu bar icon (`menu_bar.rs`) reopens the window. Shared view pieces are in `ui/parts.rs`. Views only read `Core` and call its command methods (`submit`, `end_run`, `abort`, `request_leave`, `answer_dialog`…).
- **pi RPC** (`src/pi.rs`): JSONL on stdin/stdout, split on LF only. A reader thread calls `Core::apply` per record, then fires the window wakers. `Core::request_leave` selects the chat at once (a live task, a saved one read from its file, or a new one) and queues any that needs a process in `Core::unstarted`; `poll` hands those to `start_unstarted` off the UI thread, and records sent before the process exists wait in its outbox. Each process has a stable generation; reader callbacks route records to that task under the shared lock and restore the selected task before waking the window. Unknown generations are dropped. Callbacks hold weak references so dropping `Core` shuts down all processes.
- **Extension ↔ app contract**: the extension writes custom session entries, `whodunit.deal`, `.done`, `.switch`, `.fanout-view` and `.fanout-pick` (constants in `src/transcript.rs`). Dialogs (role picker, end run, fan-out pick) arrive as `extension_ui_request` and are answered with `extension_ui_response`. The end-run select is recognized by its title (`END_TITLE`, "End this run") and its options read `5 · nailed it, great` down to `1 · failed`: a digit only highlights, ⏎ commits. Renaming any of these means changing the extension too.
- **The model and the role stay hidden** (the ground rule in `README.md`): `transcript.rs` never copies model or provider fields into the transcript. The role, size and agent show only via `Core::reveal()` after a `done` entry (sidebar subtitles, fan-out notes, the extension footer and `/role`). Don't add UI or logging that leaks either before `/done`.
- **Ending seals the session**: after `done`, `Core::submit` refuses everything but slash commands and the composer gives way to an ended panel with **New task**; the extension's `input` hook refuses prompts too, for plain pi.
- **Parallel chats**: one task per chat, but any number of chats may work at once. `request_leave` (`Leave::{New, NewIn, Open}`) navigates without a scoring guard or abort. The sidebar groups sessions by `Core::session_state`: Working, Idle (including dialogs waiting for input), Ended. Scoring and aborting affect only the selected chat. `FullUi` keeps composer and dialog drafts per task ID.
- **Session files** (`src/sessions.rs`): pi stores sessions under `~/.pi/agent/sessions/--<cwd with / \ : as ->--/` (`config::session_dir`). The sidebar lists every folder's dealt sessions and reads each one's folder from its header line. Opening one shows its transcript from the file in the same frame, then starts pi with `--session` in that folder; a live chat keeps its process. The handler that navigates calls `catch_up`, so that frame is already the new chat, settled (`Tx::settle`), with nothing animating over from the last one. pi writes a file only after the first assistant message, so live chats (including classification and fan-outs before the first answer) are overlaid from memory.
- **Scoreboard sprites**: `assets/sprites/{roles,models,deciders}.json`, keyed by role name, agent id and deal source (`jev`, `picker`, `forced`, `fallback`), each with a `fallback`. `src/sprites.rs` upscales them nearest-neighbour once at startup to twice their point size, so they stay exact at 2×.
- **Scoreboard exists twice**: `src/scoreboard.rs` (the app's page and `--scoreboard`) and `scoreboard/scoreboard.mjs`. Both join `runs.jsonl` with session-file metrics. Both also show who decided each role (Jev, picker, `/role`, fallback) from the deal's `source`. Ranking rules (mean score vs fan-out win share, `TOO_FEW = 3`, 10-minute idle cap) come from `README.md`; a change to one implementation needs the same change in the other.
- **Environment**: a Finder-launched app gets launchd's bare env, so `config::login_env()` runs the login shell once (`$SHELL -lic`) to find `pi` and the API keys. These are kept in memory only, never written to disk.
- **Fixtures** (`src/fixtures.rs`) build fixed transcripts for the headless scenes (`empty`, `chat`, `end`, `reveal`, `fanout`, `run`, `tasks`, `long`, `wide`, `busy`, `slash`, `slash-empty`, `shell`, `growing`, `board`, `picker`, `picker-cmd`). The screenshots in `docs/` (used by both READMEs) are the `run`, `end` and `fanout` scenes at `--scale 2`. Add a scene there and register it in `main.rs` when adding UI states.
