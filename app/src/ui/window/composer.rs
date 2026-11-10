//! The composer: its field, attachments, slash list and footer, and the keys, pastes and drops
//! it takes.

use rune::{ClipboardContent, El, Icon, Key, KeyEvent, TextStyle, Transition, Tx, col, row, text};
use rune_kit::{
    Button, ButtonVariant, ControlSize, IconButton, IconButtonVariant, Kit, ProgressCircle, popover,
};

use super::folder_picker;
use super::{Page, Window, catch_up, chat, dialog_keys, request_end, start_soon};
use crate::config;
use crate::core::{APP_COMMANDS, Core, cmd, lock};
use crate::transcript::{SHOT_EXTENSIONS, Shot};
use crate::ui::parts;

static FOLDER: Icon = Icon {
    name: "aw-folder",
    view: 16.0,
    body: r#"<path d="M2.5 4.5H6.5L8 6H13.5V12.5H2.5Z" stroke="currentColor" stroke-width="1.2" stroke-linejoin="round" fill="none"/>"#,
};
static SEND: Icon = Icon {
    name: "aw-send",
    view: 16.0,
    body: r#"<path d="M8 12.5V3.5M4 7.5L8 3.5L12 7.5" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round" fill="none"/>"#,
};
static REMOVE: Icon = Icon {
    name: "aw-remove",
    view: 16.0,
    body: r#"<path d="M5 5L11 11M11 5L5 11" stroke="currentColor" stroke-width="1.4" stroke-linecap="round"/>"#,
};
static STOP: Icon = Icon {
    name: "aw-stop",
    view: 16.0,
    body: r#"<rect x="4.5" y="4.5" width="7" height="7" rx="1.5" fill="currentColor"/>"#,
};

impl Window {
    fn slash_index(&self) -> usize {
        let (input, index) = &self.slash_pick;
        if *input != self.draft.composer.text {
            return 0;
        }
        *index
    }
}

fn submit(s: &mut Window, tx: &mut Tx, follow_up: bool) {
    let text = s.draft.composer.text.clone();
    if text.trim() == "/scoreboard" {
        s.draft.composer.clear();
        s.open_scoreboard(tx);
        return;
    }
    if lock(&s.core).dialog.is_some() {
        return;
    }
    s.draft.composer.clear();
    s.show(Page::Chat, tx);
    let shared = s.core.clone();
    let mut core = lock(&shared);
    if core.submit_with(&text, follow_up, &s.draft.attachments) {
        s.draft.attachments.clear();
    }
    // `/new` leaves as the New task button does.
    catch_up(s, &core, tx);
    start_soon(&core, tx);
    tx.scroll_to_bottom("transcript");
}

fn composer_key(s: &mut Window, ev: &KeyEvent, tx: &mut Tx) -> bool {
    let empty = s.draft.composer.text.is_empty();
    if dialog_keys(&mut lock(&s.core), ev, empty) {
        return true;
    }
    match &ev.key {
        Key::Escape if lock(&s.core).streaming => {
            lock(&s.core).abort();
            true
        }
        Key::Enter if ev.mods.cmd && ev.mods.shift => {
            submit(s, tx, true);
            true
        }
        Key::Enter if ev.mods.cmd => {
            request_end(s, tx);
            true
        }
        Key::Enter if ev.mods.shift => false,
        Key::Up | Key::Down | Key::Tab | Key::Enter => slash_key(s, &ev.key, tx),
        Key::Backspace if empty && !ev.mods.cmd => s.draft.attachments.pop().is_some(),
        _ => false,
    }
}

/// Files a drop or paste can attach, whatever their extension's case.
pub(super) fn is_shot_file(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| SHOT_EXTENSIONS.iter().any(|x| x.eq_ignore_ascii_case(e)))
}

/// Adds images to the composer. A sealed chat or an open dialog takes none, as it takes no prompt.
pub(super) fn attach(s: &mut Window, shots: impl IntoIterator<Item = Result<Shot, String>>) {
    let mut core = lock(&s.core);
    if core.t.week.done.is_some() || core.dialog.is_some() {
        return;
    }
    for shot in shots {
        match shot {
            Ok(shot) => s.draft.attachments.push(shot),
            Err(error) => core.toast(error, "error"),
        }
    }
    s.draft.composer.focused = true;
}

