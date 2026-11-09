//! Process-level session isolation without real pi or model calls.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rune::Tx;
use serde_json::json;
use whodunit::config::AppConfig;
use whodunit::core::{Core, Leave, SessionState, Shared, lock, start_unstarted};
use whodunit::transcript::{Block, Item};
use whodunit::ui::full::{self, FullUi};

fn wait(shared: &Shared, done: impl Fn(&Core) -> bool) {
    let until = Instant::now() + Duration::from_secs(15);
    loop {
        {
            let core = lock(shared);
            assert!(core.exited.is_none(), "pi exited: {:?}", core.exited);
            if done(&core) {
                return;
            }
        }
        assert!(Instant::now() < until, "timed out");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn navigate(shared: &Shared, leave: Leave) {
    lock(shared).request_leave(leave);
    start_unstarted(shared);
    wait(shared, |core| core.ready);
}

fn state(shared: &Shared, path: &Path) -> SessionState {
    let core = lock(shared);
    let summary = core
        .sessions
        .iter()
        .find(|s| s.path == path)
        .expect("live session listed");
    core.session_state(summary)
}

fn answer(core: &Core) -> String {
    core.t
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Assistant { blocks, .. } => Some(
                blocks
                    .iter()
                    .flatten()
                    .filter_map(|b| match b {
                        Block::Text(text) => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<String>(),
            ),
            _ => None,
        })
        .collect()
}

/// Runs `body` in a child test process with its own HOME and no login shell, rather than
/// changing them in a multithreaded test runner. Starting pi must never write to the user's app.json.
fn isolated(test: &str, body: fn(&Path)) {
    if let Some(home) = std::env::var_os("WHODUNIT_FAKE_HOME") {
        return body(Path::new(&home));
    }
    let home = std::env::temp_dir().join(format!("whodunit-{test}-{}", std::process::id()));
    std::fs::create_dir_all(&home).unwrap();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", test, "--nocapture"])
        .env("HOME", &home)
        .env("SHELL", "/no-login-shell")
        .env("WHODUNIT_FAKE_HOME", &home)
        .output()
        .unwrap();
    let _ = std::fs::remove_dir_all(home);
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// The app's shared state, running the deterministic RPC peer as pi.
#[cfg(unix)]
fn app(root: &Path) -> Shared {
    use std::os::unix::fs::PermissionsExt;
    let bin = root.join("pi");
    std::fs::copy(
        concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/pi.py"),
        &bin,
    )
    .unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    Arc::new(Mutex::new(Core::new(
        AppConfig {
            pi_bin: bin.to_string_lossy().into_owned(),
            ..AppConfig::default()
        },
        root.to_owned(),
    )))
}

#[test]
fn chats_keep_working_independently() {
    isolated("chats_keep_working_independently", run_chats);
}

#[cfg(unix)]
fn run_chats(root: &Path) {
    let shared = app(root);
    // The first task is typed before its process has started; pi reads it once it has.
    lock(&shared).request_leave(Leave::New);
    lock(&shared).submit("task A", false);
    assert!(
        lock(&shared).toasts.is_empty(),
        "{:?}",
        lock(&shared).toasts
    );
    start_unstarted(&shared);
    let mut ui = FullUi::new(shared.clone());
    full::poll(&mut ui, 0.0, &mut Tx::default());
    ui.composer.replace_text("unsent follow-up for A");
    ui.attachments = vec![whodunit::fixtures::screenshot()];
    wait(&shared, |core| core.streaming && core.t.week.deal.is_some());
    let a = PathBuf::from(lock(&shared).session_file.clone().unwrap());
    let a_id = lock(&shared).task_id();
    assert_eq!(state(&shared, &a), SessionState::Working);

    navigate(&shared, Leave::New);
    full::poll(&mut ui, 0.0, &mut Tx::default());
    assert!(
        ui.composer.text.is_empty() && ui.attachments.is_empty(),
        "a new task starts with an empty composer"
    );
    ui.composer.replace_text("unsent follow-up for B");
    let b_id = lock(&shared).task_id();
    assert_ne!(a_id, b_id);
    lock(&shared).submit("task B", false);
    wait(&shared, |core| core.streaming && core.t.week.deal.is_some());
    let b = PathBuf::from(lock(&shared).session_file.clone().unwrap());
    assert_eq!(state(&shared, &a), SessionState::Working);
    assert_eq!(state(&shared, &b), SessionState::Working);

    // A finishes offscreen. Its answer, stats and idle state must not replace B's.
    std::fs::write(a.with_extension("release"), "").unwrap();
    let until = Instant::now() + Duration::from_secs(15);
    while state(&shared, &a) != SessionState::Idle {
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(lock(&shared).task_id(), b_id);
    assert!(lock(&shared).streaming);
    assert!(answer(&lock(&shared)).is_empty());

    navigate(&shared, Leave::Open(a.clone()));
    full::poll(&mut ui, 0.0, &mut Tx::default());
    assert_eq!(ui.composer.text, "unsent follow-up for A");
    assert_eq!(ui.attachments, [whodunit::fixtures::screenshot()]);
    assert_eq!(lock(&shared).task_id(), a_id, "reuse A's process");
    assert_eq!(answer(&lock(&shared)), "task A");
    assert!(lock(&shared).reveal().is_none());
    assert!(!format!("{:?}", lock(&shared).t.items).contains("secret-model"));
    let a_cost = lock(&shared).stats.cost;
    lock(&shared).end_run();
    wait(&shared, |core| core.dialog.is_some());
    assert_eq!(
        state(&shared, &a),
        SessionState::Idle,
        "waiting on a score is not working"
    );

    // Score dialog remains attached to A while visiting B, including across folders.
    navigate(&shared, Leave::Open(b.clone()));
    full::poll(&mut ui, 0.0, &mut Tx::default());
    assert_eq!(ui.composer.text, "unsent follow-up for B");
    assert!(lock(&shared).dialog.is_none());
    assert_ne!(lock(&shared).stats.cost, a_cost);
    let other = root.join("another-project");
    std::fs::create_dir_all(&other).unwrap();
    navigate(&shared, Leave::NewIn(other.clone()));
    assert_eq!(lock(&shared).cwd, other);
    lock(&shared).submit("task C", false);
    wait(&shared, |core| core.streaming && core.t.week.deal.is_some());
    let c = PathBuf::from(lock(&shared).session_file.clone().unwrap());
    assert_eq!(state(&shared, &b), SessionState::Working);
    navigate(&shared, Leave::Open(a.clone()));
    assert_eq!(lock(&shared).cwd, root);
    assert!(lock(&shared).dialog.is_some());
    lock(&shared).pick(0);
    wait(&shared, |core| core.t.week.done.is_some());
    assert_eq!(state(&shared, &a), SessionState::Ended);
    assert!(lock(&shared).reveal().is_some());
    lock(&shared).submit("not another task in this chat", false);
    assert!(
        lock(&shared)
            .toasts
            .iter()
            .any(|t| t.text.contains("ended"))
    );
    assert_eq!(state(&shared, &b), SessionState::Working);

    navigate(&shared, Leave::Open(b.clone()));
    lock(&shared).abort();
    wait(&shared, |core| !core.streaming);
    assert_eq!(state(&shared, &b), SessionState::Idle);
    assert_eq!(state(&shared, &a), SessionState::Ended);
    assert_eq!(
        state(&shared, &c),
        SessionState::Working,
        "aborting B doesn't stop C"
    );
    assert!(lock(&shared).reveal().is_none());
    std::fs::write(c.with_extension("release"), "").unwrap();
    navigate(&shared, Leave::Open(c.clone()));
    wait(&shared, |core| !core.streaming && core.t.has_answer());
    assert_eq!(answer(&lock(&shared)), "task C");
    assert_eq!(lock(&shared).cwd, other);

    // Reader callbacks hold weak references so dropping the app closes all child processes.
    drop(ui);
    drop(shared);
    let until = Instant::now() + Duration::from_secs(5);
    while !a.with_extension("closed").exists()
        || !b.with_extension("closed").exists()
        || !c.with_extension("closed").exists()
    {
        assert!(Instant::now() < until, "pi children were not shut down");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn a_saved_chat_opens_whole_before_pi_answers() {
    isolated("a_saved_chat_opens_whole_before_pi_answers", open_saved);
}

#[cfg(unix)]
fn open_saved(root: &Path) {
    let shared = app(root);
    // A run scored in an earlier launch, with an answer the user branched away from.
    let dir = root.join(".pi/agent/sessions/--saved--");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("saved.jsonl");
    let lines = [
        json!({"type": "session", "version": 3, "id": "s", "cwd": root}),
        json!({"type": "message", "id": "1", "parentId": null, "message": {"role": "user", "content": "saved task"}}),
        json!({"type": "custom", "id": "2", "parentId": "1", "customType": "whodunit.deal", "data": {"role": "builder", "size": "S", "agent": "secret-lab/secret-model"}}),
        json!({"type": "message", "id": "3", "parentId": "2", "message": {"role": "assistant", "content": [{"type": "text", "text": "abandoned answer"}]}}),
        json!({"type": "message", "id": "4", "parentId": "2", "message": {"role": "assistant", "content": [{"type": "text", "text": "saved answer"}]}}),
        json!({"type": "custom", "id": "5", "parentId": "4", "customType": "whodunit.done", "data": {"score": 4}}),
    ];
    let text: String = lines.iter().map(|line| format!("{line}\n")).collect();
    std::fs::write(&path, text).unwrap();
    lock(&shared).refresh_sessions();
    assert_eq!(state(&shared, &path), SessionState::Ended);

    // Opened from the sidebar, it shows whole and stays ended before its pi process exists.
    lock(&shared).request_leave(Leave::Open(path.clone()));
    {
        let core = lock(&shared);
        assert_eq!(
            core.unstarted.len(),
            1,
            "its process starts off the UI thread"
        );
        assert_eq!(answer(&core), "saved answer");
        assert!(core.reveal().is_some());
    }
    assert_eq!(state(&shared, &path), SessionState::Ended);

    start_unstarted(&shared);
    wait(&shared, |core| core.ready && core.loads == 1);
    assert_eq!(answer(&lock(&shared)), "saved answer");
    assert_eq!(state(&shared, &path), SessionState::Ended);
}

#[test]
fn a_chat_whose_pi_cannot_start_stops_waiting() {
    isolated("a_chat_whose_pi_cannot_start_stops_waiting", cannot_start);
}

fn cannot_start(root: &Path) {
    let shared = Arc::new(Mutex::new(Core::new(
        AppConfig {
            pi_bin: root.join("no-pi-here").to_string_lossy().into_owned(),
            ..AppConfig::default()
        },
        root.to_owned(),
    )));
    lock(&shared).request_leave(Leave::New);
    lock(&shared).submit("typed before pi started", false);
    start_unstarted(&shared);
    let core = lock(&shared);
    assert!(
        core.exited
            .as_deref()
            .is_some_and(|e| e.contains("no-pi-here"))
    );
    assert!(core.pending_prompt.is_none(), "nothing is left classifying");
}

#[test]
fn an_image_goes_out_with_its_prompt_and_comes_back_in_the_chat() {
    isolated(
        "an_image_goes_out_with_its_prompt_and_comes_back_in_the_chat",
        send_image,
    );
}

#[cfg(unix)]
fn send_image(root: &Path) {
    let shared = app(root);
    let shot = whodunit::fixtures::screenshot();
    lock(&shared).request_leave(Leave::New);
    start_unstarted(&shared);
    wait(&shared, |core| core.ready);
    // A shell command doesn't take images, so the composer keeps them.
    assert!(!lock(&shared).submit_with("!true", false, std::slice::from_ref(&shot)));
    assert!(lock(&shared).submit_with("", false, std::slice::from_ref(&shot)));
    assert_eq!(lock(&shared).pending_shots, std::slice::from_ref(&shot));
    wait(&shared, |core| {
        core.t
            .items
            .iter()
            .any(|item| matches!(item, Item::User { shots, .. } if *shots == [shot.clone()]))
    });
    let path = PathBuf::from(lock(&shared).session_file.clone().unwrap());
    std::fs::write(path.with_extension("release"), "").unwrap();
    wait(&shared, |core| !core.streaming && core.t.has_answer());
    // Context counts arrive with the turn, not just at its end, and read against the
    // point where pi compacts (272k window, 16k reserve by default).
    let percent = lock(&shared).stats.context_percent().unwrap();
    assert!((percent - 30_000.0 / (272_000.0 - 16_384.0) * 100.0).abs() < 0.01);

    // Reopened from its file, the image is still there.
    let saved = whodunit::sessions::transcript(&path).unwrap();
    assert!(
        saved
            .items
            .iter()
            .any(|item| matches!(item, Item::User { shots, .. } if *shots == [shot.clone()]))
    );
}
