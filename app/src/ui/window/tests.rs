use super::*;
use crate::config;
use crate::core::SessionState;
use crate::sessions::Summary;
use crate::transcript::{Item, Transcript};
use rune::{Env, Mods, Runtime};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

fn core(scene: fn(&Shared)) -> Shared {
    let core = Arc::new(Mutex::new(Core::new(
        config::AppConfig::default(),
        "/fixture".into(),
    )));
    scene(&core);
    core
}

fn transcript_of(scene: fn(&Shared)) -> Transcript {
    std::mem::take(&mut lock(&core(scene)).t)
}

fn window(core: Shared) -> Runtime<Window> {
    let mut rt = Runtime::new(
        Window::new(core),
        view,
        (1180.0, 780.0),
        Env::new(true, 1.0),
    );
    rt.poll = Some(poll);
    rt.on_key = Some(on_key);
    rt.frame(0.0);
    rt.frame(1.0);
    rt
}

/// What the frame on screen still has in motion: anything moving, fading, growing or
/// revealing, or drawn otherwise than once every track has landed. A running tool's
/// spinner loops on purpose.
fn in_motion(rt: &mut Runtime<Window>) -> Vec<String> {
    let first = rt.elements();
    rt.frame(1.0);
    let settled: HashMap<String, f32> = rt
        .elements()
        .into_iter()
        .map(|e| (e.path, e.opacity))
        .collect();
    first
        .into_iter()
        .filter(|e| e.loop_phase.is_none())
        .filter(|e| {
            e.moving
                || e.revealing
                || e.in_growing_row
                || e.exiting
                || settled.get(&e.path) != Some(&e.opacity)
        })
        .map(|e| e.path)
        .collect()
}

fn shows(rt: &Runtime<Window>, path: &str) -> bool {
    rt.elements().iter().any(|e| e.path.ends_with(path))
}

fn title(rt: &Runtime<Window>) -> Option<String> {
    let elements = rt.elements();
    let title = elements
        .iter()
        .find(|e| e.key.as_deref() == Some("title"))?;
    title.text.clone()
}

/// Unit tests never start pi: that would run the login shell and write the real app.json.
fn unstarted(core: &Shared) -> usize {
    std::mem::take(&mut lock(core).unstarted).len()
}

/// A press and release on the element, and the frame drawn right after them.
fn click(rt: &mut Runtime<Window>, key: &str) {
    let (x, y) = rt.find(key).unwrap();
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(0.016);
}

fn summary(path: &Path, t: &Transcript) -> Summary {
    Summary {
        path: path.to_owned(),
        cwd: "/fixture".into(),
        title: t.title().unwrap(),
        week: t.week.clone(),
        updated: std::time::SystemTime::now(),
    }
}

/// A run scored in an earlier launch, saved where no live chat has it open.
fn saved_chat(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("whodunit-{name}-{}.jsonl", std::process::id()));
    let lines = [
        r#"{"type":"session","version":3,"id":"s","cwd":"/fixture"}"#,
        r#"{"type":"message","id":"1","parentId":null,"message":{"role":"user","content":"Saved task"}}"#,
        r#"{"type":"custom","id":"2","parentId":"1","customType":"whodunit.deal","data":{"role":"builder","size":"S","agent":"lab/model"}}"#,
        r#"{"type":"message","id":"3","parentId":"2","message":{"role":"assistant","content":[{"type":"text","text":"Saved answer"}]}}"#,
        r#"{"type":"custom","id":"4","parentId":"3","customType":"whodunit.done","data":{"score":4}}"#,
    ];
    std::fs::write(&path, lines.join("\n")).unwrap();
    path
}

#[test]
fn the_composer_can_remove_its_folder_without_losing_the_draft() {
    let shared = core(crate::fixtures::scene_empty);
    let mut rt = window(shared.clone());
    rt.poll = None;
    let shot = crate::fixtures::screenshot();
    rt.update(|s, _| {
        s.draft
            .composer
            .replace_text("A question without a project");
        s.draft.attachments = vec![shot.clone()];
    });
    rt.frame(0.016);
    assert!(shows(&rt, "/remove-folder"));
    click(&mut rt, "remove-folder");
    assert!(rt.state.draft.composer.focused);
    assert_eq!(rt.state.draft.composer.text, "A question without a project");
    assert_eq!(rt.state.draft.attachments, vec![shot]);
    assert!(config::is_temporary(&lock(&shared).cwd));
    assert_eq!(unstarted(&shared), 1);
    assert!(!shows(&rt, "/remove-folder"));
    assert!(shows(&rt, "/workspace"));
    assert!(
        rt.elements()
            .iter()
            .any(|e| e.text.as_deref() == Some("No folder"))
    );
}

#[test]
fn a_started_chat_does_not_offer_to_remove_its_folder() {
    let rt = window(core(crate::fixtures::scene_chat));
    assert!(!shows(&rt, "/remove-folder"));
}

