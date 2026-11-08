# Whodunit.app

A macOS desktop frontend for the [whodunit](../README.md) pi extension, written in Rust on [rune](https://github.com/anomalyco/rune). It drives pi over RPC (`pi --mode rpc --model whodunit/auto`) and keeps the same ground rule: neither the role nor the model shows until the run ends.

On top of plain pi it adds a sidebar of sessions, a guard against leaving a run open, a scoreboard page and a menu bar icon that reopens the window.

The sidebar lists runs from every folder, newest first, each under its folder's name (and its role, size and score once ended). Opening one from another folder restarts pi there. Edits and writes show as diffs. While the agent works, a line over the composer says what it's doing and for how long. Thinking stays hidden until the run ends, since a trace's style names the lab; after that every thought shows in full, capped at twelve lines that scroll.

## Install

It needs macOS and Rust (the version is pinned in `rust-toolchain.toml`). Set up the extension first, as in the [root README](../README.md#setup).

```sh
cd app
scripts/bundle.sh    # installs ~/Applications/Whodunit.app
```

Launched from Finder, the app runs your login shell once to find `pi` and your API keys. They stay in memory and are never written to disk.

## Ending a run

⌘D or the **End run** button opens the score card. A digit from 5 to 1 only highlights a row, and ⏎ commits, because ending can't be undone. Esc goes back to working. Once ended, the composer gives way to a **New task** button (⌘N).

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="../docs/end-dark.png">
  <img alt="Ending a run: five scores on one card, 4 highlighted, End run waits for Enter" src="../docs/end-light.png">
</picture>

Leaving a session whose run hasn't ended (⌘N, or picking another session) asks first: **Stay**, **Leave unrated**, or **End it now** to score it on the spot.

## Fan-outs

A fan-out shows each contender's answer as A, B, C. Press a letter (or 1, 2, 3) to pick the best, and the winner continues the session.

<img alt="A fan-out: three anonymous answers, picked by key" src="../docs/fanout-light.png">

## Folder picker

⌘O, or the folder button on a new task's composer, opens the folder picker.

The top list is recent folders, newest first: the ones you opened here and the ones plain pi ran in. The folder pi runs in now isn't listed, since ⌘N already starts a new task there. Below the recents is the current folder's parent, so your other projects are one ↓ away.

⏎ works one level at a time. When the picker opens nothing is selected, and ⏎ runs pi in the folder it's browsing. ⏎ on a subfolder, or on Go up, opens that folder with its first subfolder selected, so ⏎ keeps going down, or Go up when it has none. Recent folders skip the step and run pi on the first ⏎.

Typing filters both lists and selects the first match. **Go up**, always the first row under Browse, opens its parent, as ⌫ or ← on an empty filter does. Hold ⌘ and the button turns into **Select ⌘⏎**: it runs pi in the folder you're browsing, whatever is selected.

## Keys

| Key | Does |
|---|---|
| ⌘D | End the run |
| ⌘N | New task |
| ⌘1 | Chat |
| ⌘2 | Scoreboard |
| ⌘O | Pick the folder pi runs in |
| ⌘⏎ | Queue a follow-up while the agent works |
| Tab / ⏎ | In the `/` list: complete the command / run it (commands that need an argument are completed instead) |
| Esc | Stop the agent |

## Command line

```sh
whodunit -- --cwd ~/some/dir    # run pi in that folder
whodunit -- --scoreboard        # print the scoreboard as Markdown and exit
```

The Scoreboard page shows the same board as `scoreboard/scoreboard.mjs`.
