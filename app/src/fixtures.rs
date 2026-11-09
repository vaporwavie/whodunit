//! Scenes for headless snapshots (`--snap x.png --scene fanout`): fixed transcripts, no pi process.

use crate::core::{Command, Core, Dialog, DialogKind, Shared, lock};
use crate::sessions::Summary;
use crate::transcript::{Block, Deal, Done, END_TITLE, FanAnswer, Item, ToolRun, Transcript};

fn deal(role: &str, size: &str) -> Option<Deal> {
    Some(Deal {
        role: role.into(),
        agent: "anthropic/claude-opus-5-5".into(),
        size: size.into(),
    })
}

fn sidebar(core: &mut Core) {
    let mut rated = Transcript::default().week;
    rated.deal = deal("builder", "M");
    rated.done = Some(Done { score: 4 });
    let mut unrated = Transcript::default().week;
    unrated.deal = deal("scribe", "S");
    core.sessions = vec![
        Summary {
            path: "/fixture/a.jsonl".into(),
            cwd: crate::config::home().join("Workspace/whodunit"),
            title: "Rename the session store to vault".into(),
            week: core.t.week.clone(),
        },
        Summary {
            path: "/fixture/b.jsonl".into(),
            cwd: crate::config::home().join("Workspace/blog"),
            title: "Draft the launch post".into(),
            week: unrated,
        },
        Summary {
            path: "/fixture/c.jsonl".into(),
            cwd: crate::config::home().join("Workspace/uploader"),
            title: "Add retries to the uploader".into(),
            week: rated,
        },
    ];
    core.session_file = Some("/fixture/a.jsonl".into());
    core.cwd = crate::config::home().join("Workspace/whodunit");
}

fn chat(core: &mut Core) {
    core.t.items = vec![
        Item::User("Rename `SessionStore` to `Vault` across the app and keep the tests green.".into()),
        Item::Assistant {
            blocks: vec![
                Some(crate::transcript::Block::Thinking("Find every reference first.".into())),
                Some(crate::transcript::Block::Tool { id: "t1".into(), name: "bash".into(), args: "rg -l SessionStore src".into(), written: None }),
                Some(crate::transcript::Block::Text("Found it in **4 files**. Renaming the type, its module and the two call sites:\n\n```rust\npub struct Vault {\n    root: PathBuf,\n}\n```\n\nTests pass (`cargo test`: 38 passed).".into())),
            ],
            streaming: false,
            error: None,
        },
    ];
    core.t.tools.insert(
        "t1".into(),
        crate::transcript::ToolRun {
            output: "src/store.rs\nsrc/main.rs\nsrc/ui.rs\ntests/store.rs".into(),
            is_error: false,
            running: false,
            patch: None,
        },
    );
    core.t.week.deal = deal("errand-boy", "S");
    core.t.first_prompt = Some("Rename SessionStore to Vault".into());
    core.stats.context_percent = Some(12.0);
    core.stats.cost = 0.084;
}

pub fn scene_chat(shared: &Shared) {
    let mut core = lock(shared);
    chat(&mut core);
    sidebar(&mut core);
}

/// A new task before the first prompt: the empty body over the composer.
pub fn scene_empty(shared: &Shared) {
    sidebar(&mut lock(shared));
}

/// The chat with `/` typed: the slash list over the composer, pi's commands after the app's.
pub fn scene_slash(shared: &Shared) {
    scene_chat(shared);
    let mut core = lock(shared);
    core.commands = [(
        "fast",
        "Toggle OpenAI priority tier (increased usage/cost): /fast [on|off|status]",
    )]
    .into_iter()
    .map(|(name, description)| Command {
        name: name.into(),
        description: description.into(),
    })
    .collect();
}

pub fn scene_end(shared: &Shared) {
    let mut core = lock(shared);
    chat(&mut core);
    sidebar(&mut core);
    core.dialog = Some(Dialog {
        id: "d".into(),
        title: END_TITLE.into(),
        kind: DialogKind::Select(vec![
            "5 · nailed it, great".into(),
            "4 · nailed it, rough edges".into(),
            "3 · needed small fixes".into(),
            "2 · needed big fixes".into(),
            "1 · failed".into(),
        ]),
        highlight: Some(1),
        generation: 1,
    });
}