/// ⌘V with an image, a screenshot or image files copied in Finder attaches them; anything
/// else pastes as text.
fn composer_paste(s: &mut Window, content: &ClipboardContent, _: &mut Tx) -> bool {
    let files: Vec<_> = content.files().iter().filter(|p| is_shot_file(p)).collect();
    if !files.is_empty() {
        attach(s, files.into_iter().map(|p| Shot::from_file(p)));
        return true;
    }
    let Some(image) = content.image() else {
        return false;
    };
    let shot = Shot::from_bytes(image.png().to_vec())
        .ok_or_else(|| "The pasted image couldn’t be read.".to_owned());
    attach(s, [shot]);
    true
}

/// ↑/↓ move the slash list's highlight, Tab completes it, ⏎ runs it.
fn slash_key(s: &mut Window, key: &Key, tx: &mut Tx) -> bool {
    let matches = slash_matches(&s.draft.composer.text, &lock(&s.core));
    if matches.is_empty() {
        return false;
    }
    let count = matches.len();
    let index = s.slash_index().min(count - 1);
    let (name, description) = &matches[index];
    match key {
        Key::Up => s.slash_pick = (s.draft.composer.text.clone(), (index + count - 1) % count),
        Key::Down => s.slash_pick = (s.draft.composer.text.clone(), (index + 1) % count),
        Key::Tab => s.draft.composer.replace_text(format!("/{name} ")),
        _ => choose_slash(s, name, description, tx),
    }
    true
}

/// pi reports no arity, so a `<arg>` in the description marks a command that needs one.
fn needs_argument(description: &str) -> bool {
    description.contains('<')
}

/// ⏎ or a click on a slash row: run the command, or type it out when it still needs an argument.
fn choose_slash(s: &mut Window, name: &str, description: &str, tx: &mut Tx) {
    s.draft.composer.focused = true;
    if needs_argument(description) {
        s.draft.composer.replace_text(format!("/{name} "));
        return;
    }
    s.draft.composer.replace_text(format!("/{name}"));
    submit(s, tx, false);
}

fn slash_matches(input: &str, core: &Core) -> Vec<(String, String)> {
    let Some(prefix) = input.strip_prefix('/') else {
        return Vec::new();
    };
    if prefix.contains(' ') {
        return Vec::new();
    }
    APP_COMMANDS
        .iter()
        .map(|(n, d)| ((*n).to_owned(), (*d).to_owned()))
        .chain(
            core.commands
                .iter()
                .map(|c| (c.name.clone(), c.description.clone())),
        )
        .filter(|(n, _)| n.starts_with(prefix))
        .take(6)
        .collect()
}

/// One footer height for every state, so the busy line or the meter never moves the transcript.
const FOOTER_H: f32 = 24.0;
/// Names and descriptions sit in two columns, so every row shares one height.
const SLASH_ROW_H: f32 = 28.0;

fn slash_list(input: &str, highlighted: usize, core: &Core, k: &Kit) -> Option<El> {
    let c = &k.c;
    let matches = slash_matches(input, core);
    if matches.is_empty() {
        return None;
    }
    let highlighted = highlighted.min(matches.len() - 1);
    let cell = || row().h(SLASH_ROW_H).center_y();
    let highlight = row()
        .pin(
            Some(0.0),
            Some(highlighted as f32 * SLASH_ROW_H),
            Some(0.0),
            None,
        )
        .h(SLASH_ROW_H)
        .radius(k.radius.sm)
        .fill(c.hover);
    let mut names = col().pl(10.0).pr(12.0);
    let mut descriptions = col().grow(1.0).shrink().min_w(0.0).pr(8.0);
    let mut hits = col().pin(Some(0.0), Some(0.0), Some(0.0), None);
    for (index, (name, description)) in matches.iter().enumerate() {
        let (target, about) = (name.clone(), description.clone());
        hits = hits.child(
            row()
                .w_full()
                .h(SLASH_ROW_H)
                .role("button")
                .label(format!("/{name}"))
                .on_pointer_move(move |s: &mut Window, _| {
                    s.slash_pick = (s.draft.composer.text.clone(), index);
                })
                .on_press_tx(move |s: &mut Window, tx| choose_slash(s, &target, &about, tx)),
        );
    }
    for (name, description) in matches {
        names =
            names.child(cell().child(text(format!("/{name}")).font(k.ty.mono).color(c.text_base)));
        descriptions = descriptions.child(
            cell().child(
                text(description)
                    .font(k.ty.small)
                    .color(c.text_muted)
                    .truncate()
                    .shrink(),
            ),
        );
    }
    let list = col().padding(4.0).child(
        row()
            .w_full()
            .child(highlight)
            .child(names)
            .child(descriptions)
            .child(hits),
    );
    Some(
        popover::raised(k, list, k.radius.lg)
            .key("slash")
            .w_full()
            .transition(Transition::none()),
    )
}