#[test]
fn dealing_can_be_stopped_by_button_or_escape_with_the_prompt_and_images_restored() {
    for (by_key, focused, draft) in [
        (false, false, ""),
        (false, true, "An unsent addition"),
        (true, false, ""),
        (true, true, "An unsent addition"),
    ] {
        let shared = core(crate::fixtures::scene_empty);
        let shot = crate::fixtures::screenshot();
        lock(&shared).submit_with("Check this screen", false, std::slice::from_ref(&shot));
        let mut rt = window(shared.clone());
        // Cancellation queues a replacement pi; unit tests must never start it.
        rt.poll = None;
        rt.update(|s, _| {
            s.draft.composer.replace_text(draft);
            s.draft.composer.focused = focused;
            if !draft.is_empty() {
                s.draft.attachments.push(shot.clone());
            }
        });
        rt.frame(0.016);
        if focused {
            let (x, y) = rt.find("input").unwrap();
            rt.pointer_down(x, y);
            rt.pointer_up(x, y);
        }
        assert!(shows(&rt, "/stage-deal"));
        assert!(lock(&shared).can_stop());
        if by_key {
            rt.key(Key::Escape.into());
            rt.frame(0.016);
        } else {
            click(&mut rt, "send");
        }
        assert_eq!(unstarted(&shared), 1);
        assert!(!lock(&shared).can_stop());
        assert!(lock(&shared).reveal().is_none());
        assert!(rt.state.draft.composer.focused);
        assert_eq!(
            rt.state.draft.composer.text,
            if draft.is_empty() {
                "Check this screen".to_owned()
            } else {
                format!("Check this screen\n\n{draft}")
            }
        );
        assert_eq!(
            rt.state.draft.attachments,
            vec![shot; if draft.is_empty() { 1 } else { 2 }]
        );
        rt.frame(1.0);
        assert!(!shows(&rt, "/stage-deal"));
        click(&mut rt, "send");
        let core = lock(&shared);
        assert!(core.dealing());
        assert_eq!(core.sent_before_start().len(), 1);
        assert!(rt.state.draft.composer.text.is_empty());
        assert!(rt.state.draft.attachments.is_empty());
    }
}

#[test]
fn enter_during_dealing_keeps_the_unsent_draft() {
    let shared = core(crate::fixtures::scene_dealing);
    let mut rt = window(shared.clone());
    rt.update(|s, _| {
        s.draft.composer.replace_text("Don't lose this");
        s.draft.composer.focused = true;
    });
    rt.frame(0.016);
    let (x, y) = rt.find("input").unwrap();
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.key(Key::Enter.into());
    rt.frame(0.016);
    assert_eq!(rt.state.draft.composer.text, "Don't lose this");
    assert!(lock(&shared).sent_before_start().is_empty());
    assert!(lock(&shared).dealing());
}

#[test]
fn ending_lives_below_the_composer_and_command_return_never_sends_the_draft() {
    let core = core(crate::fixtures::scene_chat);
    let mut rt = window(core.clone());
    let elements = rt.elements();
    let rect = |key: &str| {
        elements
            .iter()
            .find(|e| e.key.as_deref() == Some(key))
            .unwrap()
            .rect
    };
    let composer = rect("composer-frame");
    assert!(rect("end-run")[1] >= composer[1] + composer[3]);
    rt.update(|s, _| s.draft.composer.replace_text("An unsent draft"));
    rt.key(KeyEvent {
        key: Key::Enter,
        mods: Mods {
            cmd: true,
            ..Mods::default()
        },
    });
    rt.frame(0.016);
    assert_eq!(rt.state.draft.composer.text, "An unsent draft");
    let core = lock(&core);
    let sent = core.sent_before_start();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0]["message"], "/done");
    assert!(core.t.week.done.is_none());
    assert!(core.reveal().is_none());
    assert!(rt.state.seal.progress().is_none());
}

#[test]
fn command_shift_return_still_queues_followups() {
    let core = core(crate::fixtures::scene_working);
    let mut rt = window(core.clone());
    rt.update(|s, _| s.draft.composer.replace_text("Next, run the tests"));
    rt.key(KeyEvent {
        key: Key::Enter,
        mods: Mods {
            cmd: true,
            shift: true,
            ..Mods::default()
        },
    });
    rt.frame(0.016);
    let core = lock(&core);
    let sent = core.sent_before_start();
    let last = sent.last().unwrap();
    assert_eq!(last["message"], "Next, run the tests");
    assert_eq!(last["streamingBehavior"], "followUp");
}

#[test]
fn ending_requires_a_score_and_explicit_confirmation_and_can_be_cancelled() {
    let core = core(crate::fixtures::scene_end);
    lock(&core).dialog.as_mut().unwrap().highlight = None;
    let mut rt = window(core.clone());
    assert!(shows(&rt, "/confirm-end"));
    assert!(shows(&rt, "/keep-working"));
    rt.key(Key::Enter.into());
    rt.frame(0.016);
    assert!(
        lock(&core).dialog.is_some(),
        "Enter without a score does nothing"
    );
    rt.key(Key::Text("4".into()).into());
    rt.frame(0.016);
    assert_eq!(lock(&core).dialog.as_ref().unwrap().highlight, Some(1));
    assert!(lock(&core).t.week.done.is_none());
    assert!(lock(&core).reveal().is_none());
    click(&mut rt, "keep-working");
    assert!(lock(&core).dialog.is_none());
    assert!(lock(&core).t.week.done.is_none());
    assert!(shows(&rt, "/composer-frame"));
    assert!(!shows(&rt, "/seal-sweep"));
    crate::fixtures::scene_end(&core);
    rt.frame(0.016);
    click(&mut rt, "confirm-end");
    assert!(lock(&core).dialog.is_none());
    assert!(
        lock(&core).t.week.done.is_none(),
        "wait for pi's done entry"
    );
    assert!(!shows(&rt, "/seal-sweep"));
    {
        let mut core = lock(&core);
        core.t.week.done = Some(crate::transcript::Done { score: 4 });
        core.version += 1;
    }
    rt.frame(0.016);
    assert!(shows(&rt, "/seal-sweep"));
    assert!(shows(&rt, "/ended"));
    assert!(!shows(&rt, "/composer-frame"));
    assert!(lock(&core).reveal().is_some());
}

