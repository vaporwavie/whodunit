//! ⌘O: pick the folder pi runs in. Recent folders sit on top, the browsed folder's subfolders below.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant, SystemTime};

use rune::{
    Color, El, GrowthSpec, Icon, Key, KeyEvent, ScrollSpec, TextEdit, Tx, col, el, icon, row, text,
};
use rune_kit::{Button, ButtonVariant, Dialog, Kit, TextInput, glyphs};

use crate::config::{self, AppConfig};
use crate::core::{Leave, cmd};
use crate::folders::{self, Recent};
use crate::ui::full::FullUi;

const ROW_H: f32 = 30.0;
const LIST_H: f32 = 330.0;
/// Wide enough for "Select ⌘⏎" ("Select Ctrl+⏎" off macOS), so holding ⌘ doesn't nudge Cancel.
const OPEN_SLOT_W: f32 = if cfg!(target_os = "macos") {
    100.0
} else {
    124.0
};
const RECENT: &str = "Recent";
/// Only drawn under the recents, with a rule above it, to end their group.
const BROWSE: &str = "Browse";

type Wakers = Vec<Arc<dyn Fn() + Send + Sync>>;

/// ⌘ held this long before the button says so, so a ⌘Tab away doesn't flash it.
const CMD_HOLD: Duration = Duration::from_millis(300);

static GO_UP: Icon = Icon {
    name: "aw-go-up",
    view: 16.0,
    body: r#"<path d="M9.5 6L6 2.5L2.5 6M6 2.5V10.5A2.5 2.5 0 0 0 8.5 13H13.5" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" stroke-linejoin="round" fill="none"/>"#,
};

/// A folder read on a background thread; recents come only with the first one.
struct Listing {
    dir: PathBuf,
    folders: Vec<String>,
    recents: Option<Vec<Recent>>,
}

#[derive(Default)]
pub struct FolderPicker {
    pub open: bool,
    pub query: TextEdit,
    dir: PathBuf,
    folders: Vec<String>,
    recents: Vec<Recent>,
    /// Index into `targets()`, which run in screen order: recents, go up, subfolders. None means
    /// the browsed folder itself, which ⏎ then picks.
    highlight: Option<usize>,
    /// Moved to another folder: its first subfolder gets selected once it's read.
    moved: bool,
    now: Option<SystemTime>,
    /// When ⌘ went down, until `CMD_HOLD` turns it into `cmd`.
    cmd_since: Option<Instant>,
    /// ⌘ is held: ⏎ picks the browsed folder, and only then does the button say so.
    cmd: bool,
    /// A slow disk or a network volume can't stall the window, so folders are read off the UI thread.
    loading: Option<Receiver<Listing>>,
    wakers: Wakers,
}