pub fn scene_reveal(shared: &Shared) {
    let mut core = lock(shared);
    chat(&mut core);
    if let Some(Item::Assistant { blocks, .. }) = core.t.items.get_mut(1) {
        blocks[0] = Some(Block::Thinking("The user wants `SessionStore` renamed to `Vault` everywhere, with tests still passing.\n\nFirst I need every reference: the type, its module path, the imports and the two call sites in `main.rs`. A plain text search is safer than guessing from the module tree, since tests import it by its full path.\n\nThen rename the file from `store.rs` to `vault.rs` so the module name follows the type. The tests live in `tests/store.rs`; they should move too, or the next person looking for the vault tests won't find them.\n\nLast, run the suite. If anything still names `SessionStore`, the compiler will say so.\n\nOne risk: serialized data. If the store writes its type name to disk, renaming it would break old files. Checking `serde` attributes before touching anything.".into()));
    }
    core.t.week.done = Some(Done { score: 5 });
    sidebar(&mut core);
    core.toast(
        "It was Opus 5.5, dealt as errand-boy. Logged 5/5, nailed it.",
        "info",
    );
}

pub fn scene_fanout(shared: &Shared) {
    let mut core = lock(shared);
    core.t.items = vec![Item::Fanout {
        prompt: Some("Write a two-line release note for the scoreboard page.".into()),
        answers: vec![
            FanAnswer { label: "A".into(), ok: true, error: None, text: "**The scoreboard is here.** Every rated task, grouped by role, so you can see which agent earned its seat.".into() },
            FanAnswer { label: "B".into(), ok: true, error: None, text: "See who won each role this week.\nRatings, cost and minutes, side by side.".into() },
            FanAnswer { label: "C".into(), ok: true, error: None, text: "New: /scoreboard. One table per role, and no lab names until you rate.".into() },
        ],
        picked: None,
    }];
    core.t.first_prompt = Some("Write a two-line release note for the scoreboard page.".into());
    core.dialog = Some(Dialog {
        id: "d".into(),
        title: "Which answer is best?".into(),
        kind: DialogKind::Select(vec!["A".into(), "B".into(), "C".into()]),
        highlight: Some(0),
        generation: 1,
    });
    sidebar(&mut core);
    core.t.week.deal = None;
}

fn tool(id: &str, name: &str, args: &str) -> Option<Block> {
    Some(Block::Tool {
        id: id.into(),
        name: name.into(),
        args: args.into(),
        written: None,
    })
}

fn ran(core: &mut Core, id: &str, output: &str, is_error: bool, running: bool) {
    core.t.tools.insert(
        id.into(),
        ToolRun {
            output: output.into(),
            is_error,
            running,
            patch: None,
        },
    );
}

const RETRY_PATCH: &str = "--- src/upload.rs\n+++ src/upload.rs\n@@ -41,9 +41,13 @@ impl Uploader {\n     async fn send_with_retry(&self, chunk: &Chunk) -> Result<()> {\n         let mut attempt = 0;\n         loop {\n             match self.send_chunk(chunk).await {\n                 Ok(()) => return Ok(()),\n-                Err(err) if attempt < 4 => {\n+                Err(err) if attempt < 4 && retryable(&err) => {\n                     attempt += 1;\n                     sleep(backoff(attempt)).await;\n                 }\n                 Err(err) => return Err(err),\n             }\n         }\n     }\n+\n+fn retryable(err: &UploadError) -> bool {\n+    matches!(err, UploadError::Status(500..) | UploadError::Timeout)\n+}\n";

const DEADLINE_TEST: &str = "use uploader::{Uploader, UploadError};\n\n#[tokio::test]\nasync fn gives_up_at_the_deadline() {\n    let server = MockServer::always(503);\n    let started = Instant::now();\n    let err = Uploader::new(server.url()).upload(\"big.bin\").await.unwrap_err();\n    assert!(matches!(err, UploadError::Deadline));\n    assert!(started.elapsed() < Duration::from_secs(31));\n}\n";

