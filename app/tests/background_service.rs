//! Real service processes and deterministic pi peers. No models or user data.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use whodunit::config::AppConfig;
use whodunit::core::{
    Core, Leave, SessionState, Shared, connect_background, lock, start_unstarted,
};
use whodunit::service;
use whodunit::transcript::{Block, Item};

fn until(mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !done() {
        assert!(Instant::now() < deadline, "timed out");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn wait(shared: &Shared, done: impl Fn(&Core) -> bool) {
    until(|| {
        let core = lock(shared);
        assert!(core.exited.is_none(), "{:?}", core.exited);
        done(&core)
    });
}

struct Stop;
impl Drop for Stop {
    fn drop(&mut self) {
        let _ = service::stop();
    }
}

fn isolated(test: &str, body: fn(&Path)) {
    if let Some(root) = std::env::var_os("WHODUNIT_SERVICE_TEST") {
        service::ensure_started_with(Path::new(env!("CARGO_BIN_EXE_whodunit"))).unwrap();
        let _stop = Stop;
        return body(Path::new(&root));
    }
    let root = tempfile::tempdir().unwrap();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", test, "--nocapture"])
        .env("HOME", root.path())
        .env("SHELL", "/no-login-shell")
        .env("WHODUNIT_SERVICE_TEST", root.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn app(root: &Path) -> Shared {
    use std::os::unix::fs::PermissionsExt;
    let bin = root.join("pi");
    if !bin.exists() {
        std::fs::copy(
            concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/pi.py"),
            &bin,
        )
        .unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    Arc::new(Mutex::new(Core::background(
        AppConfig {
            pi_bin: bin.to_string_lossy().into_owned(),
            ..AppConfig::default()
        },
        root.to_owned(),
    )))
}

fn new_chat(shared: &Shared, leave: Leave) {
    lock(shared).request_leave(leave);
    start_unstarted(shared);
    wait(shared, |core| core.ready);
}

fn resume(root: &Path) -> Shared {
    let shared = app(root);
    connect_background(&shared, None);
    wait(&shared, |core| core.ready);
    shared
}

fn text(core: &Core) -> String {
    core.t
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Assistant { blocks, .. } => Some(
                blocks
                    .iter()
                    .flatten()
                    .filter_map(|block| match block {
                        Block::Text(text) => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<String>(),
            ),
            _ => None,
        })
        .collect()
}

#[test]
fn working_chats_survive_disconnect_and_finish_without_a_ui() {
    isolated(
        "working_chats_survive_disconnect_and_finish_without_a_ui",
        |root| {
            let shared = app(root);
            new_chat(&shared, Leave::Temporary);
            let cwd = lock(&shared).cwd.clone();
            std::fs::write(cwd.join("scratch.txt"), "keep me").unwrap();
            let shot = whodunit::fixtures::screenshot();
            lock(&shared).submit_with("first task", false, std::slice::from_ref(&shot));
            wait(&shared, |core| core.streaming);
            let first = PathBuf::from(lock(&shared).session_file.clone().unwrap());
            let pid = lock(&shared).stats.cost;

            new_chat(&shared, Leave::NewIn(root.to_owned()));
            lock(&shared).submit("second task", false);
            wait(&shared, |core| core.streaming);
            let second = PathBuf::from(lock(&shared).session_file.clone().unwrap());
            drop(shared);
            assert!(cwd.join("scratch.txt").exists());
            assert_eq!(service::list().unwrap().len(), 2);
            std::fs::write(first.with_extension("release"), "").unwrap();
            std::fs::write(second.with_extension("release"), "").unwrap();
            until(|| {
                std::fs::read_to_string(&first)
                    .unwrap()
                    .contains("assistant")
                    && std::fs::read_to_string(&second)
                        .unwrap()
                        .contains("assistant")
            });

            let reopened = resume(root);
            wait(&reopened, |core| core.sessions.len() == 2);
            new_chat(&reopened, Leave::Open(first.clone()));
            wait(&reopened, |core| !core.working());
            assert_eq!(text(&lock(&reopened)), "first task");
            assert_eq!(lock(&reopened).stats.cost, pid, "same pi process");
            assert!(
                lock(&reopened).t.items.iter().any(
                    |item| matches!(item, Item::User { shots, .. } if shots == std::slice::from_ref(&shot))
                )
            );
            assert!(lock(&reopened).reveal().is_none());
            assert!(!format!("{:?}", lock(&reopened).t.items).contains("secret-model"));
            assert!(cwd.join("scratch.txt").exists());
            new_chat(&reopened, Leave::Open(second));
            assert_eq!(text(&lock(&reopened)), "second task");
            drop(reopened);
            service::stop().unwrap();
            assert!(
                !cwd.exists(),
                "explicit service shutdown cleans scratch folders"
            );
            assert!(first.with_extension("closed").exists());
        },
    );
}

#[test]
fn renamed_chats_persist_through_switches_reconnects_and_service_restarts() {
    isolated(
        "renamed_chats_persist_through_switches_reconnects_and_service_restarts",
        |root| {
            let shared = app(root);
            new_chat(&shared, Leave::NewIn(root.to_owned()));
            lock(&shared).submit("first task", false);
            wait(&shared, |core| core.streaming);
            let first = PathBuf::from(lock(&shared).session_file.clone().unwrap());
            assert!(!lock(&shared).rename_chat("   "));
            assert!(lock(&shared).rename_chat("  My first chat  "));
            wait(&shared, |core| {
                core.t.name.as_deref() == Some("My first chat")
            });
            assert!(lock(&shared).streaming);
            assert!(lock(&shared).reveal().is_none());

            new_chat(&shared, Leave::New);
            lock(&shared).submit("second task", false);
            wait(&shared, |core| core.streaming);
            let second = PathBuf::from(lock(&shared).session_file.clone().unwrap());
            assert!(lock(&shared).rename_chat("My second chat"));
            // The name event may arrive after switching; it must still update only its chat.
            new_chat(&shared, Leave::Open(first.clone()));
            wait(&shared, |core| {
                core.sessions
                    .iter()
                    .any(|s| s.path == second && s.title == "My second chat")
            });
            assert_eq!(lock(&shared).t.name.as_deref(), Some("My first chat"));
            std::fs::write(first.with_extension("release"), "").unwrap();
            wait(&shared, |core| core.can_end_run());
            lock(&shared).end_run();
            wait(&shared, |core| core.dialog.is_some());
            lock(&shared).pick(0);
            wait(&shared, |core| core.t.week.done.is_some());
            assert!(lock(&shared).rename_chat("Finished first chat"));
            wait(&shared, |core| {
                core.t.name.as_deref() == Some("Finished first chat")
            });
            drop(shared);

            let reopened = resume(root);
            new_chat(&reopened, Leave::Open(first.clone()));
            assert_eq!(
                lock(&reopened).t.name.as_deref(),
                Some("Finished first chat")
            );
            assert!(lock(&reopened).t.week.done.is_some());
            drop(reopened);
            service::stop().unwrap();
            service::ensure_started_with(Path::new(env!("CARGO_BIN_EXE_whodunit"))).unwrap();

            let reopened = app(root);
            new_chat(&reopened, Leave::Open(first));
            assert_eq!(
                lock(&reopened).t.name.as_deref(),
                Some("Finished first chat")
            );
            assert!(lock(&reopened).t.week.done.is_some());
            new_chat(&reopened, Leave::Open(second));
            assert_eq!(lock(&reopened).t.name.as_deref(), Some("My second chat"));
        },
    );
}

#[test]
fn inline_titles_save_on_enter_blur_and_navigation_but_escape_discards() {
    isolated(
        "inline_titles_save_on_enter_blur_and_navigation_but_escape_discards",
        |root| {
            use rune::{Env, Key, KeyEvent, Mods, Runtime};
            use whodunit::ui::window::{self, Window};

            fn click(rt: &mut Runtime<Window>, key: &str) {
                let (x, y) = rt.find(key).unwrap();
                rt.pointer_down(x, y);
                rt.pointer_up(x, y);
                rt.frame(0.016);
                rt.frame(0.016);
            }

            let shared = app(root);
            new_chat(&shared, Leave::NewIn(root.to_owned()));
            lock(&shared).submit("first task", false);
            wait(&shared, |core| core.streaming);
            let file = PathBuf::from(lock(&shared).session_file.clone().unwrap());
            let sidebar = format!("session-{}", file.display());
            let mut rt = Runtime::new(
                Window::new(shared.clone()),
                window::view,
                (1180.0, 780.0),
                Env::new(true, 1.0),
            );
            rt.poll = Some(window::poll);
            rt.on_key = Some(window::on_key);
            rt.frame(0.0);
            rt.frame(1.0);
            let mut saved = String::new();
            for target in ["title", sidebar.as_str()] {
                for action in ["enter", "blur", "escape"] {
                    click(&mut rt, target);
                    let name = format!("{target}-{action}");
                    rt.key(Key::Text(name.clone()).into());
                    rt.frame(0.016);
                    match action {
                        "enter" => rt.key(Key::Enter.into()),
                        "escape" => rt.key(Key::Escape.into()),
                        _ => click(&mut rt, "input"),
                    }
                    rt.frame(0.016);
                    rt.frame(0.016);
                    if action == "escape" {
                        assert_eq!(lock(&shared).t.name.as_deref(), Some(saved.as_str()));
                        assert!(!std::fs::read_to_string(&file).unwrap().contains(&name));
                    } else {
                        wait(&shared, |core| core.t.name.as_deref() == Some(&name));
                        saved = name;
                        rt.frame(0.016);
                    }
                    assert!(lock(&shared).streaming);
                    assert!(lock(&shared).reveal().is_none());
                }
            }

            click(&mut rt, "title");
            rt.key(Key::Text("Saved before editing the sidebar".into()).into());
            rt.frame(0.016);
            click(&mut rt, &sidebar);
            wait(&shared, |core| {
                core.t.name.as_deref() == Some("Saved before editing the sidebar")
            });
            rt.key(Key::Text("Discard this sidebar edit".into()).into());
            rt.frame(0.016);
            rt.key(Key::Escape.into());
            rt.frame(0.016);
            assert_eq!(
                lock(&shared).t.name.as_deref(),
                Some("Saved before editing the sidebar")
            );

            click(&mut rt, "title");
            rt.key(Key::Text("Saved before switching".into()).into());
            rt.frame(0.016);
            rt.key(KeyEvent {
                key: Key::Command('n'),
                mods: Mods {
                    cmd: true,
                    ..Mods::default()
                },
            });
            rt.frame(0.016);
            rt.frame(0.016);
            wait(&shared, |core| {
                core.ready && core.session_file.as_deref() != file.to_str()
            });
            new_chat(&shared, Leave::Open(file));
            assert_eq!(
                lock(&shared).t.name.as_deref(),
                Some("Saved before switching")
            );
        },
    );
}

#[test]
fn a_pending_score_reconnects_and_is_answered_once() {
    isolated("a_pending_score_reconnects_and_is_answered_once", |root| {
        let shared = app(root);
        new_chat(&shared, Leave::New);
        lock(&shared).submit("score me", false);
        wait(&shared, |core| core.streaming);
        let path = PathBuf::from(lock(&shared).session_file.clone().unwrap());
        std::fs::write(path.with_extension("release"), "").unwrap();
        wait(&shared, |core| core.can_end_run());
        lock(&shared).end_run();
        wait(&shared, |core| core.dialog.is_some());
        let dialog = lock(&shared).dialog.as_ref().unwrap().id.clone();
        drop(shared);

        let reopened = resume(root);
        assert_eq!(lock(&reopened).dialog.as_ref().unwrap().id, dialog);
        assert!(lock(&reopened).reveal().is_none());
        lock(&reopened).pick(0);
        wait(&reopened, |core| core.t.week.done.is_some());
        assert!(lock(&reopened).reveal().is_some());
        drop(reopened);
        let reopened = resume(root);
        assert!(
            lock(&reopened).dialog.is_none(),
            "answered dialogs don't reappear"
        );
        assert_eq!(
            std::fs::read_to_string(path)
                .unwrap()
                .matches("whodunit.done")
                .count(),
            1
        );
    });
}

#[test]
fn classification_without_a_session_file_reconnects_and_can_be_stopped() {
    isolated(
        "classification_without_a_session_file_reconnects_and_can_be_stopped",
        |root| {
            std::fs::write(root.join("defer-session"), "").unwrap();
            let shared = app(root);
            new_chat(&shared, Leave::Temporary);
            let cwd = lock(&shared).cwd.clone();
            std::fs::write(cwd.join("scratch.txt"), "keep through retry").unwrap();
            std::fs::write(root.join("stall-dealing"), "").unwrap();
            let shot = whodunit::fixtures::screenshot();
            lock(&shared).submit_with("classify this", false, std::slice::from_ref(&shot));
            until(|| root.join("stalled-pid").exists());
            assert!(lock(&shared).session_file.is_none());
            drop(shared);

            let reopened = resume(root);
            assert!(lock(&reopened).dealing());
            assert_eq!(lock(&reopened).sessions.len(), 1);
            let summary = lock(&reopened).sessions[0].clone();
            assert_eq!(
                lock(&reopened).session_state(&summary),
                SessionState::Working
            );
            let (prompt, images) = lock(&reopened).cancel_deal().unwrap();
            assert!(
                !lock(&reopened)
                    .sessions
                    .iter()
                    .any(|s| s.path == summary.path)
            );
            assert_eq!(prompt, "classify this");
            assert_eq!(images, [shot]);
            std::fs::remove_file(root.join("defer-session")).unwrap();
            lock(&reopened).submit_with(&prompt, false, &images);
            start_unstarted(&reopened);
            wait(&reopened, |core| core.streaming);
            assert_eq!(lock(&reopened).cwd, cwd);
            assert!(cwd.join("scratch.txt").exists());
            let path = PathBuf::from(lock(&reopened).session_file.clone().unwrap());
            std::fs::write(path.with_extension("release"), "").unwrap();
            wait(&reopened, |core| !core.working() && text(core) == prompt);
        },
    );
}

#[test]
fn streaming_state_and_queues_reconnect_without_replaying_output() {
    isolated(
        "streaming_state_and_queues_reconnect_without_replaying_output",
        |root| {
            std::fs::write(root.join("stream-fixture"), "").unwrap();
            let shared = app(root);
            new_chat(&shared, Leave::New);
            lock(&shared).submit("final answer", false);
            wait(&shared, |core| core.statuses.contains_key("compaction"));
            lock(&shared).submit("queued steer", false);
            lock(&shared).submit("queued follow-up", true);
            wait(&shared, |core| core.queue.1.len() == 1);
            let path = PathBuf::from(lock(&shared).session_file.clone().unwrap());
            let elapsed = lock(&shared).turn_started.unwrap().elapsed();
            drop(shared);

            let reopened = resume(root);
            {
                let core = lock(&reopened);
                assert!(core.streaming);
                assert_eq!(text(&core), "partial\u{2028}answer\u{2029}");
                assert!(core.turn_started.unwrap().elapsed() >= elapsed);
                assert!(core.t.tools["tool-1"].running);
                assert_eq!(core.t.tools["tool-1"].output, "still running");
                assert_eq!(
                    core.queue,
                    (
                        vec!["queued steer".to_owned()],
                        vec!["queued follow-up".to_owned()]
                    )
                );
                assert_eq!(
                    core.widgets["fanout"],
                    ["fan-out · A running · B done · 12s"]
                );
                assert!(core.statuses.contains_key("compaction"));
                assert!(core.reveal().is_none());
            }
            // Attach a second view while the first remains connected. A snapshot doesn't
            // replay text_delta or send prompts, and both views see the same completion.
            let other = resume(root);
            assert_eq!(text(&lock(&other)), "partial\u{2028}answer\u{2029}");
            lock(&other).abort();
            wait(&reopened, |core| {
                !core.streaming && core.queue.0.is_empty() && core.queue.1.is_empty()
            });
            assert_eq!(
                lock(&reopened).editor_text.as_ref().unwrap().1,
                "queued steer\nqueued follow-up"
            );
            assert_eq!(
                std::fs::read_to_string(path)
                    .unwrap()
                    .matches("\"role\":\"user\"")
                    .count(),
                1
            );
        },
    );
}

#[test]
fn killing_the_ui_process_does_not_close_pi() {
    if let Some(root) = std::env::var_os("WHODUNIT_CRASH_WORKER") {
        let root = Path::new(&root);
        let shared = app(root);
        new_chat(&shared, Leave::New);
        lock(&shared).submit("survive a crash", false);
        wait(&shared, |core| core.streaming);
        let path = lock(&shared).session_file.clone().unwrap();
        std::fs::write(root.join("client-ready"), path).unwrap();
        loop {
            std::thread::sleep(Duration::from_secs(1));
        }
    }
    isolated("killing_the_ui_process_does_not_close_pi", |root| {
        let mut worker = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "killing_the_ui_process_does_not_close_pi",
                "--nocapture",
            ])
            .env("WHODUNIT_CRASH_WORKER", root)
            .spawn()
            .unwrap();
        until(|| root.join("client-ready").exists());
        let path = PathBuf::from(std::fs::read_to_string(root.join("client-ready")).unwrap());
        worker.kill().unwrap();
        worker.wait().unwrap();
        assert!(!path.with_extension("closed").exists());
        std::fs::write(path.with_extension("release"), "").unwrap();
        let reopened = resume(root);
        wait(&reopened, |core| {
            !core.working() && text(core) == "survive a crash"
        });
        assert_eq!(service::list().unwrap().len(), 1);
    });
}

#[test]
fn a_crashed_service_leaves_a_socket_that_the_next_launch_can_replace() {
    isolated(
        "a_crashed_service_leaves_a_socket_that_the_next_launch_can_replace",
        |_root| {
            service::stop().unwrap();
            let mut process = std::process::Command::new(env!("CARGO_BIN_EXE_whodunit"))
                .arg("--background-service")
                .spawn()
                .unwrap();
            until(|| service::list().is_ok());
            process.kill().unwrap();
            process.wait().unwrap();
            assert!(service::list().is_err());
            service::ensure_started_with(Path::new(env!("CARGO_BIN_EXE_whodunit"))).unwrap();
            assert!(service::list().unwrap().is_empty());
        },
    );
}

#[test]
fn concurrent_launchers_and_saved_session_opens_share_one_process() {
    isolated(
        "concurrent_launchers_and_saved_session_opens_share_one_process",
        |root| {
            service::stop().unwrap();
            std::thread::scope(|scope| {
                for _ in 0..4 {
                    scope.spawn(|| {
                        service::ensure_started_with(Path::new(env!("CARGO_BIN_EXE_whodunit")))
                            .unwrap()
                    });
                }
            });
            let a = app(root);
            new_chat(&a, Leave::New);
            lock(&a).submit("shared session", false);
            wait(&a, |core| core.streaming);
            let path = PathBuf::from(lock(&a).session_file.clone().unwrap());
            let b = app(root);
            lock(&b).refresh_sessions();
            new_chat(&b, Leave::Open(path.clone()));
            assert_eq!(service::list().unwrap().len(), 1);
            assert_eq!(lock(&a).stats.cost, lock(&b).stats.cost);
            lock(&b).abort();
            wait(&a, |core| !core.streaming);
            wait(&b, |core| !core.streaming);
            assert!(!path.with_extension("closed").exists());
        },
    );
}