#[derive(Clone, Copy, PartialEq)]
enum Line {
    Heading(&'static str),
    Up,
    Recent(usize),
    Folder(usize),
    Empty(&'static str),
}

impl FolderPicker {
    /// Browses the current folder's parent, so its siblings (the other projects) are one key away.
    pub fn show(config: &AppConfig, cwd: &Path, wakers: Wakers) -> FolderPicker {
        let home = config::home();
        let dir = match cwd.parent() {
            Some(parent) if cwd != home => parent.to_owned(),
            _ => cwd.to_owned(),
        };
        let mut picker = FolderPicker::with(dir.clone(), Vec::new(), Vec::new(), SystemTime::now());
        picker.wakers = wakers;
        let (config, cwd) = (config.clone(), cwd.to_owned());
        picker.load(dir, move || {
            folders::recents(&config)
                .into_iter()
                .filter(|r| r.path != cwd)
                .collect()
        });
        picker
    }

    pub fn with(
        dir: PathBuf,
        folders: Vec<String>,
        recents: Vec<Recent>,
        now: SystemTime,
    ) -> FolderPicker {
        let mut query = TextEdit::new("");
        query.focused = true;
        FolderPicker {
            open: true,
            query,
            dir,
            folders,
            recents,
            now: Some(now),
            ..FolderPicker::default()
        }
    }

    /// The headless `picker-cmd` scene: ⌘ held past `CMD_HOLD`.
    pub fn holding_cmd(mut self) -> FolderPicker {
        self.cmd = true;
        self
    }

    fn load(&mut self, dir: PathBuf, recents: impl FnOnce() -> Vec<Recent> + Send + 'static) {
        let (send, receive) = mpsc::channel();
        self.loading = Some(receive);
        let wakers = self.wakers.clone();
        let first = self.recents.is_empty() && self.folders.is_empty();
        std::thread::spawn(move || {
            let listing = Listing {
                folders: folders::subfolders(&dir),
                recents: first.then(recents),
                dir,
            };
            if send.send(listing).is_ok() {
                wakers.iter().for_each(|wake| wake());
            }
        });
    }

    /// Takes a finished listing, if one arrived, and turns a long enough ⌘ hold into `cmd`. A
    /// listing for a folder already left is dropped.
    pub fn poll(&mut self) -> bool {
        let held = self
            .cmd_since
            .is_some_and(|since| !self.cmd && since.elapsed() >= CMD_HOLD);
        if held {
            self.cmd = true;
        }
        let Some(listing) = self.loading.as_ref().and_then(|r| r.try_recv().ok()) else {
            return held;
        };
        self.loading = None;
        if listing.dir != self.dir {
            return held;
        }
        self.folders = listing.folders;
        if let Some(recents) = listing.recents {
            self.recents = recents;
        }
        self.highlight = self.landing();
        true
    }

    fn set_cmd(&mut self, down: bool, tx: &mut Tx) {
        if !down {
            self.cmd_since = None;
            self.cmd = false;
            return;
        }
        if self.cmd_since.is_none() {
            self.cmd_since = Some(Instant::now());
            tx.wake_after(CMD_HOLD);
        }
    }

    fn query(&self) -> String {
        self.query.text.trim().to_lowercase()
    }

    fn shown_recents(&self) -> Vec<usize> {
        let names = self
            .recents
            .iter()
            .map(|r| config::short_path(&r.path).to_lowercase())
            .collect::<Vec<_>>();
        ranked(&names, &self.query(), |name| {
            name.rsplit('/').next().unwrap_or(name).to_owned()
        })
    }

    fn shown_folders(&self) -> Vec<usize> {
        let names = self
            .folders
            .iter()
            .map(|f| f.to_lowercase())
            .collect::<Vec<_>>();
        ranked(&names, &self.query(), str::to_owned)
    }

    /// The "Go up" row: there is a parent, and no filter is hiding the rows that don't match.
    fn can_go_up(&self) -> bool {
        self.dir.parent().is_some() && self.query().is_empty()
    }

    /// Every row the selection can sit on, top to bottom as drawn.
    fn targets(&self) -> Vec<Line> {
        let recents = self.shown_recents().into_iter().map(Line::Recent);
        let up = self.can_go_up().then_some(Line::Up);
        let folders = self.shown_folders().into_iter().map(Line::Folder);
        recents.chain(up).chain(folders).collect()
    }

    /// Where the selection rests with no filter: on the first subfolder after a move, so ⏎ keeps
    /// going down, or on Go up in an empty folder; on nothing when the picker opens, so ⏎ picks the
    /// folder it browses.
    fn landing(&self) -> Option<usize> {
        if !self.moved {
            return None;
        }
        let targets = self.targets();
        let first = |wanted: fn(&Line) -> bool| targets.iter().position(wanted);
        first(|line| matches!(line, Line::Folder(_))).or_else(|| first(|line| *line == Line::Up))
    }

    fn highlighted(&self) -> Option<Line> {
        self.highlight
            .and_then(|index| self.targets().get(index).copied())
    }

    fn path_of(&self, line: Line) -> PathBuf {
        match line {
            Line::Recent(i) => self.recents[i].path.clone(),
            Line::Folder(i) => self.dir.join(&self.folders[i]),
            Line::Up => self
                .dir
                .parent()
                .map_or_else(|| self.dir.clone(), Path::to_owned),
            _ => self.dir.clone(),
        }
    }

    fn browse(&mut self, dir: PathBuf) {
        self.dir = dir.clone();
        self.folders.clear();
        self.query.clear();
        self.moved = true;
        self.highlight = None;
        self.load(dir, Vec::new);
    }

    fn up(&mut self) {
        let Some(parent) = self.dir.parent().map(Path::to_owned) else {
            return;
        };
        self.browse(parent);
    }

    /// From nothing selected, ↓ lands on the first row under Browse and ↑ on the last recent.
    fn step(&mut self, delta: isize) {
        let count = self.targets().len();
        if count == 0 {
            return;
        }
        let last = count as isize - 1;
        let from = match self.highlight {
            Some(index) => index as isize,
            None if delta > 0 => self.shown_recents().len() as isize - 1,
            None => self.shown_recents().len() as isize,
        };
        self.highlight = Some((from + delta).clamp(0, last) as usize);
    }

    fn select(&mut self, highlight: usize) {
        if self.highlight != Some(highlight) {
            self.highlight = Some(highlight);
        }
    }

    /// While filtering, the first match is selected, so ⏎ goes straight into it.
    fn filtered(&mut self) {
        if self.query().is_empty() {
            self.highlight = self.landing();
            return;
        }
        self.highlight = self
            .targets()
            .iter()
            .position(|line| !matches!(line, Line::Up));
    }

    /// The rows as drawn, with the target index of each selectable one.
    fn lines(&self) -> Vec<(Line, Option<usize>)> {
        let mut lines = Vec::new();
        let recents = self.shown_recents().len();
        for (target, line) in self.targets().into_iter().enumerate() {
            if target == 0 && recents > 0 {
                lines.push((Line::Heading(RECENT), None));
            }
            if target == recents && recents > 0 {
                lines.push((Line::Heading(BROWSE), None));
            }
            lines.push((line, Some(target)));
        }
        if self.shown_folders().is_empty() && self.loading.is_none() {
            let note = if self.query().is_empty() {
                "No folders here"
            } else {
                "No match"
            };
            lines.push((Line::Empty(note), None));
        }
        lines
    }

    /// The list row the selection is on, for the filter field's active descendant.
    fn selected_line(&self) -> Option<usize> {
        self.lines()
            .iter()
            .position(|(_, target)| target.is_some() && *target == self.highlight)
    }
}

/// Matches of `query` in `names`: names whose `stem` starts with it first, then any containing it.
fn ranked(names: &[String], query: &str, stem: impl Fn(&str) -> String) -> Vec<usize> {
    if query.is_empty() {
        return (0..names.len()).collect();
    }
    let (mut prefix, mut inside) = (Vec::new(), Vec::new());
    for (index, name) in names.iter().enumerate() {
        if stem(name).starts_with(query) {
            prefix.push(index);
        } else if name.contains(query) {
            inside.push(index);
        }
    }
    prefix.extend(inside);
    prefix
}

pub fn show(s: &mut FullUi) {
    let picker = {
        let core = crate::core::lock(&s.core);
        FolderPicker::show(&core.config, &core.cwd, core.wakers())
    };
    s.picker = picker;
    s.composer.focused = false;
}

pub fn close(s: &mut FullUi) {
    s.picker.open = false;
    s.picker.loading = None;
    s.picker.query.focused = false;
    s.composer.focused = true;
}

/// The window lost focus, so a ⌘ held for ⌘Tab won't come back up here.
pub fn release_cmd(s: &mut FullUi, tx: &mut Tx) {
    s.picker.set_cmd(false, tx);
}

fn run_in(s: &mut FullUi, path: PathBuf, tx: &mut Tx) {
    close(s);
    s.leave(Leave::NewIn(path), tx);
}

fn pick_here(s: &mut FullUi, tx: &mut Tx) {
    let dir = s.picker.dir.clone();
    run_in(s, dir, tx);
}

/// One step: a subfolder opens to its own subfolders, "Go up" opens the parent, a recent runs pi
/// at once, and with nothing selected the browsed folder runs pi there.
fn open(s: &mut FullUi, tx: &mut Tx) {
    let Some(line) = s.picker.highlighted() else {
        pick_here(s, tx);
        return;
    };
    let path = s.picker.path_of(line);
    match line {
        Line::Folder(_) | Line::Up => s.picker.browse(path),
        _ => run_in(s, path, tx),
    }
}

/// ↑↓ move, Home and End jump to the ends, ⏎ opens, ⌘⏎ picks the browsed folder, → goes into the
/// selected subfolder, ⌫ or ← on an empty filter goes up.
pub fn on_key(s: &mut FullUi, ev: &KeyEvent, tx: &mut Tx) -> bool {
    let empty = s.picker.query.text.is_empty();
    match ev.key {
        // Taken, or rune draws no frame for it and the Open button keeps its old label.
        Key::Modifier { .. } => s.picker.set_cmd(ev.mods.cmd, tx),
        Key::Escape => close(s),
        Key::Up => s.picker.step(-1),
        Key::Down => s.picker.step(1),
        Key::Home => s.picker.select(0),
        Key::End => s.picker.step(isize::MAX / 2),
        Key::Enter if ev.mods.cmd => pick_here(s, tx),
        Key::Enter => open(s, tx),
        Key::Right if empty && matches!(s.picker.highlighted(), Some(Line::Folder(_))) => {
            open(s, tx)
        }
        Key::Backspace | Key::Left if empty => s.picker.up(),
        _ => return false,
    }
    true
}

fn heading(label: &str, k: &Kit) -> El {
    let title = row().h(ROW_H).px(8.0).pt(6.0).center_y().child(
        text(label)
            .font(k.ty.small_medium)
            .color(k.c.text_muted)
            .truncate()
            .shrink(),
    );
    if label != BROWSE {
        return title;
    }
    col()
        .w_full()
        .pt(8.0)
        .child(el().h(1.0).mx(8.0).fill(k.c.border_weak))
        .child(title)
}

fn note(label: &str, k: &Kit) -> El {
    row()
        .h(ROW_H)
        .px(8.0)
        .center_y()
        .child(text(label).font(k.ty.ui).color(k.c.text_faint))
}

fn line_view(p: &FolderPicker, line: Line, target: usize, k: &Kit) -> El {
    let c = &k.c;
    let on = !p.cmd && p.highlight == Some(target);
    let glyph = match line {
        Line::Up => &GO_UP,
        _ => &glyphs::FOLDER,
    };
    let mut item = row()
        .w_full()
        .h(ROW_H)
        .px(8.0)
        .gap(8.0)
        .center_y()
        .radius(k.radius.md)
        .role("option")
        .focus_selected(on)
        .fill(if on { c.hover } else { Color::TRANSPARENT })
        .child(icon(glyph, 16.0).color(c.icon_muted).a11y_hidden());
    match line {
        Line::Up => {
            let parent = config::short_path(&p.path_of(line));
            item = item
                .label(format!("Go up to {parent}"))
                .child(text("Go up").font(k.ty.ui).color(c.text_base))
                .child(
                    text(parent)
                        .font(k.ty.ui)
                        .color(c.text_muted)
                        .truncate()
                        .grow(1.0)
                        .shrink(),
                )
                .child(text("⌫").font(k.ty.small).color(c.text_muted).a11y_hidden());
        }
        Line::Recent(i) => {
            let recent = &p.recents[i];
            let short = config::short_path(&recent.path);
            let (name, parent) = match recent.path.file_name() {
                Some(name) if short != "~" => (
                    name.to_string_lossy().into_owned(),
                    recent
                        .path
                        .parent()
                        .map(config::short_path)
                        .unwrap_or_default(),
                ),
                _ => (short, String::new()),
            };
            let ago = p
                .now
                .map(|now| folders::ago(recent.at, now))
                .unwrap_or_default();
            item = item
                .label(config::short_path(&recent.path))
                .child(
                    text(name)
                        .font(k.ty.ui_medium)
                        .color(c.text_base)
                        .truncate()
                        .shrink()
                        .min_w(0.0),
                )
                .child(
                    text(parent)
                        .font(k.ty.ui)
                        .color(c.text_muted)
                        .truncate()
                        .grow(1.0)
                        .shrink(),
                )
                .child(
                    text(ago)
                        .font(k.ty.small.features("'tnum' 1"))
                        .color(c.text_muted),
                );
        }
        Line::Folder(i) => {
            item = item
                .label(p.folders[i].clone())
                .child(
                    text(p.folders[i].clone())
                        .font(k.ty.ui)
                        .color(c.text_base)
                        .truncate()
                        .grow(1.0)
                        .shrink(),
                )
                .child(
                    icon(k.glyph.caret_right, 12.0)
                        .color(c.icon_faint)
                        .a11y_hidden(),
                );
        }
        Line::Heading(_) | Line::Empty(_) => {}
    }
    // Pointer movement, not hover: rows scrolling under a resting pointer must not steal the
    // keyboard's selection.
    item.on_pointer_move(move |s: &mut FullUi, _| s.picker.select(target))
        .on_press_tx(move |s: &mut FullUi, tx| {
            s.picker.select(target);
            open(s, tx);
        })
}

fn list(p: &FolderPicker, k: &Kit) -> El {
    let mut rows = col()
        .key("folder-rows")
        .w_full()
        .growth(GrowthSpec::default())
        .virtualize(ROW_H)
        .reveal_selected(true);
    for (index, (line, target)) in p.lines().into_iter().enumerate() {
        let view = match (line, target) {
            (Line::Heading(label), _) => heading(label, k),
            (Line::Empty(label), _) => note(label, k),
            (_, Some(target)) => line_view(p, line, target, k),
            (_, None) => continue,
        };
        rows = rows.child(view.key(format!("line-{index}")));
    }
    let viewport = col()
        .key("folder-viewport")
        .role("listbox")
        .label(format!("Folders in {}", config::short_path(&p.dir)))
        .w_full()
        .h(LIST_H)
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
        .scroll_margin(ROW_H)
        .child(rows);
    rune_kit::scroll_bar(k, "folder-scroll-area", "folder-viewport", viewport)
        .w_full()
        .px(8.0)
        .preserve_focus_on_press(true)
}

fn footer(p: &FolderPicker, k: &Kit) -> El {
    row()
        .w_full()
        .padding(16.0)
        .gap(8.0)
        .center_y()
        .justify_end()
        .border_sides([1.0, 0.0, 0.0, 0.0], k.c.border_weak)
        .child(
            Button::new("Cancel")
                .variant(ButtonVariant::Ghost)
                .shortcut("esc")
                .show_shortcut()
                .on_press(close)
                .build(k),
        )
        .child(
            row().w(OPEN_SLOT_W).justify_end().child(
                Button::new(if p.cmd { "Select" } else { "Open" })
                    .variant(ButtonVariant::Contrast)
                    .shortcut(if p.cmd { cmd("⏎") } else { "⏎".into() })
                    .show_shortcut()
                    .on_press_tx(|s: &mut FullUi, tx| {
                        if s.picker.cmd {
                            pick_here(s, tx);
                            return;
                        }
                        open(s, tx);
                    })
                    .build(k),
            ),
        )
}

pub fn build(s: &FullUi, k: &Kit, layer: &mut Vec<El>) {
    let p = &s.picker;
    if !p.open {
        return;
    }
    let field = TextInput::new(&p.query)
        .leading_icon(k.glyph.search)
        .placeholder("Filter folders")
        .width(None)
        .on_edit(|s: &mut FullUi| &mut s.picker.query)
        .on_change(|s: &mut FullUi, _| s.picker.filtered())
        .on_key(on_key)
        .active_descendant(p.selected_line().map(|line| format!("line-{line}")))
        .build(k)
        .key("folder-filter");
    Dialog::new("folders", "Which folder?")
        .width(600.0)
        .fit()
        .open(true)
        .on_close(close)
        .child(col().w_full().px(16.0).pb(8.0).child(field))
        .child(list(p, k))
        .child(el().h(8.0))
        .child(footer(p, k))
        .build(k, layer);
}
