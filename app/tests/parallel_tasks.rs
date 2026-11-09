//! Process-level session isolation without real pi or model calls.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rune::Tx;
use whodunit::config::AppConfig;
use whodunit::core::{Core, Leave, SessionState, Shared, lock, start_pi};
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
    let (cwd, session) = lock(shared)
        .relaunch
        .take()
        .expect("navigation is immediate, even for an open run");
    start_pi(shared, Some(cwd), session);
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

#[test]
fn chats_keep_working_independently() {
    // HOME and login-shell discovery are isolated in a child test process, not changed in a
    // multithreaded test runner. start_pi must never write to the user's app.json here.
    if std::env::var_os("WHODUNIT_FAKE_HOME").is_none() {
        let home = std::env::temp_dir().join(format!("whodunit-parallel-{}", std::process::id()));
        std::fs::create_dir_all(&home).unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "chats_keep_working_independently", "--nocapture"])
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
        return;
    }
    run_chats();
}

#[cfg(unix)]
fn run_chats() {
    use std::os::unix::fs::PermissionsExt;
    let root = PathBuf::from(std::env::var_os("WHODUNIT_FAKE_HOME").unwrap());
    let bin = root.join("pi");
    std::fs::copy(
        concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/pi.py"),
        &bin,
    )
    .unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    let shared = Arc::new(Mutex::new(Core::new(
        AppConfig {
            pi_bin: bin.to_string_lossy().into_owned(),
            ..AppConfig::default()
        },
        root.clone(),
    )));
    start_pi(&shared, None, None);
    wait(&shared, |core| core.ready);
    let mut ui = FullUi::new(shared.clone());
    full::poll(&mut ui, 0.0, &mut Tx::default());
    ui.composer.replace_text("unsent follow-up for A");
    lock(&shared).submit("task A", false);
    wait(&shared, |core| core.streaming && core.t.week.deal.is_some());
    let a = PathBuf::from(lock(&shared).session_file.clone().unwrap());
    let a_id = lock(&shared).task_id();
    assert_eq!(state(&shared, &a), SessionState::Working);

    navigate(&shared, Leave::New);
    full::poll(&mut ui, 0.0, &mut Tx::default());
    assert!(
        ui.composer.text.is_empty(),
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
    start_pi(&shared, Some(other.clone()), None);
    wait(&shared, |core| core.ready);
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
