//! Borderless title editing in the toolbar and the selected sidebar row.

use rune::{Cursor, El, Key, KeyEvent, TextEdit, Tx, text, text_input};
use rune_kit::Kit;

use super::Window;
use crate::core::lock;

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Place {
    Title,
    Sidebar,
}

pub(super) struct Rename {
    task: u64,
    place: Place,
    original: String,
    edit: TextEdit,
}

pub(super) fn begin(s: &mut Window, place: Place, tx: &mut Tx) {
    let previous = s
        .renaming
        .as_ref()
        .map(|rename| rename.edit.text.trim().to_owned())
        .filter(|name| !name.is_empty());
    save(s, tx);
    let core = lock(&s.core);
    let original = previous
        .or_else(|| core.t.title())
        .or_else(|| core.pending_prompt.clone())
        .unwrap_or_default();
    let mut edit = TextEdit::new(original.clone());
    edit.focused = true;
    edit.anchor = 0;
    edit.focus = edit.text.len();
    s.renaming = Some(Rename {
        task: core.task_id(),
        place,
        original,
        edit,
    });
    s.draft.composer.focused = false;
    s.draft.dialog_edit.focused = false;
}

pub(super) fn edit(s: &mut Window) -> &mut TextEdit {
    &mut s.renaming.as_mut().expect("title is being edited").edit
}

pub(super) fn at(s: &Window, place: Place) -> Option<&TextEdit> {
    s.renaming
        .as_ref()
        .filter(|rename| rename.place == place)
        .map(|rename| &rename.edit)
}

pub(super) fn save(s: &mut Window, _: &mut Tx) {
    let Some(rename) = s.renaming.take() else {
        return;
    };
    let name = rename.edit.text.trim();
    if !name.is_empty() && name != rename.original {
        let mut core = lock(&s.core);
        if rename.task == core.task_id() {
            core.rename_chat(name);
        }
    }
}

pub(super) fn cancel(s: &mut Window, _: &mut Tx) {
    s.renaming = None;
}

pub(super) fn on_key(s: &mut Window, ev: &KeyEvent, tx: &mut Tx) -> bool {
    match ev.key {
        Key::Escape => cancel(s, tx),
        Key::Enter => save(s, tx),
        _ => return false,
    }
    true
}

/// Rune writes focus changes back through the editor accessor, including clicks on blank space.
pub(super) fn poll(s: &mut Window, tx: &mut Tx) -> bool {
    if s.renaming.is_some() {
        tx.wake_after(std::time::Duration::from_millis(16));
    }
    if s.renaming
        .as_ref()
        .is_some_and(|rename| !rename.edit.focused)
    {
        save(s, tx);
        return true;
    }
    false
}

pub(super) fn title(s: &Window, title: String, k: &Kit) -> El {
    let title = if let Some(editor) = at(s, Place::Title) {
        text_input(editor, k.ty.ui_medium)
            .single_line()
            .label("Chat title")
            .color(k.c.text_base)
            .caret(k.c.text_base, k.selection())
            .min_w(120.0)
            .on_edit(edit)
            .on_input_key(on_key)
            .on_submit(save)
            .focus_return("title")
            .key("rename-title")
    } else {
        text(title)
            .key("title")
            .font(k.ty.ui_medium)
            .color(k.c.text_base)
            .truncate()
            .role("button")
            .label("Rename chat")
            .focusable(true)
            .cursor(Cursor::Text)
            .hover(k.c.bg_accent.alpha(0.08))
            .on_press_tx(|s: &mut Window, tx| begin(s, Place::Title, tx))
    };
    title
        .px(4.0)
        .py(4.0)
        .radius(k.radius.sm)
        .shrink()
        .min_w(0.0)
}

pub fn scene(s: &mut Window) {
    crate::fixtures::scene_chat(&s.core);
    begin(s, Place::Title, &mut Tx::default());
}

pub fn sidebar_scene(s: &mut Window) {
    crate::fixtures::scene_chat(&s.core);
    begin(s, Place::Sidebar, &mut Tx::default());
}
