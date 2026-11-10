use std::sync::{Arc, Mutex};

use rune::{Env, Runtime, clipboard};
use whodunit::config::AppConfig;
use whodunit::core::{Core, Shared, lock};
use whodunit::transcript::{Block, FanAnswer, Item};
use whodunit::ui::window::{self, Window};

const FLOW: &str = "flowchart LR\n    A[Request] --> B{Cached?}\n    B -->|Yes| C[Reply]\n    B -->|No| D[Database]";
const TOKENS: &str = r##"{"name":"Field notes","colors":{"ink":"#234"},"typography":{"body":{"size":14,"sample":"Readable text"}},"spacing":{"small":8}}"##;

fn answer(body: &str, streaming: bool) -> Item {
    Item::Assistant {
        blocks: vec![Some(Block::Text(body.to_owned()))],
        streaming,
        error: None,
    }
}

fn window(items: Vec<Item>) -> (Shared, Runtime<Window>) {
    let core = Arc::new(Mutex::new(Core::new(
        AppConfig::default(),
        "/fixture".into(),
    )));
    lock(&core).t.items = items;
    let mut rt = Runtime::new(
        Window::new(core.clone()),
        window::view,
        (1180.0, 780.0),
        Env::new(true, 1.0),
    );
    rt.poll = Some(window::poll);
    rt.frame(0.0);
    rt.frame(1.0);
    (core, rt)
}

fn has_key(rt: &Runtime<Window>, key: &str) -> bool {
    rt.elements().iter().any(|e| e.key.as_deref() == Some(key))
}

fn has_text(rt: &Runtime<Window>, text: &str) -> bool {
    rt.elements()
        .iter()
        .any(|e| e.text.as_deref().is_some_and(|t| t.contains(text)))
}

fn has_label(rt: &Runtime<Window>, label: &str) -> bool {
    rt.elements()
        .iter()
        .any(|e| e.label.as_deref() == Some(label))
}