/// A builder run mid-way through its second turn: failed and passing tools, a shell command, a queued follow-up.
pub fn scene_run(shared: &Shared) {
    let mut core = lock(shared);
    core.t.items = vec![
        Item::User("The uploader gives up on the first 503. Retry it with backoff, but never retry a 4xx.".into()),
        Item::Assistant {
            blocks: vec![
                Some(Block::Thinking("Read the client before touching it.".into())),
                tool("r1", "read", "src/upload.rs"),
                Some(Block::Text("`send_chunk` returns on the first error, so a 503 ends the whole upload. Adding a retry loop around it:".into())),
                tool("e1", "edit", "src/upload.rs"),
                tool("b1", "bash", "cargo test upload"),
                Some(Block::Text("One test fails: the loop also retried the `413` case, which should fail fast. Narrowing it to 5xx and timeouts.".into())),
                tool("e2", "edit", "src/upload.rs"),
                tool("b2", "bash", "cargo test upload"),
                Some(Block::Text("Done. Retries now cover:\n\n- **5xx** and connection timeouts, up to 4 attempts\n- backoff of `250ms × 2ⁿ` with ±20% jitter\n\nA `4xx` still fails on the first try:\n\n```rust\nfn retryable(err: &UploadError) -> bool {\n    match err {\n        UploadError::Status(code) => *code >= 500,\n        UploadError::Timeout => true,\n        _ => false,\n    }\n}\n```".into())),
            ],
            streaming: false,
            error: None,
        },
        Item::Bash {
            command: "git diff --stat".into(),
            output: " src/upload.rs   | 41 +++++++++++++++++++++++++++++++++--------\n tests/upload.rs | 18 ++++++++++++++++++\n 2 files changed, 51 insertions(+), 8 deletions(-)".into(),
            exit_code: Some(0),
            running: false,
        },
        Item::Notice("Context compacted".into()),
        Item::User("Cap the total wait at 30 seconds and log each retry at warn.".into()),
        Item::Assistant {
            blocks: vec![
                Some(Block::Thinking("Thread a deadline through the loop.".into())),
                Some(Block::Tool {
                    id: "w1".into(),
                    name: "write".into(),
                    args: "tests/deadline.rs".into(),
                    written: Some(DEADLINE_TEST.into()),
                }),
                tool("e3", "edit", "src/upload.rs"),
                Some(Block::Text("Added a 30s deadline: the loop stops when the next sleep would cross it. Each retry logs the attempt, status and delay at `warn`. Running the suite again".into())),
                tool("b3", "bash", "cargo test upload -- --nocapture"),
            ],
            streaming: true,
            error: None,
        },
    ];
    ran(
        &mut core,
        "r1",
        "pub async fn upload(&self, file: &Path) -> Result<Receipt> {\n    for chunk in self.chunks(file)? {\n        self.send_chunk(&chunk).await?;\n    }\n    self.finish().await\n}",
        false,
        false,
    );
    ran(
        &mut core,
        "e1",
        "Replaced 3 lines with 22 lines.",
        false,
        false,
    );
    ran(
        &mut core,
        "b1",
        "   Compiling uploader v0.4.2\n    Finished `test` profile in 3.18s\n     Running tests/upload.rs\n\nrunning 6 tests\ntest retries_on_503 ... ok\ntest retries_on_timeout ... ok\ntest gives_up_after_four ... ok\ntest backoff_doubles ... ok\ntest fails_fast_on_404 ... ok\ntest fails_fast_on_413 ... FAILED\n\nfailures:\n    fails_fast_on_413: expected 1 attempt, got 4\n\ntest result: FAILED. 5 passed; 1 failed",
        true,
        false,
    );
    ran(
        &mut core,
        "e2",
        "Replaced 1 line with 7 lines.",
        false,
        false,
    );
    ran(
        &mut core,
        "b2",
        "running 6 tests\n......\ntest result: ok. 6 passed; 0 failed; finished in 0.41s",
        false,
        false,
    );
    ran(
        &mut core,
        "w1",
        "Successfully wrote to tests/deadline.rs",
        false,
        false,
    );
    ran(
        &mut core,
        "e3",
        "Replaced 9 lines with 16 lines.",
        false,
        false,
    );
    ran(&mut core, "b3", "", false, true);
    core.t.tools.get_mut("e2").expect("e2 ran").patch = Some(RETRY_PATCH.into());
    core.turn_started = Some(std::time::Instant::now() - std::time::Duration::from_secs(72));
    core.t.week.deal = deal("builder", "M");
    core.t.first_prompt = Some("Retry uploads with backoff".into());
    core.streaming = true;
    core.queue = (Vec::new(), vec!["Then add a test for the deadline.".into()]);
    core.stats.context_percent = Some(41.0);
    core.stats.cost = 0.312;
    sidebar(&mut core);
    core.sessions[0].title = "Retry uploads with backoff".into();
    core.sessions[2].title = "Port the scoreboard to Node".into();
}

