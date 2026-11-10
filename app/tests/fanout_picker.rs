use std::sync::{Arc, Mutex};

use rune::{Env, Key, Runtime};
use whodunit::config::AppConfig;
use whodunit::core::{Core, DialogKind, Shared, lock};
use whodunit::fixtures;
use whodunit::transcript::Item;
use whodunit::ui::window::{self, Window};

fn shared(scene: fn(&Shared)) -> Shared {
    let shared = Arc::new(Mutex::new(Core::new(
        AppConfig::default(),
        "/fixture".into(),
    )));
    scene(&shared);
    shared
}

fn window(shared: Shared, size: (f32, f32), dark: bool) -> Runtime<Window> {
    let mut rt = Runtime::new(Window::new(shared), window::view, size, Env::new(dark, 1.0));
    rt.poll = Some(window::poll);
    rt.on_key = Some(window::on_key);
    rt.frame(0.0);
    rt.frame(1.0);
    rt
}

fn click(rt: &mut Runtime<Window>, key: &str) {
    let (x, y) = rt.find(key).unwrap();
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(0.016);
}

fn shown(rt: &Runtime<Window>) -> Vec<String> {
    rt.elements()
        .into_iter()
        .filter_map(|e| e.key)
        .filter(|key| matches!(key.as_str(), "fan-A" | "fan-B" | "fan-C"))
        .collect()
}

#[test]
fn browsing_shows_one_full_width_answer_without_picking_it() {
    let shared = shared(fixtures::scene_fanout);
    let mut rt = window(shared.clone(), (1180.0, 780.0), true);
    assert_eq!(shown(&rt), ["fan-A"]);
    let a = rt
        .elements()
        .into_iter()
        .find(|e| e.key.as_deref() == Some("fan-A"))
        .unwrap();
    assert!(
        a.rect[2] > 700.0,
        "answer uses the reading column: {:?}",
        a.rect
    );
    click(&mut rt, "view-B");
    assert_eq!(shown(&rt), ["fan-B"]);
    assert_eq!(lock(&shared).dialog.as_ref().unwrap().highlight, Some(1));
    for (key, expected) in [
        (Key::Text("3".into()), "fan-C"),
        (Key::Right, "fan-A"),
        (Key::Left, "fan-C"),
        (Key::Text("b".into()), "fan-B"),
        (Key::Text("9".into()), "fan-B"),
    ] {
        rt.key(key.into());
        rt.frame(0.016);
        assert_eq!(shown(&rt), [expected]);
        assert!(lock(&shared).dialog.is_some());
        assert!(lock(&shared).reveal().is_none());
    }
    click(&mut rt, "pick-answer");
    assert_eq!(
        shown(&rt),
        ["fan-B"],
        "keep the chosen answer while pi records the pick"
    );
    assert!(lock(&shared).dialog.is_none());
    assert!(lock(&shared).t.week.done.is_none());
}

#[test]
fn enter_picks_the_visible_default_and_escape_only_cancels() {
    for key in [Key::Enter, Key::Escape] {
        let shared = shared(fixtures::scene_fanout);
        lock(&shared).dialog.as_mut().unwrap().highlight = None;
        let mut rt = window(shared.clone(), (1180.0, 780.0), false);
        assert_eq!(shown(&rt), ["fan-A"]);
        rt.key(key.into());
        rt.frame(0.016);
        assert!(lock(&shared).dialog.is_none());
        assert!(lock(&shared).reveal().is_none());
        assert!(!lock(&shared).streaming);
    }
}

#[test]
fn long_answers_start_at_the_top_and_keep_navigation_in_view() {
    for dark in [false, true] {
        for size in [(1180.0, 780.0), (720.0, 480.0)] {
            let shared = shared(fixtures::scene_fanout_long);
            let mut rt = window(shared, size, dark);
            let rect = |rt: &Runtime<Window>, key: &str| {
                rt.elements()
                    .into_iter()
                    .find(|e| e.key.as_deref() == Some(key))
                    .unwrap()
                    .rect
            };
            assert_eq!(shown(&rt), ["fan-B"]);
            let before = rect(&rt, "pick-answer");
            for key in ["view-A", "view-B", "view-C", "pick-answer"] {
                let [x, y, w, h] = rect(&rt, key);
                assert!(
                    x >= 0.0 && y >= 0.0 && x + w <= size.0 && y + h <= size.1,
                    "{key} outside {size:?}: {x}, {y}, {w}, {h}"
                );
            }
            let b = rect(&rt, "fan-B");
            assert!(b[1] > 0.0 && b[1] < 240.0, "starts at the top: {b:?}");
            rt.wheel(b[0] + 30.0, b[1] + 30.0, -600.0);
            rt.frame(1.0);
            assert_eq!(before, rect(&rt, "pick-answer"));
            click(&mut rt, "view-A");
            assert_eq!(shown(&rt), ["fan-A"]);
            let a = rect(&rt, "fan-A");
            assert!(
                a[1] > 0.0 && a[1] < 240.0,
                "new answer starts at the top: {a:?}"
            );
        }
    }
}

#[test]
fn history_defaults_to_the_winner_and_keeps_other_answers_browsable() {
    let shared = shared(fixtures::scene_fanout);
    {
        let mut core = lock(&shared);
        core.dialog = None;
        let Item::Fanout { picked, .. } = &mut core.t.items[0] else {
            unreachable!()
        };
        *picked = Some("C".into());
    }
    let mut rt = window(shared, (1180.0, 780.0), true);
    assert_eq!(shown(&rt), ["fan-C"]);
    assert!(rt.find("pick-answer").is_none());
    click(&mut rt, "view-A");
    assert_eq!(shown(&rt), ["fan-A"]);
}

#[test]
fn unrelated_selects_keep_their_normal_dialog() {
    let shared = shared(fixtures::scene_fanout);
    lock(&shared).dialog.as_mut().unwrap().kind =
        DialogKind::Select(vec!["First".into(), "Second".into()]);
    let mut rt = window(shared.clone(), (1180.0, 780.0), true);
    assert!(rt.find("pick-answer").is_none());
    rt.key(Key::Text("2".into()).into());
    rt.frame(0.016);
    assert!(lock(&shared).dialog.is_none());
}