fn click(rt: &mut Runtime<Window>, key: &str, index: usize) {
    let r = rt
        .elements()
        .into_iter()
        .filter(|e| e.key.as_deref() == Some(key))
        .nth(index)
        .unwrap()
        .rect;
    let (x, y) = (r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(0.0);
    rt.frame(1.0);
}

#[test]
fn flowcharts_render_and_keep_their_source_copyable() {
    let (_, mut rt) = window(vec![answer(&format!("```mermaid\n{FLOW}\n```"), false)]);
    let board = clipboard::memory();
    rt.use_clipboard(&board);
    assert!(has_key(&rt, "preview-source"));
    for key in ["preview-source", "copy-code"] {
        let button = rt
            .elements()
            .into_iter()
            .find(|e| e.key.as_deref() == Some(key))
            .unwrap();
        assert_eq!(button.role, "button");
        assert!(button.focusable);
        assert_eq!(button.help, button.label);
        assert_eq!(button.rect[2], button.rect[3]);
        assert!(
            rt.elements()
                .iter()
                .any(|e| e.path == format!("{}/icon", button.path))
        );
    }
    assert!(has_label(&rt, "Show source"));
    assert!(!has_text(&rt, "Source"));
    assert!(!has_text(&rt, "Copy"));
    assert!(!has_text(&rt, "flowchart LR"));
    click(&mut rt, "copy-code", 0);
    assert_eq!(board.content().unwrap().text(), FLOW);
    assert!(has_label(&rt, "Copied"));
    click(&mut rt, "preview-source", 0);
    assert!(has_text(&rt, "flowchart LR"));
    assert!(has_label(&rt, "Show preview"));
    assert!(!has_text(&rt, "Preview"));
    click(&mut rt, "preview-source", 0);
    assert!(!has_text(&rt, "flowchart LR"));
}

#[test]
fn system_design_diagrams_use_the_native_renderer() {
    for source in [
        "sequenceDiagram\n    Client->>API: Request\n    API->>Database: Query",
        "stateDiagram-v2\n    [*] --> Idle\n    Idle --> Working",
        "C4Context\n    Person(user, \"User\")\n    System(app, \"App\")\n    Rel(user, app, \"Uses\")",
    ] {
        let (_, rt) = window(vec![answer(&format!("```mermaid\n{source}\n```"), false)]);
        assert!(has_key(&rt, "preview-source"), "{source}");
    }
}

#[test]
fn unsupported_mermaid_stays_visible_as_source() {
    let (_, rt) = window(vec![answer(
        "```mermaid\nunknownDiagram\nA --> B\n```",
        false,
    )]);
    assert!(has_text(&rt, "unknownDiagram"));
    assert!(!has_key(&rt, "preview-source"));
}

#[test]
fn design_tokens_render_and_toggle_without_changing_copy() {
    let (_, mut rt) = window(vec![answer(
        &format!("```design-system\n{TOKENS}\n```"),
        false,
    )]);
    let board = clipboard::memory();
    rt.use_clipboard(&board);
    assert!(has_key(&rt, "design-system-preview"));
    for label in [
        "Field notes",
        "Colors",
        "Typography",
        "Readable text",
        "Spacing",
        "small · 8 pt",
    ] {
        assert!(has_text(&rt, label), "{label}");
    }
    click(&mut rt, "preview-source", 0);
    assert!(!has_key(&rt, "design-system-preview"));
    assert!(has_text(&rt, TOKENS));
    click(&mut rt, "copy-code", 0);
    assert_eq!(board.content().unwrap().text(), TOKENS);
    click(&mut rt, "preview-source", 0);
    assert!(has_key(&rt, "design-system-preview"));
}

#[test]
fn incomplete_design_tokens_wait_and_invalid_tokens_explain_the_error() {
    let (core, mut rt) = window(vec![answer("```design-system\n{\"colors\":", true)]);
    assert!(!has_key(&rt, "design-system-preview"));
    assert!(!has_text(&rt, "Cannot preview"));
    {
        let mut core = lock(&core);
        core.t.items[0] = answer("```design-system\n{\"spacing\":{\"bad\":-4}}\n```", false);
        core.version += 1;
    }
    rt.frame(0.016);
    assert!(has_text(&rt, "Cannot preview design system"));
    assert!(has_text(&rt, "spacing must be 0–128"));
    assert!(has_text(&rt, "\"bad\":-4"));
    assert!(has_key(&rt, "copy-code"));
    {
        let mut core = lock(&core);
        core.t.items[0] = answer(&format!("```design-system\n{TOKENS}\n```"), false);
        core.version += 1;
    }
    rt.frame(0.016);
    assert!(has_key(&rt, "design-system-preview"));
}

#[test]
fn streaming_diagrams_update_the_source_used_by_copy() {
    let (core, mut rt) = window(vec![answer("```mermaid\nflowchart LR\nA --> B\n", true)]);
    assert!(has_key(&rt, "preview-source"));
    {
        let mut core = lock(&core);
        core.t.items[0] = answer(&format!("```mermaid\n{FLOW}\n```"), false);
        core.version += 1;
    }
    rt.frame(0.016);
    rt.frame(1.0);
    let board = clipboard::memory();
    rt.use_clipboard(&board);
    click(&mut rt, "copy-code", 0);
    assert_eq!(board.content().unwrap().text(), FLOW);
}

#[test]
fn fanout_previews_keep_independent_controls() {
    let (_, mut rt) = window(vec![Item::Fanout {
        prompt: None,
        shots: vec![],
        answers: [FLOW, "flowchart TD\nX --> Y"]
            .into_iter()
            .enumerate()
            .map(|(i, source)| FanAnswer {
                label: char::from(b'A' + i as u8).to_string(),
                text: format!("```mermaid\n{source}\n```"),
                ok: true,
                error: None,
            })
            .collect(),
        picked: None,
    }]);
    let board = clipboard::memory();
    rt.use_clipboard(&board);
    click(&mut rt, "preview-source", 0);
    assert!(has_text(&rt, "flowchart LR"));
    assert!(!has_text(&rt, "flowchart TD"));
    click(&mut rt, "view-B", 0);
    assert!(!has_text(&rt, "flowchart LR"));
    click(&mut rt, "copy-code", 0);
    assert_eq!(board.content().unwrap().text(), "flowchart TD\nX --> Y");
    click(&mut rt, "view-A", 0);
    assert!(has_text(&rt, "flowchart LR"));
}

#[test]
fn reloading_a_chat_does_not_reuse_another_blocks_source_toggle() {
    let (core, mut rt) = window(vec![answer(&format!("```mermaid\n{FLOW}\n```"), false)]);
    click(&mut rt, "preview-source", 0);
    assert!(has_text(&rt, "flowchart LR"));
    {
        let mut core = lock(&core);
        core.t.items[0] = answer(&format!("```design-system\n{TOKENS}\n```"), false);
        core.loads += 1;
        core.version += 1;
    }
    rt.frame(0.016);
    assert!(has_key(&rt, "design-system-preview"));
    assert!(!has_text(&rt, TOKENS));
}

#[test]
fn fanout_can_switch_between_design_tokens_and_a_diagram() {
    let (_, mut rt) = window(vec![Item::Fanout {
        prompt: None,
        shots: vec![],
        answers: [("A", "design-system", TOKENS), ("B", "mermaid", FLOW)]
            .into_iter()
            .map(|(label, language, source)| FanAnswer {
                label: label.into(),
                text: format!("```{language}\n{source}\n```"),
                ok: true,
                error: None,
            })
            .collect(),
        picked: None,
    }]);
    assert!(has_key(&rt, "design-system-preview"));
    click(&mut rt, "view-B", 0);
    assert!(!has_key(&rt, "design-system-preview"));
    click(&mut rt, "preview-source", 0);
    assert!(has_text(&rt, "flowchart LR"));
    click(&mut rt, "view-A", 0);
    assert!(has_key(&rt, "design-system-preview"));
    click(&mut rt, "preview-source", 0);
    assert!(!has_key(&rt, "design-system-preview"));
    assert!(has_text(&rt, TOKENS));
}