/// Tool output too wide for its card: minified bundle lines, a long error and a `!cmd` path.
pub fn scene_wide(shared: &Shared) {
    let mut core = lock(shared);
    chat(&mut core);
    sidebar(&mut core);
    let minified = "this.editor.setText(combinedText),this.updatePendingMessagesDisplay(),options?.abort&&this.session.abort(),allQueued.forEach(m=>this.session.followUp(m.text)),queueCompactionMessage(text,mode){this.compactionQueuedMessages.push({text,mode}),this.editor.addToHistory?.(text)}";
    let grep = (0..12)
        .map(|n| format!("/home/me/.local/share/fnm/node-versions/v24.21.0/installation/lib/node_modules/@earendil-works/pi-coding-agent/dist/bundle/chunks/chunk-44DLKSKE.js:{}:{minified}", 1000 + n))
        .collect::<Vec<_>>()
        .join("\n");
    core.t
        .items
        .push(Item::User("Where does pi keep its cache settings?".into()));
    core.t.items.push(Item::Assistant {
        blocks: vec![
            tool("w1", "bash", "rg -n 'cache|compact' ~/.local/share/fnm/node-versions/v24.21.0/installation/lib/node_modules/@earendil-works/pi-coding-agent/dist/bundle/chunks"),
            tool("w2", "bash", "node -e 'require(\"./settings.json\")'"),
        ],
        streaming: false,
        error: None,
    });
    ran(&mut core, "w1", &grep, false, false);
    ran(
        &mut core,
        "w2",
        &format!(
            "node:internal/modules/cjs/loader:1228\n  throw err;\n  ^\n\nError: Cannot find module './settings.json' {}",
            minified.repeat(3)
        ),
        true,
        false,
    );
    core.t.items.push(Item::Bash {
        command: "ls -d ~/.local/share/fnm/node-versions/*/installation/lib/node_modules/@earendil-works/pi-coding-agent/dist/bundle/chunks".into(),
        output: (0..300).map(|n| format!("/home/me/.local/share/fnm/node-versions/v24.21.0/installation/lib/node_modules/@earendil-works/pi-coding-agent/dist/bundle/chunks/chunk-{n:04}.js:{minified}")).collect::<Vec<_>>().join("\n"),
        exit_code: Some(0),
        running: false,
    });
}

/// Working, idle and ended chats, without the old score-before-switch guard.
pub fn scene_tasks(shared: &Shared) {
    scene_run(shared);
}

/// A 60-turn transcript, for measuring scroll frames.
pub fn scene_long(shared: &Shared) {
    let mut core = lock(shared);
    chat(&mut core);
    sidebar(&mut core);
    let turn = core.t.items.clone();
    core.t.items = (0..60)
        .flat_map(|n| {
            turn.iter().cloned().map(move |item| match item {
                Item::User(prompt) => Item::User(format!("{prompt} (turn {n})")),
                Item::Assistant { blocks, streaming, error } => Item::Assistant {
                    blocks: blocks.into_iter().map(|b| match b {
                        Some(crate::transcript::Block::Text(t)) => Some(crate::transcript::Block::Text(format!("{t}\n\n- turn {n}, step one\n- turn {n}, step two\n\n> quoted line {n}"))),
                        other => other,
                    }).collect(),
                    streaming,
                    error,
                },
                other => other,
            })
        })
        .collect();
}

