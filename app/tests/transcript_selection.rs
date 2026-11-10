//! The chat's text selects as one flow: a drag runs from the prompt through the agent's tool
//! calls into its answer, and Cmd+A takes the whole chat.

use std::sync::{Arc, Mutex};

use rune::{Env, Key, KeyEvent, Mods, Runtime};
use whodunit::config::AppConfig;
use whodunit::core::Core;
use whodunit::ui::window::{self, Window};

fn window() -> Runtime<Window> {
    let core = Arc::new(Mutex::new(Core::new(
        AppConfig::default(),
        "/fixture".into(),
    )));
    whodunit::fixtures::scene_chat(&core);
    let mut rt = Runtime::new(
        Window::new(core),
        window::view,
        (1180.0, 780.0),
        Env::new(true, 1.0),
    );
    rt.poll = Some(window::poll);
    rt.frame(0.0);
    rt.frame(1.0);
    rt
}

/// The top-left corner, just inside, of the text that starts with `start`.
fn text_at(rt: &Runtime<Window>, start: &str) -> (f32, f32) {
    let e = rt
        .elements()
        .into_iter()
        .find(|e| e.text.as_deref().is_some_and(|t| t.starts_with(start)))
        .unwrap_or_else(|| panic!("no text starting {start:?}"));
    (e.rect[0] + 1.0, e.rect[1] + e.rect[3] / 2.0)
}

fn drag(rt: &mut Runtime<Window>, (x0, y0): (f32, f32), (x1, y1): (f32, f32)) {
    rt.pointer_move(x0, y0);
    rt.pointer_down(x0, y0);
    rt.frame(0.016);
    for step in 1..=12 {
        let f = step as f32 / 12.0;
        rt.pointer_move(x0 + (x1 - x0) * f, y0 + (y1 - y0) * f);
        rt.frame(0.016);
    }
    rt.pointer_up(x1, y1);
    rt.frame(0.016);
}

#[test]
fn a_drag_selects_from_the_prompt_into_the_answer() {
    let mut rt = window();
    let from = text_at(&rt, "Rename `SessionStore`");
    let (x, y) = text_at(&rt, "Found it in 4 files");
    drag(&mut rt, from, (x + 60.0, y));
    let selected = rt.selected_text().expect("a selection");
    assert!(
        selected.starts_with("Rename `SessionStore` to `Vault`"),
        "{selected:?}"
    );
    for part in ["rg -l SessionStore src", "tests/store.rs", "\nFound it"] {
        assert!(selected.contains(part), "{part:?} in {selected:?}");
    }
}

#[test]
fn command_a_takes_the_whole_chat() {
    let mut rt = window();
    let (x, y) = text_at(&rt, "Found it in 4 files");
    rt.pointer_move(x + 20.0, y);
    rt.pointer_down(x + 20.0, y);
    rt.pointer_up(x + 20.0, y);
    rt.frame(0.016);
    rt.key(KeyEvent {
        key: Key::Command('a'),
        mods: Mods {
            cmd: true,
            ..Mods::default()
        },
    });
    rt.frame(0.016);
    let selected = rt.selected_text().expect("a selection");
    assert!(
        selected.starts_with("Rename `SessionStore`"),
        "{selected:?}"
    );
    assert!(selected.contains("pub struct Vault"), "{selected:?}");
    assert!(selected.trim_end().ends_with("38 passed)."), "{selected:?}");
}