fn composer_footer(s: &Window, core: &Core, k: &Kit) -> El {
    let mut status = row().gap(8.0).center_y().grow(1.0).shrink().min_w(0.0);
    if let Some(mode) = shell_mode(&s.draft.composer.text) {
        status = status.child(parts::chip(mode, k));
    }
    if chat::blank(core) {
        status = status.child(workspace_button(core, k));
    }
    if let Some(queue) = parts::queue_line(core, k) {
        status = status.child(queue);
    }
    let mut footer = row()
        .key("composer-footer")
        .w_full()
        .h(FOOTER_H)
        .pl(8.0)
        .gap(10.0)
        .center_y()
        .child(status);
    if let Some(meter) = context_meter(core, k) {
        footer = footer.child(meter);
    }
    footer.child(send_button(s, core, k))
}

/// A new task picks its folder where it's typed, as the session belongs to it from the first prompt.
fn workspace_button(core: &Core, k: &Kit) -> El {
    Button::new(config::short_path(&core.cwd))
        .icon(&FOLDER)
        .variant(ButtonVariant::Ghost)
        .control_size(ControlSize::Small)
        .help("Change the folder pi works in")
        .shortcut(cmd("O"))
        .on_press(folder_picker::show)
        .build(k)
        .key("workspace")
}

/// `!` runs a shell command and `!!` keeps its output out of the agent's context.
fn shell_mode(input: &str) -> Option<&'static str> {
    if input.starts_with("!!") {
        return Some("shell · kept out of context");
    }
    if input.starts_with('!') {
        return Some("shell");
    }
    None
}

/// One slot that is Stop while the agent works on an empty field and Send otherwise, so swapping never moves the footer.
fn send_button(s: &Window, core: &Core, k: &Kit) -> El {
    let empty = s.draft.composer.text.trim().is_empty() && s.draft.attachments.is_empty();
    if core.streaming && empty {
        return IconButton::new(&STOP)
            .variant(IconButtonVariant::Submit)
            .label("Stop")
            .shortcut("esc")
            .on_press(|s: &mut Window| lock(&s.core).abort())
            .build(k)
            .key("send");
    }
    let button = IconButton::new(&SEND)
        .variant(if empty {
            IconButtonVariant::GhostMuted
        } else {
            IconButtonVariant::Submit
        })
        .disabled(empty)
        .on_press_tx(|s: &mut Window, tx: &mut Tx| submit(s, tx, false));
    let button = if core.streaming {
        button
            .label("Steer")
            .help(format!("Steer  ⏎ · Queue  {}", cmd("⇧⏎")))
    } else {
        button.label("Send").shortcut("⏎")
    };
    button.build(k).key("send")
}