/// The long transcript while an answer streams in.
pub fn scene_growing(shared: &Shared) {
    scene_long(shared);
    let shared = shared.clone();
    std::thread::spawn(move || {
        for n in 0.. {
            std::thread::sleep(std::time::Duration::from_millis(20));
            lock(&shared)
                .t
                .items
                .push(Item::User(format!("streamed line {n}")));
            lock(&shared).toast(format!("tick {n}"), "info");
        }
    });
}

/// The long transcript while pi keeps sending records, as a fan-out's ticker does.
pub fn scene_busy(shared: &Shared) {
    scene_long(shared);
    let shared = shared.clone();
    std::thread::spawn(move || {
        for n in 0.. {
            std::thread::sleep(std::time::Duration::from_millis(20));
            lock(&shared).toast(format!("tick {n}"), "info");
        }
    });
}

fn board_row(
    name: &str,
    runs: usize,
    nailed: usize,
    score_sum: f64,
    minutes: &[f64],
    cost: f64,
    fanouts: usize,
    wins: usize,
) -> crate::scoreboard::Row {
    crate::scoreboard::Row {
        agent: name.into(),
        name: name.into(),
        runs,
        rated: runs,
        unrated: 0,
        nailed,
        score_sum,
        your_minutes: minutes.to_vec(),
        costs: vec![cost; runs],
        fanouts,
        wins,
    }
}

/// The scoreboard page with a few weeks of runs.
pub fn board() -> crate::scoreboard::Board {
    use crate::scoreboard::{Board, Decisions, Reference, Table};
    Board {
        runs: 31,
        decisions: Decisions {
            jev: 24,
            picker: 5,
            forced: 2,
            fallback: 0,
        },
        tables: vec![
            Table {
                role: "builder".into(),
                fanout: false,
                decisions: Decisions {
                    jev: 14,
                    picker: 3,
                    forced: 1,
                    fallback: 0,
                },
                rows: vec![
                    board_row("Opus 5.5", 9, 7, 39.0, &[4.0, 6.5, 3.0], 0.412, 0, 0),
                    board_row("GPT-6.1 Sol", 7, 4, 26.0, &[7.0, 9.5], 0.091, 0, 0),
                    board_row(
                        "Kimi K3.1 Pro with a long display name",
                        2,
                        1,
                        7.0,
                        &[11.0],
                        0.034,
                        0,
                        0,
                    ),
                ],
            },
            Table {
                role: "scribe".into(),
                fanout: true,
                decisions: Decisions {
                    jev: 10,
                    picker: 2,
                    forced: 1,
                    fallback: 0,
                },
                rows: vec![
                    board_row("GPT-6 Astra", 8, 6, 33.0, &[2.0, 1.5], 0.06, 8, 5),
                    board_row("Gemini 4 Argon", 5, 3, 19.0, &[2.5], 0.021, 8, 3),
                ],
            },
        ],
        reference: Reference {
            sessions: 42,
            your_minutes: Some(5.5),
            agent_minutes: Some(12.25),
            turns: Some(9.0),
        },
    }
}

/// ⌘O's folder picker over the chat: recents on top, the browsed folder's subfolders below.
pub fn picker() -> crate::ui::folder_picker::FolderPicker {
    use std::time::{Duration, SystemTime};
    let home = crate::config::home();
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
    let recent = |path: &str, ago: u64| crate::folders::Recent {
        path: home.join(path),
        at: now - Duration::from_secs(ago),
    };
    crate::ui::folder_picker::FolderPicker::with(
        home.join("Workspace"),
        [
            "dotfiles",
            "harness",
            "launch-site",
            "rune",
            "uploader",
            "vault",
            "whodunit",
        ]
        .map(str::to_owned)
        .to_vec(),
        vec![
            recent("Workspace/uploader", 40 * 60),
            recent("Workspace/launch-site", 5 * 3_600),
            crate::folders::Recent {
                path: home.clone(),
                at: now - Duration::from_secs(22 * 3_600),
            },
            recent("Workspace/rune", 2 * 86_400),
            recent("Documents/recipes", 9 * 86_400),
        ],
        now,
    )
}