#[test]
fn end_confirmation_fits_the_smallest_window() {
    for dark in [false, true] {
        let mut rt = Runtime::new(
            Window::new(core(crate::fixtures::scene_end)),
            view,
            (720.0, 480.0),
            Env::new(dark, 1.0),
        );
        rt.poll = Some(poll);
        rt.frame(0.0);
        rt.frame(1.0);
        let elements = rt.elements();
        let rect = |key: &str| {
            elements
                .iter()
                .find(|e| e.key.as_deref() == Some(key))
                .unwrap()
                .rect
        };
        for key in ["dialog", "confirm-end", "keep-working"] {
            let [x, y, w, h] = rect(key);
            assert!(
                x >= 0.0 && y >= 0.0 && x + w <= 720.0 && y + h <= 480.0,
                "{key}: {x}, {y}, {w}, {h}"
            );
        }
        let yes = rect("confirm-end");
        let no = rect("keep-working");
        assert!(no[0] + no[2] <= yes[0]);
    }
}

#[test]
fn reduced_motion_ends_without_a_sweep() {
    let core = core(crate::fixtures::scene_chat);
    let mut rt = window(core.clone());
    rt.set_system_reduced_motion(true);
    rt.frame(0.016);
    {
        let mut core = lock(&core);
        core.t.week.done = Some(crate::transcript::Done { score: 4 });
        core.version += 1;
    }
    rt.frame(0.016);
    assert!(shows(&rt, "/ended"));
    assert!(!shows(&rt, "/seal-sweep"));
    assert!(lock(&core).reveal().is_some());
}

#[test]
fn opening_or_reloading_an_ended_chat_never_replays_the_sweep() {
    let core = core(crate::fixtures::scene_reveal);
    let mut rt = window(core.clone());
    assert!(!shows(&rt, "/seal-sweep"));
    {
        let mut core = lock(&core);
        core.t.week.done = None;
        core.version += 1;
    }
    rt.frame(0.016);
    {
        let mut core = lock(&core);
        core.t.week.done = Some(crate::transcript::Done { score: 4 });
        core.loads += 1;
        core.version += 1;
    }
    rt.frame(0.016);
    assert!(!shows(&rt, "/seal-sweep"));
}

#[test]
fn a_working_chat_shows_its_stage_until_it_ends() {
    for (scene, key) in [
        crate::fixtures::scene_dealing as fn(&Shared),
        crate::fixtures::scene_working,
        crate::fixtures::scene_fanning,
    ]
    .into_iter()
    .zip(["/stage-deal", "/tide", "/stage-fanout"])
    {
        let core = core(scene);
        let mut rt = window(core.clone());
        assert!(shows(&rt, key), "{key}");
        assert!(rt.wake_deadline().is_some(), "{key} keeps drawing");
        {
            let mut core = lock(&core);
            core.t.week.done = Some(crate::transcript::Done { score: 4 });
            core.version += 1;
        }
        // The tide goes out rather than vanish, so give it time to drain.
        for _ in 0..3 {
            rt.frame(1.0);
        }
        assert!(!shows(&rt, key), "{key} leaves once the run ends");
    }
}

#[test]
fn dot_art_replaces_the_generic_working_row() {
    for scene in [
        crate::fixtures::scene_working as fn(&Shared),
        crate::fixtures::scene_thinking,
    ] {
        let shared = core(scene);
        let rt = window(shared.clone());
        assert!(!shows(&rt, "/working"));
        assert!(parts::busy_line(&lock(&shared)).is_none());

        lock(&shared)
            .statuses
            .insert("compaction".into(), "Compacting…".into());
        let rt = window(shared);
        assert!(shows(&rt, "/working"), "specific statuses still show");
    }
}

#[test]
fn a_live_thought_writes_itself_over_the_tide() {
    let core = core(crate::fixtures::scene_thinking);
    let rt = window(core.clone());
    let last = lock(&core).t.items.len() - 1;
    assert!(shows(&rt, &format!("/item-{last}-think0")));
    assert!(shows(&rt, "/tide"), "the tide stays through the whole turn");
    assert!(rt.wake_deadline().is_some(), "the thought keeps drawing");
    assert!(
        rt.elements().iter().all(|e| !e
            .text
            .as_deref()
            .unwrap_or_default()
            .contains("README sections")),
        "the trace stays hidden until the reveal"
    );
}

