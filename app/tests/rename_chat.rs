//! Titles edit in place, without a dialog, a prompt or an end-run action.

use std::sync::{Arc, Mutex};

use rune::{Env, Key, KeyEvent, Mods, Runtime};
use whodunit::config::AppConfig;
use whodunit::core::{Core, Shared, lock};
use whodunit::fixtures;
use whodunit::ui::window::{self, Window};

fn window(scene: fn(&Shared)) -> (Shared, Runtime<Window>) {
    let shared = Arc::new(Mutex::new(Core::new(
        AppConfig::default(),
        "/fixture".into(),
    )));
    scene(&shared);
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
    (shared, rt)
}

fn click(rt: &mut Runtime<Window>, key: &str) {
    let (x, y) = rt.find(key).unwrap();
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(0.016);
    rt.frame(0.016);
}

fn shown(rt: &Runtime<Window>, key: &str) -> bool {
    rt.elements()
        .iter()
        .any(|e| !e.exiting && e.key.as_deref() == Some(key))
}

fn editing(rt: &Runtime<Window>) -> bool {
    shown(rt, "rename-title") || shown(rt, "rename")
}

fn replace_title(rt: &mut Runtime<Window>, title: &str) {
    rt.key(KeyEvent {
        key: Key::Command('a'),
        mods: Mods {
            cmd: true,
            ..Mods::default()
        },
    });
    rt.key(Key::Text(title.into()).into());
    rt.frame(0.016);
}

const SIDEBAR: &str = "session-/fixture/a.jsonl";

#[test]
fn titles_edit_in_place_and_enter_saves_without_changing_the_draft() {
    for target in ["title", SIDEBAR] {
        let (shared, mut rt) = window(fixtures::scene_chat);
        rt.update(|s, _| s.draft.composer.replace_text("An unsent follow-up"));
        click(&mut rt, target);
        assert!(editing(&rt));
        assert!(!shown(&rt, "rename-dialog"));
        assert!(!shown(&rt, "rename-chat"));
        assert!(rt.elements().iter().any(|e| e.role == "textbox"
            && e.focused
            && e.text.as_deref() == Some("Rename SessionStore to Vault")));
        replace_title(&mut rt, "A title I chose");
        rt.key(Key::Enter.into());
        rt.frame(0.016);
        assert!(!editing(&rt));
        assert_eq!(rt.state.draft.composer.text, "An unsent follow-up");
        assert!(lock(&shared).reveal().is_none());
    }
}

#[test]
fn escape_discards_without_stopping_work() {
    for target in ["title", SIDEBAR] {
        let (shared, mut rt) = window(fixtures::scene_working);
        rt.update(|s, _| s.draft.composer.replace_text("Keep my draft"));
        let title = lock(&shared).t.title();
        click(&mut rt, target);
        replace_title(&mut rt, "Discard this");
        rt.key(Key::Escape.into());
        rt.frame(0.016);
        assert!(!editing(&rt));
        assert_eq!(lock(&shared).t.title(), title);
        assert!(lock(&shared).streaming);
        assert_eq!(rt.state.draft.composer.text, "Keep my draft");
    }
}

#[test]
fn clicking_the_composer_or_blank_space_saves_and_keeps_the_clicked_focus() {
    for target in ["title", SIDEBAR] {
        for outside in ["input", "blank"] {
            let (_, mut rt) = window(fixtures::scene_chat);
            click(&mut rt, target);
            replace_title(&mut rt, "A title I chose");
            if outside == "input" {
                click(&mut rt, "input");
                assert!(rt.state.draft.composer.focused);
            } else {
                rt.pointer_down(1100.0, 400.0);
                rt.pointer_up(1100.0, 400.0);
                rt.frame(0.016);
                rt.frame(0.016);
            }
            assert!(!editing(&rt), "{target}, {outside}");
        }
    }
}

#[test]
fn blank_titles_revert_instead_of_leaving_an_empty_editor() {
    for target in ["title", SIDEBAR] {
        let (shared, mut rt) = window(fixtures::scene_chat);
        let title = lock(&shared).t.title();
        click(&mut rt, target);
        replace_title(&mut rt, "   ");
        rt.key(Key::Enter.into());
        rt.frame(0.016);
        assert!(!editing(&rt));
        assert_eq!(lock(&shared).t.title(), title);
    }
}

#[test]
fn ended_chats_stay_sealed_after_inline_renaming() {
    for target in ["title", SIDEBAR] {
        let (shared, mut rt) = window(fixtures::scene_reveal);
        if target == SIDEBAR {
            click(&mut rt, "Ended/section-header");
            rt.frame(1.0);
        }
        let done = lock(&shared).t.week.done.as_ref().unwrap().score;
        click(&mut rt, target);
        assert!(editing(&rt), "{target}");
        assert!(!rt.state.draft.composer.focused, "before typing: {target}");
        replace_title(&mut rt, "Finished cleanup");
        rt.key(Key::Enter.into());
        rt.frame(0.016);
        assert!(!editing(&rt));
        assert_eq!(lock(&shared).t.week.done.as_ref().unwrap().score, done);
        assert!(!rt.state.draft.composer.focused, "{target}");
    }
}

#[test]
fn switching_between_title_locations_carries_the_saved_title_into_the_new_editor() {
    let (_, mut rt) = window(fixtures::scene_chat);
    click(&mut rt, "title");
    replace_title(&mut rt, "Keep this title");
    click(&mut rt, SIDEBAR);
    assert!(shown(&rt, "rename"));
    assert!(
        rt.elements().iter().any(|e| e.role == "textbox"
            && e.focused
            && e.text.as_deref() == Some("Keep this title"))
    );
    rt.key(Key::Escape.into());
    rt.frame(0.016);
    assert!(!editing(&rt));
}
