//! End to end against the real pi and the installed whodunit extension. Each test spends a few
//! cents of API usage, then removes its lines from runs.jsonl and its session files and restores
//! decks.json, so the week's data stays clean.
//! Run with `cargo test --release --test live_pi -- --ignored --nocapture --test-threads 1`.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rune::{Key, KeyEvent, Mods};
use whodunit::config::{self, AppConfig};
use whodunit::core::{Core, DialogKind, Leave, Shared, lock, start_unstarted};
use whodunit::transcript::{Block, END_TITLE, Item};
use whodunit::ui::window::dialog_keys;

fn wait_for(shared: &Shared, what: &str, seconds: u64, mut done: impl FnMut(&mut Core) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(seconds);
    loop {
        {
            let mut core = lock(shared);
            if let Some(exit) = &core.exited {
                panic!("pi exited while waiting for {what}: {exit}");
            }
            if done(&mut core) {
                return;
            }
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Presses `key` on the select dialog titled like `title`, the way the composer would.
fn press(shared: &Shared, title: &str, key: Key) {
    wait_for(shared, title, 300, |core| {
        let open = core
            .dialog
            .as_ref()
            .is_some_and(|d| d.title.contains(title) && matches!(d.kind, DialogKind::Select(_)));
        open && dialog_keys(
            core,
            &KeyEvent {
                key: key.clone(),
                mods: Mods::default(),
            },
            true,
        )
    });
}

fn text_key(key: &str) -> Key {
    Key::Text(key.to_owned())
}

fn answer_text(core: &Core) -> String {
    core.t
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Assistant { blocks, .. } => Some(
                blocks
                    .iter()
                    .flatten()
                    .filter_map(|b| {
                        if let Block::Text(t) = b {
                            Some(t.as_str())
                        } else {
                            None
                        }
                    })
                    .collect::<String>(),
            ),
            _ => None,
        })
        .collect()
}

struct Run {
    shared: Shared,
    cwd: PathBuf,
    had_runs: bool,
    decks_before: Option<Vec<u8>>,
}

impl Run {
    fn start(name: &str) -> Run {
        let runs = config::week_dir().join("runs.jsonl");
        let decks_before = std::fs::read(config::week_dir().join("decks.json")).ok();
        let cwd = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
        std::fs::create_dir_all(&cwd).expect("create test folder");
        let shared: Shared = Arc::new(Mutex::new(Core::new(AppConfig::load(), cwd.clone())));
        lock(&shared).request_leave(Leave::New);
        start_unstarted(&shared);
        wait_for(&shared, "pi to load the session", 60, |core| core.ready);
        Run {
            shared,
            cwd,
            had_runs: runs.exists(),
            decks_before,
        }
    }

    /// Scores the current chat explicitly, then starts another task.
    fn end_and_leave(&self, session: &str, score: &str) -> String {
        {
            let core = lock(&self.shared);
            assert!(
                core.reveal().is_none(),
                "the role stays hidden until the run ends"
            );
            let subtitle = core
                .sessions
                .iter()
                .find(|s| s.path.to_string_lossy() == session)
                .map(|s| s.subtitle())
                .unwrap_or_default();
            assert!(
                !subtitle.contains(" · "),
                "the sidebar hides the role: {subtitle:?}"
            );
        }
        lock(&self.shared).end_run();
        press(&self.shared, END_TITLE, text_key(score));
        {
            let core = lock(&self.shared);
            let dialog = core
                .dialog
                .as_ref()
                .expect("a score key alone doesn't end the run");
            let DialogKind::Select(options) = &dialog.kind else {
                panic!("the end-run dialog is a select")
            };
            let chosen = dialog
                .highlight
                .and_then(|i| options.get(i))
                .cloned()
                .unwrap_or_default();
            assert!(
                chosen.starts_with(&format!("{score} · ")),
                "pressing {score} highlights {chosen:?}"
            );
        }
        press(&self.shared, END_TITLE, Key::Enter);
        wait_for(&self.shared, "the scored session", 60, |core| {
            core.t.week.done.is_some()
        });
        lock(&self.shared).request_leave(Leave::New);
        start_unstarted(&self.shared);
        wait_for(&self.shared, "the new session", 60, |core| {
            core.ready && core.session_file.as_deref() != Some(session) && core.t.items.is_empty()
        });
        let log =
            std::fs::read_to_string(config::week_dir().join("runs.jsonl")).expect("runs.jsonl");
        log.lines()
            .filter(|l| l.contains(r#""type":"done""#))
            .find(|l| l.contains(session))
            .expect("a done record")
            .to_owned()
    }

    fn clean(self, session: &str) {
        let runs = config::week_dir().join("runs.jsonl");
        let log = std::fs::read_to_string(&runs).unwrap_or_default();
        let kept: Vec<&str> = log.lines().filter(|l| !l.contains(session)).collect();
        if kept.is_empty() && !self.had_runs {
            let _ = std::fs::remove_file(&runs);
        } else {
            std::fs::write(
                &runs,
                kept.iter().map(|l| format!("{l}\n")).collect::<String>(),
            )
            .expect("clean runs.jsonl");
        }
        if let Some(before) = self.decks_before {
            std::fs::write(config::week_dir().join("decks.json"), before)
                .expect("restore decks.json");
        }
        drop(self.shared);
        let _ = std::fs::remove_dir_all(config::session_dir(&self.cwd));
    }
}

#[test]
#[ignore = "talks to real models through pi"]
fn a_task_is_dealt_answered_ended_and_revealed() {
    let run = Run::start("live-pi-deal");
    let shared = run.shared.clone();
    lock(&shared).submit("/role errand-boy", false);
    lock(&shared).submit("Reply with exactly one word: pong", false);
    wait_for(&shared, "the deal and the answer", 240, |core| {
        core.t.week.deal.is_some() && !core.streaming && core.t.has_answer()
    });

    let (agent, session) = {
        let core = lock(&shared);
        let deal = core.t.week.deal.clone().expect("dealt");
        assert_eq!(deal.role, "errand-boy", "/role forces the role");
        assert!(
            answer_text(&core).to_lowercase().contains("pong"),
            "answer was {:?}",
            answer_text(&core)
        );
        let id = deal.agent.rsplit('/').next().unwrap_or_default().to_owned();
        assert!(
            !format!("{:?}", core.t.items).contains(&id),
            "the transcript never names the agent"
        );
        assert!(core.reveal().is_none(), "no reveal before /done");
        (deal.agent, core.session_file.clone().expect("session file"))
    };

    let done = run.end_and_leave(&session, "5");
    assert!(
        done.contains(r#""score":5"#)
            && done.contains(r#""outcome":"nailed it""#)
            && done.contains(&agent),
        "pressing 5 then enter logs 5, nailed it: {done}"
    );
    run.clean(&session);
    println!(
        "dealt, answered, ended 5/5, revealed: {}",
        config::agent_name(&agent)
    );
}

#[test]
#[ignore = "talks to real models through pi"]
fn a_fanout_shows_every_answer_and_continues_on_the_pick() {
    let run = Run::start("live-pi-fanout");
    let shared = run.shared.clone();
    lock(&shared).submit("/role scribe", false);
    lock(&shared).submit(
        "Write a four-word slogan for a bakery. Reply with the slogan only.",
        false,
    );
    wait_for(&shared, "every contender's answer", 300, |core| {
        let picking = core
            .dialog
            .as_ref()
            .is_some_and(|d| d.title.contains("Which answer"));
        let card = core.t.items.iter().any(
            |i| matches!(i, Item::Fanout { answers, prompt: Some(_), .. } if answers.len() == 3),
        );
        picking && card
    });
    press(&shared, "Which answer", text_key("B"));
    wait_for(&shared, "the deal on the pick", 60, |core| {
        core.t.week.deal.is_some() && core.session_file.is_some()
    });

    let (agent, session) = {
        let core = lock(&shared);
        let Some(Item::Fanout { picked, .. }) = core
            .t
            .items
            .iter()
            .find(|i| matches!(i, Item::Fanout { .. }))
        else {
            panic!("no fan-out card")
        };
        assert_eq!(picked.as_deref(), Some("B"));
        let deal = core.t.week.deal.clone().expect("dealt");
        assert_eq!(deal.role, "scribe");
        (deal.agent, core.session_file.clone().expect("session file"))
    };
    wait_for(
        &shared,
        "the sidebar to list the fan-out session",
        30,
        |core| {
            core.sessions
                .iter()
                .any(|s| s.path.to_string_lossy() == session)
        },
    );

    lock(&shared).submit("Now make it three words.", false);
    wait_for(&shared, "the follow-up on the picked agent", 240, |core| {
        !core.streaming
            && core
                .t
                .items
                .iter()
                .any(|i| matches!(i, Item::Assistant { .. }))
    });

    let done = run.end_and_leave(&session, "3");
    assert!(
        done.contains(r#""score":3"#)
            && done.contains(r#""outcome":"needed my fixes""#)
            && done.contains(&agent),
        "pressing 3 then enter logs 3, needed my fixes: {done}"
    );
    run.clean(&session);
    println!(
        "fan-out picked B, follow-up answered, ended 3/5, revealed: {}",
        config::agent_name(&agent)
    );
}

#[test]
#[ignore = "starts the real pi"]
fn a_bare_launch_still_gives_pi_the_login_env() {
    // A Dock launch inherits launchd's environment, which may lack the shell's API keys.
    unsafe { std::env::remove_var("EXA_API_KEY") };
    let run = Run::start("live-pi-env");
    std::thread::sleep(Duration::from_secs(3));
    let toasts: Vec<String> = lock(&run.shared)
        .toasts
        .iter()
        .map(|t| t.text.clone())
        .collect();
    let cwd = run.cwd.clone();
    drop(run);
    let _ = std::fs::remove_dir_all(config::session_dir(&cwd));
    assert!(
        !toasts.iter().any(|t| t.contains("EXA_API_KEY")),
        "pi started without the login env: {toasts:?}"
    );
}