#[test]
fn the_tide_comes_in_and_goes_out_with_the_turn() {
    let shared = core(crate::fixtures::scene_chat);
    let mut rt = window(shared.clone());
    rt.state.window_focused = true;
    assert!(!shows(&rt, "/tide"));
    let work = |on: bool| {
        let mut core = lock(&shared);
        core.streaming = on;
        core.version += 1;
    };
    work(true);
    rt.frame(0.1);
    rt.frame(0.5);
    let coming = rt.state.tide.level();
    assert!(
        shows(&rt, "/tide") && coming > 0.0 && coming < 1.0,
        "{coming}"
    );
    rt.frame(1.0);
    assert_eq!(rt.state.tide.level(), 1.0);
    work(false);
    rt.frame(1.0);
    assert!(shows(&rt, "/tide"), "it drains rather than pop out");
    assert!(rt.state.tide.level() < 1.0);
    rt.frame(1.5);
    rt.frame(0.1);
    assert!(!shows(&rt, "/tide"));
}

#[test]
fn the_tide_leaves_room_under_the_tail_and_stays_out_of_a_dealing_or_fanout() {
    let shared = core(crate::fixtures::scene_working);
    let rt = window(shared);
    let rect = |key: &str| {
        rt.elements()
            .into_iter()
            .find(|e| e.key.as_deref() == Some(key))
            .unwrap()
            .rect
    };
    let [_, gap_y, _, gap_h] = rect("stage-tide-gap");
    let [_, tide_y, ..] = rect("tide");
    assert!(gap_y + gap_h > tide_y, "the gap sits over the crest");
    for scene in [
        crate::fixtures::scene_dealing as fn(&Shared),
        crate::fixtures::scene_fanning,
    ] {
        let rt = window(core(scene));
        assert!(!shows(&rt, "/tide"));
    }
}

#[test]
fn a_sealed_thought_never_shows_its_text_or_its_length() {
    let height = |body: &str| {
        let core = core(crate::fixtures::scene_chat);
        if let Some(Item::Assistant { blocks, .. }) = lock(&core).t.items.get_mut(1) {
            blocks[0] = Some(crate::transcript::Block::Thinking(body.into()));
        }
        let rt = window(core);
        assert!(
            rt.elements()
                .iter()
                .all(|e| !e.text.as_deref().unwrap_or_default().contains("secret")),
        );
        rt.elements()
            .into_iter()
            .find(|e| e.path.ends_with("/item-1-think0"))
            .map(|e| e.rect[3])
            .unwrap()
    };
    assert_eq!(
        height("secret"),
        height(&"a long secret trace ".repeat(400))
    );
}

#[test]
fn a_fanout_lays_its_lanes_where_the_answers_will_land() {
    let mut rt = window(core(crate::fixtures::scene_fanning));
    for label in ["A", "B", "C"] {
        assert!(
            shows(&rt, &format!("/stage-fanout/lane-{label}")),
            "{label}"
        );
    }
    let lane = |rt: &Runtime<Window>, label: &str| {
        rt.elements()
            .into_iter()
            .find(|e| e.path.ends_with(&format!("/lane-{label}")))
            .map(|e| e.rect)
            .unwrap()
    };
    let (a, c) = (lane(&rt, "A"), lane(&rt, "C"));
    assert!((a[2] - c[2]).abs() < 0.5, "lanes share the row evenly");
    assert!(
        c[0] + c[2] - a[0] > chat::COLUMN - 1.0,
        "the lanes span the column"
    );
    rt.frame(1.0);
}

#[test]
fn a_clicked_image_opens_whole_and_esc_closes_only_it() {
    let core = core(crate::fixtures::scene_attach);
    lock(&core).streaming = true;
    let mut rt = window(core.clone());
    click(&mut rt, "shot-0");
    assert_eq!(rt.state.viewing, Some(crate::fixtures::screenshot()));
    assert!(shows(&rt, "/viewer-bar"));
    rt.key(Key::Escape.into());
    rt.frame(0.016);
    assert!(rt.state.viewing.is_none());
    assert!(
        lock(&core).sent_before_start().is_empty(),
        "Esc closed the image without stopping the agent"
    );
}

