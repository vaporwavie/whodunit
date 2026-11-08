# Whodunit.app

A macOS desktop frontend for the [whodunit](../README.md) pi extension, written in Rust on [rune](https://github.com/anomalyco/rune). It drives pi over RPC (`pi --mode rpc --model whodunit/auto`) and keeps the same ground rule: neither the role nor the model shows until the run ends.

On top of plain pi it adds a sidebar of sessions, a guard against leaving a run open, a scoreboard page and a menu bar icon that reopens the window.

## Install

It needs macOS and Rust (the version is pinned in `rust-toolchain.toml`). Set up the extension first, as in the [root README](../README.md#setup).

```sh
cd app
scripts/bundle.sh    # installs ~/Applications/Whodunit.app
```

Launched from Finder, the app runs your login shell once to find `pi` and your API keys. They stay in memory and are never written to disk.

## Ending a run

⌘D or the **End run** button opens the score card. A digit from 5 to 1 only highlights a row, and ⏎ commits, because ending can't be undone. Esc goes back to working. Once ended, the composer is disabled and ⌘N starts the next task.

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="../docs/end-dark.png">
  <img alt="Ending a run: five scores on one card, 4 highlighted, End run waits for Enter" src="../docs/end-light.png">
</picture>

Leaving a session whose run hasn't ended (⌘N, or picking another session) asks first: **Stay**, **Leave unrated**, or **End it now** to score it on the spot.

## Fan-outs

A fan-out shows each contender's answer as A, B, C. Press a letter (or 1, 2, 3) to pick the best, and the winner continues the session.

<img alt="A fan-out: three anonymous answers, picked by key" src="../docs/fanout-light.png">

## Keys

| Key | Does |
|---|---|
| ⌘D | End the run |
| ⌘N | New task |
| ⌘1 | Chat |
| ⌘2 | Scoreboard |
| ⌘O | Run pi in another folder |
| ⌘⏎ | Queue a follow-up while the agent works |
| Esc | Stop the agent |

## Command line

```sh
whodunit -- --cwd ~/some/dir    # run pi in that folder
whodunit -- --scoreboard        # print the scoreboard as Markdown and exit
```

The Scoreboard page shows the same board as `scoreboard/scoreboard.mjs`.
