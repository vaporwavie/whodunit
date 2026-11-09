use std::sync::{Arc, Mutex};
use std::time::Duration;

use rune::{Env, Runtime, clipboard};
use whodunit::config::AppConfig;
use whodunit::core::{Core, Shared, lock};
use whodunit::transcript::{Block, FanAnswer, Item};
use whodunit::ui::full::{self, FullUi};

fn answer(body: &str, streaming: bool) -> Item {
    Item::Assistant {
        blocks: vec![Some(Block::Text(body.to_owned()))],
        streaming,
        error: None,
    }
}

fn window(items: Vec<Item>) -> (Shared, Runtime<FullUi>) {
    let core = Arc::new(Mutex::new(Core::new(
        AppConfig::default(),
        "/fixture".into(),
    )));
    lock(&core).t.items = items;
    let mut rt = Runtime::new(
        FullUi::new(core.clone()),
        full::view,
        (1180.0, 780.0),
        Env::new(true, 1.0),
    );
    rt.poll = Some(full::poll);
    rt.frame(0.0);
    rt.frame(1.0);
    (core, rt)
}

fn copy(rt: &mut Runtime<FullUi>, index: usize) {
    let buttons: Vec<_> = rt
        .elements()
        .into_iter()
        .filter(|el| el.key.as_deref() == Some("copy-code"))
        .collect();
    let r = buttons[index].rect;
    let (x, y) = (r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(0.0);
}

fn copied_labels(rt: &Runtime<FullUi>) -> usize {
    rt.elements()
        .iter()
        .filter(|el| el.text.as_deref() == Some("Copied"))
        .count()
}

#[test]
fn copy_preserves_code_without_prose_or_fences() {
    let source = "fn main() {\n\tprintln!(\"Olá 🦀\");\n\n}";
    let (core, mut rt) = window(vec![answer(
        &format!("Before.\n\n```rust\n{source}\n```\n\nAfter."),
        false,
    )]);
    let board = clipboard::memory();
    rt.use_clipboard(&board);
    copy(&mut rt, 0);
    assert_eq!(board.content().unwrap().text(), source);
    assert!(lock(&core).toasts.is_empty());
    assert_eq!(copied_labels(&rt), 1);
    std::thread::sleep(Duration::from_millis(1550));
    rt.frame(0.016);
    assert_eq!(copied_labels(&rt), 0);
}

#[test]
fn each_code_block_copies_its_own_source() {
    let (_, mut rt) = window(vec![answer(
        "```\nfirst\n```\n\n    second\n\n```sh\necho third\n```",
        false,
    )]);
    let board = clipboard::memory();
    rt.use_clipboard(&board);
    for (i, expected) in ["first", "second", "echo third"].iter().enumerate() {
        copy(&mut rt, i);
        assert_eq!(board.content().unwrap().text(), *expected);
        assert_eq!(copied_labels(&rt), i + 1);
    }
}

#[test]
fn streaming_copy_uses_the_latest_code() {
    let (core, mut rt) = window(vec![answer("```rust\nlet a = 1;\n", true)]);
    let board = clipboard::memory();
    rt.use_clipboard(&board);
    copy(&mut rt, 0);
    assert_eq!(board.content().unwrap().text(), "let a = 1;");
    {
        let mut core = lock(&core);
        core.t.items[0] = answer("```rust\nlet a = 1;\nlet b = 2;\n```", false);
        core.version += 1;
    }
    rt.frame(0.016);
    rt.frame(1.0);
    copy(&mut rt, 0);
    assert_eq!(board.content().unwrap().text(), "let a = 1;\nlet b = 2;");
}

#[test]
fn fanout_snippets_copy_independently() {
    let (_, mut rt) = window(vec![Item::Fanout {
        prompt: None,
        shots: Vec::new(),
        answers: ["A", "B"]
            .into_iter()
            .map(|label| FanAnswer {
                label: label.into(),
                ok: true,
                error: None,
                text: format!("```sh\necho {label}\n```"),
            })
            .collect(),
        picked: None,
    }]);
    let board = clipboard::memory();
    rt.use_clipboard(&board);
    for (i, expected) in ["echo A", "echo B"].iter().enumerate() {
        copy(&mut rt, i);
        assert_eq!(board.content().unwrap().text(), *expected);
        assert_eq!(copied_labels(&rt), i + 1);
    }
}