#[test]
fn a_clicked_link_opens_and_one_that_cant_says_why() {
    let dir = std::env::temp_dir().join(format!("whodunit-links-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("shot.png"),
        &crate::fixtures::screenshot().bytes[..],
    )
    .unwrap();
    let by_url = format!("file://{}/shot.png", dir.display());
    let core = core(|_| {});
    {
        let mut core = lock(&core);
        core.cwd = dir.clone();
        core.t.items = vec![Item::Assistant {
            blocks: vec![Some(crate::transcript::Block::Text(format!(
                "See [the docs](https://example.com/docs), [gone](missing.rs), \
                 [the shot](shot.png) or [the same](<{by_url}>)."
            )))],
            streaming: false,
            error: None,
        }];
    }
    let mut rt = window(core.clone());
    click(&mut rt, "https://example.com/docs");
    assert_eq!(rt.take_url_requests(), ["https://example.com/docs"]);
    assert!(lock(&core).toasts.is_empty());

    click(&mut rt, "missing.rs");
    assert!(rt.take_url_requests().is_empty());
    let gone = format!("No such file: {}", dir.join("missing.rs").display());
    assert_eq!(lock(&core).toasts[0].text, gone);

    // A relative path is the chat's folder's; a file URL is the same file.
    for link in ["shot.png", by_url.as_str()] {
        click(&mut rt, link);
        assert_eq!(
            rt.state.viewing,
            Some(crate::fixtures::screenshot()),
            "{link}"
        );
        rt.key(Key::Escape.into());
        rt.frame(1.0);
    }
    assert!(rt.take_url_requests().is_empty());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_clicked_chat_is_the_next_frame() {
    let one = PathBuf::from("/fixture/one.jsonl");
    let two = PathBuf::from("/fixture/two.jsonl");
    let three = saved_chat("next-frame");
    let core = core(|_| {});
    {
        let mut core = lock(&core);
        for (path, scene) in [
            (&one, crate::fixtures::scene_chat as fn(&Shared)),
            (&two, crate::fixtures::scene_run),
        ] {
            let t = transcript_of(scene);
            core.sessions.push(summary(path, &t));
            core.open_task("/fixture".into(), Some(path), t);
        }
        let saved = crate::sessions::transcript(&three).unwrap();
        core.sessions.push(summary(&three, &saved));
    }
    let mut rt = window(core.clone());
    click(&mut rt, "Ended/section-header");
    rt.frame(1.0);
    // Two live chats, then one only on disk.
    for (path, chat, started) in [
        (&one, "Rename SessionStore to Vault", 0),
        (&two, "Retry uploads with backoff", 0),
        (&three, "Saved task", 1),
    ] {
        click(&mut rt, &format!("session-{}", path.display()));
        assert_eq!(title(&rt).as_deref(), Some(chat));
        if started > 0 {
            assert!(rt.wake_deadline().is_some(), "poll starts its process");
        }
        assert_eq!(unstarted(&core), started);
        assert_eq!(in_motion(&mut rt), Vec::<String>::new(), "{chat}");
    }
    assert!(shows(&rt, "/ended"), "the saved chat shows as ended");
    assert!(shows(
        &rt,
        &format!("/Ended/ended-list/session-{}", three.display())
    ));
    let _ = std::fs::remove_file(three);
}

fn cycle_session(rt: &mut Runtime<Window>, key: char) {
    rt.key(KeyEvent {
        key: Key::Command(key),
        mods: Mods {
            cmd: true,
            shift: true,
            ..Mods::default()
        },
    });
    rt.frame(0.016);
}

#[test]
fn session_cycling_follows_sidebar_order_across_groups_and_wraps() {
    let one = PathBuf::from("/fixture/cycle-one.jsonl");
    let two = saved_chat("cycle");
    let three = PathBuf::from("/fixture/cycle-three.jsonl");
    let four = PathBuf::from("/fixture/cycle-four.jsonl");
    let five = PathBuf::from("/fixture/cycle-five.jsonl");
    let shared = core(|_| {});
    {
        let mut core = lock(&shared);
        let idle = transcript_of(crate::fixtures::scene_chat);
        let ended = crate::sessions::transcript(&two).unwrap();
        core.sessions.push(summary(&two, &ended));
        core.sessions.push(summary(&one, &idle));
        core.open_task(
            "/fixture".into(),
            Some(&one),
            transcript_of(crate::fixtures::scene_chat),
        );
        let mut last = ended;
        last.name = Some("Another ended task".into());
        core.sessions.push(summary(&five, &last));
        core.open_task("/fixture".into(), Some(&five), last);
        core.sessions.push(summary(&four, &idle));
        core.open_task("/fixture".into(), Some(&four), idle);
        let working = transcript_of(crate::fixtures::scene_run);
        core.sessions.push(summary(&three, &working));
        core.open_task("/fixture".into(), Some(&three), working);
        core.streaming = true;
        // Storage order interleaves groups; sidebar order is three, one, four, two, five.
        assert_eq!(
            core.sessions
                .iter()
                .map(|s| core.session_state(s))
                .collect::<Vec<_>>(),
            [
                SessionState::Ended,
                SessionState::Idle,
                SessionState::Ended,
                SessionState::Idle,
                SessionState::Working,
            ]
        );
        core.request_leave(Leave::Open(one.clone()));
    }
    let mut rt = window(shared.clone());
    rt.poll = None;
    rt.update(|s, _| s.draft.composer.replace_text("Unsent first draft"));
    assert!(rt.state.ended_collapsed);

    for (key, path, chat, started) in [
        (']', &four, "Rename SessionStore to Vault", 0),
        (']', &two, "Saved task", 1),
        (']', &five, "Another ended task", 0),
        (']', &three, "Retry uploads with backoff", 0),
        (']', &one, "Rename SessionStore to Vault", 0),
        ('[', &three, "Retry uploads with backoff", 0),
        ('[', &five, "Another ended task", 0),
        ('[', &two, "Saved task", 0),
        ('[', &four, "Rename SessionStore to Vault", 0),
        ('[', &one, "Rename SessionStore to Vault", 0),
    ] {
        cycle_session(&mut rt, key);
        assert_eq!(lock(&shared).path().as_ref(), Some(path));
        assert_eq!(title(&rt).as_deref(), Some(chat));
        assert_eq!(unstarted(&shared), started);
        assert_eq!(in_motion(&mut rt), Vec::<String>::new());
        if path == &two || path == &five {
            assert!(!rt.state.ended_collapsed);
            assert!(shows(
                &rt,
                &format!("/Ended/ended-list/session-{}", path.display())
            ));
        }
    }
    assert_eq!(rt.state.draft.composer.text, "Unsent first draft");

    // A new, unlisted chat starts at the first or last sidebar row.
    for (key, expected) in [(']', &three), ('{', &five)] {
        rt.update(|s, tx| s.leave(Leave::New, tx));
        assert_eq!(unstarted(&shared), 1);
        cycle_session(&mut rt, key);
        assert_eq!(lock(&shared).path().as_ref(), Some(expected));
    }

    // A state change updates cycling to match the newly grouped sidebar.
    {
        let mut core = lock(&shared);
        core.request_leave(Leave::Open(three.clone()));
        core.streaming = false;
        core.request_leave(Leave::Open(one.clone()));
    }
    cycle_session(&mut rt, '}');
    assert_eq!(lock(&shared).path().as_ref(), Some(&four));
    cycle_session(&mut rt, '}');
    assert_eq!(lock(&shared).path().as_ref(), Some(&three));
    cycle_session(&mut rt, '}');
    assert_eq!(lock(&shared).path().as_ref(), Some(&two));
    std::fs::remove_file(two).unwrap();
}

#[test]
fn session_cycling_scrolls_both_lists_to_keep_the_selected_row_visible() {
    fn assert_visible(rt: &Runtime<Window>, path: &Path, viewport: &str) {
        let elements = rt.elements();
        let row_key = format!("session-{}", path.display());
        let row = elements
            .iter()
            .find(|e| e.key.as_deref() == Some(&row_key))
            .unwrap();
        let list = elements
            .iter()
            .find(|e| e.key.as_deref() == Some(viewport))
            .unwrap();
        assert!(
            row.rect[1] >= list.rect[1] - 0.5
                && row.rect[1] + row.rect[3] <= list.rect[1] + list.rect[3] + 0.5,
            "{row_key}: row {:?} outside {viewport} {:?}",
            row.rect,
            list.rect
        );
    }

    let idle: Vec<_> = (0..12)
        .map(|i| PathBuf::from(format!("/fixture/cycle-scroll-idle-{i}.jsonl")))
        .collect();
    let ended: Vec<_> = (0..12)
        .map(|i| PathBuf::from(format!("/fixture/cycle-scroll-ended-{i}.jsonl")))
        .collect();
    let shared = core(|_| {});
    {
        let mut core = lock(&shared);
        for i in 0..12 {
            // Interleaved storage order must still cycle in sidebar order.
            for (path, done) in [(&ended[i], true), (&idle[i], false)] {
                let mut t = transcript_of(crate::fixtures::scene_chat);
                t.name = Some(format!("{} {i}", if done { "Ended" } else { "Idle" }));
                t.week.done = done.then_some(crate::transcript::Done { score: 4 });
                core.sessions.push(summary(path, &t));
                core.open_task("/fixture".into(), Some(path), t);
            }
        }
        core.request_leave(Leave::Open(idle[0].clone()));
    }
    let mut rt = window(shared.clone());
    rt.poll = None;
    rt.update(|s, _| s.draft.composer.replace_text("Keep my place"));
    for path in idle
        .iter()
        .skip(1)
        .chain(ended.iter())
        .chain(idle.iter().take(1))
    {
        cycle_session(&mut rt, ']');
        assert_eq!(lock(&shared).path().as_ref(), Some(path));
        let viewport = if ended.contains(path) {
            "ended-list"
        } else {
            "sidebar-scroll"
        };
        assert_visible(&rt, path, viewport);
    }
    // Enter collapsed history from its last row, then scroll back through both lists.
    rt.update(|s, _| s.ended_collapsed = true);
    rt.frame(1.0);
    for path in ended.iter().rev().chain(idle.iter().rev()) {
        cycle_session(&mut rt, '[');
        assert_eq!(lock(&shared).path().as_ref(), Some(path));
        let viewport = if ended.contains(path) {
            "ended-list"
        } else {
            "sidebar-scroll"
        };
        assert_visible(&rt, path, viewport);
    }
    assert_eq!(rt.state.draft.composer.text, "Keep my place");
    assert!(rt.state.draft.composer.focused);
    assert_eq!(unstarted(&shared), 0);
}

#[test]
fn cycling_out_restores_collapsed_history_unless_manually_toggled() {
    for (collapsed, manual_toggle) in [(true, false), (false, false), (true, true)] {
        let shared = core(|_| {});
        let paths: Vec<_> = (0..3)
            .map(|i| PathBuf::from(format!("/fixture/cycle-collapse-{i}.jsonl")))
            .collect();
        {
            let mut core = lock(&shared);
            for (i, path) in paths.iter().enumerate() {
                let mut t = transcript_of(crate::fixtures::scene_chat);
                t.week.done = (i > 0).then_some(crate::transcript::Done { score: 4 });
                core.sessions.push(summary(path, &t));
                core.open_task("/fixture".into(), Some(path), t);
            }
            core.request_leave(Leave::Open(paths[0].clone()));
        }
        let mut rt = window(shared.clone());
        rt.poll = None;
        if !collapsed {
            click(&mut rt, "Ended/section-header");
        }
        for path in &paths[1..] {
            cycle_session(&mut rt, ']');
            assert_eq!(lock(&shared).path().as_ref(), Some(path));
            assert!(!rt.state.ended_collapsed);
        }
        if manual_toggle {
            click(&mut rt, "Ended/section-header");
            click(&mut rt, "Ended/section-header");
        }
        cycle_session(&mut rt, ']');
        assert_eq!(lock(&shared).path().as_ref(), Some(&paths[0]));
        assert_eq!(rt.state.ended_collapsed, collapsed && !manual_toggle);

        // Backward entry and exit restore the same preference too.
        for path in paths[1..].iter().rev() {
            cycle_session(&mut rt, '[');
            assert_eq!(lock(&shared).path().as_ref(), Some(path));
            assert!(!rt.state.ended_collapsed);
        }
        cycle_session(&mut rt, '[');
        assert_eq!(lock(&shared).path().as_ref(), Some(&paths[0]));
        assert_eq!(rt.state.ended_collapsed, collapsed && !manual_toggle);
    }
}

#[test]
fn session_cycling_handles_empty_and_single_session_lists() {
    for scene in [
        crate::fixtures::scene_empty as fn(&Shared),
        crate::fixtures::scene_chat,
    ] {
        let shared = core(scene);
        lock(&shared).sessions.truncate(1);
        let mut rt = window(shared.clone());
        rt.poll = None;
        let original = lock(&shared).task_id();
        for key in ['[', ']', '{', '}'] {
            cycle_session(&mut rt, key);
            assert_eq!(lock(&shared).task_id(), original);
            assert_eq!(unstarted(&shared), 0);
        }
    }
}

#[test]
fn ended_sessions_start_collapsed_and_toggle() {
    let core = core(crate::fixtures::scene_tasks);
    let ended: Vec<_> = {
        let core = lock(&core);
        core.sessions
            .iter()
            .filter(|summary| core.session_state(summary) == SessionState::Ended)
            .map(|summary| format!("/Ended/ended-list/session-{}", summary.path.display()))
            .collect()
    };
    assert!(!ended.is_empty());
    let mut rt = window(core);
    assert!(shows(&rt, "/Ended/section-header"));
    assert!(ended.iter().all(|path| !shows(&rt, path)));
    assert!(shows(&rt, "/Working/section-header"));
    assert!(shows(&rt, "/Idle/section-header"));

    click(&mut rt, "Ended/section-header");
    assert!(ended.iter().all(|path| shows(&rt, path)));
    click(&mut rt, "Ended/section-header");
    rt.frame(1.0);
    assert!(ended.iter().all(|path| !shows(&rt, path)));
    assert!(shows(&rt, "/Ended/section-header"));
}

#[test]
fn an_ended_row_shows_its_role_and_agent_icons_named_on_hover() {
    let core = core(crate::fixtures::scene_tasks);
    let (path, role, agent) = {
        let core = lock(&core);
        let ended = core
            .sessions
            .iter()
            .find(|summary| core.session_state(summary) == SessionState::Ended)
            .unwrap();
        let deal = ended.revealed().unwrap();
        (
            ended.path.clone(),
            deal.role.clone(),
            config::agent_name(&deal.agent),
        )
    };
    let mut rt = window(core);
    click(&mut rt, "Ended/section-header");
    rt.frame(1.0);
    let find = |rt: &Runtime<Window>, tail: &str| {
        rt.elements().into_iter().find(|e| e.path.ends_with(tail))
    };
    let file = path.display();
    let lines = find(&rt, &format!("/session-{file}/lines")).unwrap();
    let role_icon = find(&rt, &format!("/role-{file}-icon")).unwrap();
    let agent_icon = find(&rt, &format!("/agent-{file}-icon")).unwrap();
    // Both centre on the row's two lines, the role's ahead of them and the agent's after.
    let centre = |r: [f32; 4]| r[1] + r[3] / 2.0;
    assert!((centre(role_icon.rect) - centre(lines.rect)).abs() < 0.5);
    assert!((centre(agent_icon.rect) - centre(lines.rect)).abs() < 0.5);
    assert!(role_icon.rect[0] + role_icon.rect[2] <= lines.rect[0] + 22.0);
    assert!(agent_icon.rect[0] >= lines.rect[0] + lines.rect[2]);

    for (icon, tip, name) in [
        (role_icon, format!("/role-{file}-tooltip"), &role),
        (agent_icon, format!("/agent-{file}-tooltip"), &agent),
    ] {
        let r = icon.rect;
        rt.pointer_move(r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);
        rt.frame(0.016);
        assert!(find(&rt, &tip).is_none(), "not before the delay");
        rt.frame(sidebar::SPRITE_TIP_DELAY.as_secs_f64() + 0.1);
        rt.frame(1.0);
        let shown = find(&rt, &tip).expect("the tooltip opens");
        assert_eq!(shown.label.as_deref(), Some(name.as_str()));
        rt.pointer_move(600.0, 400.0);
        rt.frame(0.016);
        rt.frame(1.0);
        assert!(find(&rt, &tip).is_none(), "leaving closes it");
    }
}

#[test]
fn ended_section_is_anchored_to_the_bottom_and_history_is_bounded() {
    let core = core(crate::fixtures::scene_tasks);
    {
        let mut core = lock(&core);
        let ended = core
            .sessions
            .iter()
            .find(|summary| core.session_state(summary) == SessionState::Ended)
            .unwrap()
            .clone();
        for n in 0..30 {
            let mut summary = ended.clone();
            summary.path = PathBuf::from(format!("/fixture/ended-{n}.jsonl"));
            core.sessions.push(summary);
        }
    }
    let mut rt = window(core);
    for expanded in [false, true, false] {
        if expanded != !rt.state.ended_collapsed {
            click(&mut rt, "Ended/section-header");
            rt.frame(1.0);
        }
        let elements = rt.elements();
        let sidebar = elements
            .iter()
            .find(|e| e.key.as_deref() == Some("sidebar"))
            .unwrap();
        let footer = elements
            .iter()
            .find(|e| e.key.as_deref() == Some("sidebar-footer"))
            .unwrap();
        assert!((footer.rect[1] + footer.rect[3] - sidebar.rect[1] - sidebar.rect[3]).abs() < 1.0);
        if expanded {
            let list = elements
                .iter()
                .find(|e| e.key.as_deref() == Some("ended-list"))
                .unwrap();
            assert!(list.rect[3] <= 240.0);
            assert!(list.rect[3] > 200.0);
        } else {
            let header = elements
                .iter()
                .find(|e| e.path.ends_with("/Ended/section-header"))
                .unwrap();
            assert!(sidebar.rect[1] + sidebar.rect[3] - header.rect[1] - header.rect[3] < 12.0);
        }
    }
}

/// The new task is the frame right after `leave` asks for it.
fn new_task_is_the_next_frame(leave: impl Fn(&mut Runtime<Window>)) {
    let core = core(crate::fixtures::scene_chat);
    let mut rt = window(core.clone());
    leave(&mut rt);
    rt.frame(0.016);
    assert_eq!(title(&rt).as_deref(), Some("New task"));
    assert!(shows(&rt, "/empty-intro"));
    assert!(rt.wake_deadline().is_some(), "poll starts its process");
    assert_eq!(unstarted(&core), 1);
    assert_eq!(in_motion(&mut rt), Vec::<String>::new());
}

#[test]
fn a_new_task_is_the_next_frame() {
    new_task_is_the_next_frame(|rt| {
        rt.key(KeyEvent {
            key: Key::Command('n'),
            mods: Mods {
                cmd: true,
                ..Mods::default()
            },
        })
    });
    new_task_is_the_next_frame(|rt| {
        rt.update(|s, _| s.draft.composer.replace_text("/new"));
        rt.frame(0.016);
        rt.key(Key::Enter.into());
    });
}

#[test]
fn a_transcript_reloaded_from_pi_lands_whole() {
    let core = core(crate::fixtures::scene_chat);
    let mut rt = window(core.clone());
    lock(&core).open_task("/fixture".into(), None, Transcript::default());
    rt.frame(0.016);
    assert_eq!(in_motion(&mut rt), Vec::<String>::new());
    {
        let mut core = lock(&core);
        core.t = transcript_of(crate::fixtures::scene_long);
        core.loads += 1;
        core.version += 1;
    }
    rt.frame(0.016);
    assert_eq!(in_motion(&mut rt), Vec::<String>::new());
    assert!(shows(&rt, "/item-119-md2"), "it opens at the latest answer");
}

#[test]
fn pages_switch_in_one_frame() {
    let core = core(crate::fixtures::scene_chat);
    let mut rt = window(core);
    rt.update(|s, tx| {
        s.board = Some(crate::fixtures::board());
        s.show(Page::Scoreboard, tx);
    });
    rt.frame(0.016);
    assert_eq!(in_motion(&mut rt), Vec::<String>::new());
    assert!(shows(&rt, "/scoreboard-page"));
    rt.key(KeyEvent {
        key: Key::Command('1'),
        mods: Mods {
            cmd: true,
            ..Mods::default()
        },
    });
    rt.frame(0.016);
    assert_eq!(in_motion(&mut rt), Vec::<String>::new());
    assert!(shows(&rt, "/composer-frame"));
}

#[test]
fn command_i_focuses_the_composer_from_any_page() {
    let core = core(crate::fixtures::scene_chat);
    let mut rt = window(core);
    rt.update(|s, tx| {
        s.draft.composer.focused = false;
        s.board = Some(crate::fixtures::board());
        s.show(Page::Scoreboard, tx);
    });
    rt.frame(0.016);
    rt.key(KeyEvent {
        key: Key::Command('i'),
        mods: Mods {
            cmd: true,
            ..Mods::default()
        },
    });
    rt.frame(0.016);
    assert!(shows(&rt, "/composer-frame"));
    assert!(rt.state.draft.composer.focused);
}

#[test]
fn a_settled_turn_develops_its_answer_once() {
    let core = core(crate::fixtures::scene_chat);
    let mut rt = window(core.clone());
    assert!(!shows(&rt, "/develop"));
    let settle = |core: &Shared| {
        let mut core = lock(core);
        core.turns += 1;
        core.version += 1;
    };
    settle(&core);
    rt.frame(0.016);
    assert!(rt.state.develop.at("item-1").is_some());
    assert!(shows(&rt, "/develop"));

    rt.update(|s, _| s.develop.clear());
    lock(&core).t.items.push(Item::Bash {
        command: "cargo test".into(),
        output: String::new(),
        exit_code: Some(0),
        running: false,
    });
    settle(&core);
    rt.frame(0.016);
    assert!(
        rt.state.develop.progress().is_none(),
        "a turn that doesn't end on an answer gets no pass"
    );
    assert!(!shows(&rt, "/develop"));
}