/// How full the context is, measured against where pi compacts on its own: at 100% the
/// next check compacts. Right after a compaction it shows pi's estimate of what was kept.
fn context_meter(core: &Core, k: &Kit) -> Option<El> {
    let stats = &core.stats;
    let percent = stats.context_percent().filter(|p| p.round() > 0.0)?;
    let tokens = |n: f64| format!("{:.0}k", n / 1000.0);
    let help = match (stats.reserve, stats.context_window) {
        (Some(reserve), Some(window)) if window > reserve => format!(
            "{} of {} tokens before pi compacts the context{}",
            tokens(stats.context_tokens.unwrap_or_default()),
            tokens(window - reserve),
            if stats.predicted {
                ", estimated after the last compaction"
            } else {
                ""
            }
        ),
        _ => "Context used. Auto-compaction is off.".to_owned(),
    };
    let meter = row()
        .key("context-meter")
        .gap(6.0)
        .center_y()
        .help(help)
        .child(
            ProgressCircle::new((percent.min(100.0) / 100.0) as f32)
                .size(14.0)
                .label("Context used until compaction")
                .build(k),
        )
        .child(
            text(format!(
                "{}{percent:.0}%",
                if stats.predicted { "~" } else { "" }
            ))
            .font(k.ty.small.features("'tnum' 1"))
            .color(k.c.text_muted),
        );
    Some(meter)
}

const COMPOSER_SHOT: f32 = 56.0;

/// The images the next prompt carries, each with a button that takes it off.
fn attachment_strip(s: &Window, k: &Kit) -> Option<El> {
    if s.draft.attachments.is_empty() {
        return None;
    }
    let tiles = s.draft.attachments.iter().enumerate().map(|(index, shot)| {
        let remove = IconButton::new(&REMOVE)
            .variant(IconButtonVariant::Neutral)
            .control_size(ControlSize::Small)
            .label("Remove image")
            .on_press(move |s: &mut Window| {
                if index < s.draft.attachments.len() {
                    s.draft.attachments.remove(index);
                }
            })
            .build(k)
            .pin(None, Some(-6.0), Some(-6.0), None);
        col()
            .key(format!("attachment-{index}"))
            .child(parts::shot_tile(s, shot, COMPOSER_SHOT, k))
            .child(remove)
    });
    Some(
        row()
            .key("attachments")
            .w_full()
            .gap(10.0)
            .flex_wrap()
            .px(8.0)
            .pt(8.0)
            .children(tiles),
    )
}

/// pi's composer field: plain, growing from two lines to ten, and taking pasted images.
fn composer_field(s: &Window, placeholder: &str, k: &Kit) -> El {
    let c = &k.c;
    let style = TextStyle {
        line_height: (13.0f32 * 1.35 * 2.0).round() / 2.0,
        ..k.ty.ui
    };
    let field = rune::text_input(&s.draft.composer, style)
        .key("input")
        .w_full()
        .color(c.text_base)
        .placeholder(placeholder, c.text_faint)
        .caret(c.text_base, k.selection())
        .label(placeholder)
        .on_edit(|s: &mut Window| &mut s.draft.composer)
        .on_submit(|s: &mut Window, tx: &mut Tx| submit(s, tx, false))
        .on_input_key(composer_key)
        .on_input_paste(composer_paste);
    col()
        .role("textbox")
        .w_full()
        .padding(8.0)
        .child(rune::auto_grow(field, 2, 10))
}

pub(super) fn build(s: &Window, core: &Core, k: &Kit) -> El {
    let placeholder = match (&core.t.week.deal, core.streaming) {
        (_, true) => "Steer the agent…",
        (None, _) => "What are we working on?",
        (Some(_), _) => "Reply…",
    };
    let field = composer_field(s, placeholder, k).key("composer");
    let focused = s.draft.composer.focused || s.dropping;
    let mut frame = col()
        .key("composer-frame")
        .w_full()
        .radius(k.radius.lg)
        .fill(k.c.surface_raised_base)
        .border(
            1.0,
            if focused {
                k.c.border_base
            } else {
                k.c.border_weak
            },
        )
        .padding(6.0);
    if let Some(strip) = attachment_strip(s, k) {
        frame = frame.child(strip);
    }
    let frame = frame.child(field).child(composer_footer(s, core, k));
    let mut column = col().w_full().gap(8.0);
    if let Some(working) = parts::working(core, k) {
        column = column.child(working);
    }
    if let Some(list) = slash_list(&s.draft.composer.text, s.slash_index(), core, k) {
        // A zero-height shelf the list hangs above, so opening it never moves the intro or the transcript.
        let shelf = row()
            .w_full()
            .h(0.0)
            .child(list.pin(Some(0.0), None, Some(0.0), Some(8.0)));
        column = column.child(shelf);
    }
    column.child(frame)
}
