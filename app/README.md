# Whodunit.app

A macOS desktop frontend for the [whodunit](../README.md) pi extension, written in Rust on [rune](https://github.com/anomalyco/rune). It drives pi over RPC (`pi --mode rpc --model whodunit/auto`) and keeps the same ground rule: neither the role nor the model shows until the run ends.

On top of plain pi it adds parallel task chats, a sidebar of sessions, a scoreboard page and a menu bar icon that reopens the window.

## Multiple tasks

One task belongs to one chat, but you can work on several tasks at once. ⌘N starts another chat without stopping the others or asking you to score them. Each live chat has its own pi process, folder, transcript, queue and dialogs. Switching back restores that chat, including any unsent draft. Chats in the same folder share files; use separate worktrees if the tasks need isolated edits.

The sidebar lists chats from every folder in three groups:

- **Working**: the agent is running, classifying or dealing, running a fan-out, or doing shell work or compaction. Background chats show their activity too.
- **Idle**: the chat is open but isn't working. It may be waiting for your reply, a role or fan-out pick, or a score. A dot marks chats waiting on a dialog. Saved open chats from earlier launches also appear here.
- **Ended**: you've scored the run and sealed the chat. This section stays at the bottom of the sidebar and starts collapsed; click its heading to show or hide ended chats. Expanded history scrolls separately from open chats.

Each row shows its folder, with the size and score added only after the run ends. An ended row also shows its role's icon before the title and its agent's icon at the end; rest the pointer on either to see its name. Opening a saved chat starts pi in that chat's folder; live chats keep their existing process. Changing folders starts a new chat and leaves other tasks running. Quit the app to stop all its pi processes; chats resume from their saved history, not mid-turn.

Code snippets have a **Copy** button that copies just the code, without Markdown fences. It works in chat and fan-out answers, including while an answer streams. You can still select and copy part of an answer.

Edits and writes show as diffs. While the agent works, a line over the composer says what it's doing and for how long. Thinking stays hidden until the run ends, since a trace's style names the lab; after that every thought shows in full, capped at twelve lines that scroll.

The ring next to Send shows how close the chat is to compaction. At 100%, pi compacts on its own: that's the model's window minus `compaction.reserveTokens` from pi's settings (the `whodunit/auto` override wins if you set one). It updates after every response and every batch of tool results, not only when the turn ends. Right after a compaction, pi has no count until the next response, so the ring shows the compaction's own estimate of what it kept, marked `~`. With auto-compaction off, it shows the share of the whole window.

## Images

Paste an image with ⌘V, or drop PNG, JPEG, GIF or WebP files anywhere on an open chat. They show as tiles above the composer and go with your next message. × or ⌫ in an empty composer removes one. Click any image, in the composer or the chat, to see it whole; Esc or a click closes it, and **Open in Preview** opens it to zoom. Slash and `!` commands leave them for the next prompt. pi scales large images down before a model sees them.

Jev can't see images, so it classifies the text plus a note saying images are attached. The titler sees them. In a fan-out every contender gets the images, and the winner keeps them for follow-ups. A contender without vision gets pi's placeholder text instead, so an image-heavy task can handicap it.

## Install

It needs macOS or Linux, and Rust (the version is pinned in `rust-toolchain.toml`). Set up the extension first, as in the [root README](../README.md#setup).

```sh
cd app
scripts/bundle.sh    # installs ~/Applications/Whodunit.app (macOS) or ~/.local/bin + a desktop entry (Linux)
```

Launched from Finder, the app runs your login shell once to find `pi` and your API keys. They stay in memory and are never written to disk.

## Ending a run

⌘D or the **End run** button opens the score card. A digit from 5 to 1 only highlights a row, and ⏎ commits, because ending can't be undone. Esc goes back to working. Once ended, the composer gives way to a **New task** button (⌘N).

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="../docs/end-dark.png">
  <img alt="Ending a run: five scores on one card, 4 highlighted, End run waits for Enter" src="../docs/end-light.png">
</picture>

End each run when you're finished with its task, not when you switch chats. ⌘D scores only the selected chat. Switching away leaves an open run unrated until you return and score it; ending one chat doesn't affect the others.

## Fan-outs

A fan-out shows each contender's answer as A, B, C. Press a letter (or 1, 2, 3) to pick the best, and the winner continues the session.

<img alt="A fan-out: three anonymous answers, picked by key" src="../docs/fanout-light.png">

## Folder picker

⌘O, or the folder button on a new task's composer, opens the folder picker.

The top list is recent folders, newest first: the ones you opened here and the ones plain pi ran in. The folder pi runs in now isn't listed, since ⌘N already starts a new task there. Below the recents is the current folder's parent, so your other projects are one ↓ away.

⏎ works one level at a time. When the picker opens nothing is selected, and ⏎ runs pi in the folder it's browsing. ⏎ on a subfolder, or on Go up, opens that folder with its first subfolder selected, so ⏎ keeps going down, or Go up when it has none. Recent folders skip the step and run pi on the first ⏎.

Typing filters both lists and selects the first match. **Go up**, always the first row under Browse, opens its parent, as ⌫ or ← on an empty filter does. Hold ⌘ and the button turns into **Select ⌘⏎**: it runs pi in the folder you're browsing, whatever is selected.

## Keys

On Linux, read ⌘ as Ctrl.

| Key | Does |
|---|---|
| ⌘D | End the run |
| ⌘N | New task in another chat; other chats keep working |
| ⌘1 | Chat |
| ⌘2 | Scoreboard |
| ⌘O | Pick the folder pi runs in |
| ⌘⏎ | Queue a follow-up while the agent works |
| ⌘V | Paste text, or attach a copied image or image files |
| ⌫ | In an empty composer: remove the last attached image |
| Tab / ⏎ | In the `/` list: complete the command / run it (commands that need an argument are completed instead) |
| Esc | Cancel the selected chat's dialog, or stop its agent |

## Command line

```sh
whodunit -- --cwd ~/some/dir    # run pi in that folder
whodunit -- --scoreboard        # print the scoreboard as Markdown and exit
```

The Scoreboard page shows the same numbers as `scoreboard/scoreboard.mjs`, laid out like character sheets. It opens with **Leading each role**, one tile per role with the agent winning it, its score or fan-out record and its lead over the runner-up; a role shows *No leader yet* until an agent has 3 runs. Below that come the model tiles, then one card per role, stacked in a single column so each figure column lines up down the page. A card's header has the role's sprite, its criteria and a dithered bar of who decided its deals (Jev, picker, `/role`, fallback), with each share that isn't zero beside it under its decider's icon. Under it, a table lists every contender: rank, score (and fan-outs won, which fan-out roles rank by), runs, nailed %, cost per run and your median minutes, each header explaining itself on hover. The leader's row is tinted, with its lead over the runner-up under its ranking figure. Agents short of 3 runs stay in the table, faded and unranked, but their figures are blocked: a hatch covers the columns, with a block per run needed, filled for each run done, and how many runs are left. A class without a leader still shows who is closest. The sprites are JSON in `assets/sprites/` (16×16 roles, 12×12 models and deciders, at most 5 colors each). Roles are keyed by role name and deciders by deal source.

Model artwork in `assets/sprites/models.json` uses stable keys (`opus`, `sol`, `astra`, `gemini`, `kimi`). `assets/sprites/model-aliases.json` maps each supported `provider/model` ID to an artwork key, so multiple providers can share one sprite. To support another ID, add an explicit alias; names and ID fragments are never used to guess the icon. Unlisted IDs get `fallback`. These aliases affect only artwork, not scoreboard grouping or run history. Both files are compiled into the app, so rebuild after editing them.
