//! The default mode: an ordinary agent chat window (sessions, transcript, composer) under the week's rules.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use rune::{
    Cx, El, Icon, Key, KeyEvent, ScrollSpec, Spring, TextEdit, Transition, Tx, col, row, text,
};
use rune_kit::diff::DiffState;
use rune_kit::shell::{
    self, PaneMotion, PaneState, RowStatus, Sidebar, SidebarRow, SidebarSection, SidebarToggle,
    SplitView, Toolbar,
};
use rune_kit::{
    Button, ButtonVariant, ControlSize, IconButton, IconButtonVariant, Kit, ProgressCircle,
    TextArea, popover,
};

use crate::config;
use crate::core::{APP_COMMANDS, Core, DialogKind, Leave, Shared, cmd, lock, score_key};
use crate::scoreboard::{self, Board};
use crate::sessions::Summary;
use crate::transcript::Item;
use crate::ui::dither;
use crate::ui::folder_picker::{self, FolderPicker};
use crate::ui::parts::{self, HasCore};

static PLUS: Icon = Icon {
    name: "aw-plus",
    view: 16.0,
    body: r#"<path d="M8 3.5V12.5M3.5 8H12.5" stroke="currentColor" stroke-width="1.2" stroke-linecap="round"/>"#,
};
static CHART: Icon = Icon {
    name: "aw-chart",
    view: 16.0,
    body: r#"<path d="M3 13V8M8 13V3M13 13V6" stroke="currentColor" stroke-width="1.4" stroke-linecap="round"/>"#,
};
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
static STOP: Icon = Icon {
    name: "aw-stop",
    view: 16.0,
    body: r#"<rect x="4.5" y="4.5" width="7" height="7" rx="1.5" fill="currentColor"/>"#,
};

#[derive(Clone, Copy, PartialEq)]
pub enum Page {
    Chat,
    Scoreboard,
}

pub struct FullUi {
    pub core: Shared,
    pub composer: TextEdit,
    pub dialog_edit: TextEdit,
    pub sidebar: PaneState,
    pub page: Page,
    pub board: Option<Board>,
    pub picker: FolderPicker,
    /// Diffs of the open session's `edit` and `write` calls, by call id.
    diffs: HashMap<String, DiffState>,
    /// The working timer's second as last drawn.
    ticked: Option<u64>,
    window_focused: bool,
    /// Reduce Motion as the last view saw it, for `poll`, which has no `Cx`.
    reduced_motion: AtomicBool,
    /// The slash list's highlighted row, for the composer text it was moved on.
    slash_pick: (String, usize),
    seen: u64,
    seen_session: Option<String>,
    editor_generation: u64,
    dialog_generation: u64,
}

impl HasCore for FullUi {
    fn core(&self) -> &Shared {
        &self.core
    }

    fn diff(&self, id: &str) -> Option<&DiffState> {
        self.diffs.get(id)
    }

    fn diff_mut(&mut self, id: &str) -> &mut DiffState {
        self.diffs.entry(id.to_owned()).or_default()
    }
}

impl FullUi {
    pub fn new(core: Shared) -> FullUi {
        let mut composer = TextEdit::new("");
        composer.focused = true;
        FullUi {
            core,
            composer,
            dialog_edit: TextEdit::new(""),
            sidebar: PaneState::new(264.0).range(180.0, 480.0),
            page: Page::Chat,
            board: None,
            picker: FolderPicker::default(),
            diffs: HashMap::new(),
            ticked: None,
            window_focused: false,
            reduced_motion: AtomicBool::new(false),
            slash_pick: (String::new(), 0),
            seen: 0,
            seen_session: None,
            editor_generation: 0,
            dialog_generation: 0,
        }
    }

    fn slash_index(&self) -> usize {
        let (input, index) = &self.slash_pick;
        if *input != self.composer.text {
            return 0;
        }
        *index
    }

    fn shore_motion(&self) -> dither::Motion {
        if self.reduced_motion.load(Ordering::Relaxed) {
            return dither::Motion::Still;
        }
        if !self.window_focused {
            return dither::Motion::Paused;
        }
        dither::Motion::Live
    }

    fn open_scoreboard(&mut self) {
        self.board = Some(scoreboard::compute());
        self.page = Page::Scoreboard;
    }
}

fn submit(s: &mut FullUi, tx: &mut Tx, follow_up: bool) {
    let text = s.composer.text.clone();
    if text.trim() == "/scoreboard" {
        s.composer.clear();
        s.open_scoreboard();
        return;
    }
    if lock(&s.core).dialog.is_some() {
        return;
    }
    s.composer.clear();
    s.page = Page::Chat;
    lock(&s.core).submit(&text, follow_up);
    tx.scroll_to_bottom("transcript");
}

/// Keys the dialogs, the unrated guard and the slash list take before the composer types them.
pub fn dialog_keys(core: &mut Core, ev: &KeyEvent, empty_field: bool) -> bool {
    if core.leave.is_some() {
        match ev.key {
            Key::Enter => core.answer_leave(Some(true)),
            Key::Escape => core.answer_leave(None),
            _ => return false,
        }
        return true;
    }
    let Some(dialog) = core.dialog.as_ref() else {
        return false;
    };
    let select = matches!(dialog.kind, DialogKind::Select(_));
    let confirm = matches!(dialog.kind, DialogKind::Confirm(_));
    match &ev.key {
        Key::Escape => core.answer_dialog(None),
        Key::Up if select => core.move_highlight(-1),
        Key::Down if select => core.move_highlight(1),
        Key::Enter if select => core.commit(),
        Key::Enter if confirm => core.answer_dialog(Some(serde_json::json!(true))),
        Key::Text(t) if confirm && empty_field && (t == "y" || t == "n") => {
            core.answer_dialog(Some(serde_json::json!(t == "y")))
        }
        Key::Text(t) if select && empty_field => {
            let DialogKind::Select(options) = &dialog.kind else {
                return false;
            };
            if dialog.ends_run() {
                let Some(index) = options
                    .iter()
                    .position(|o| score_key(o).is_some_and(|(key, _)| key == t))
                else {
                    return false;
                };
                core.choose(index);
                return true;
            }
            let by_number = t
                .parse::<usize>()
                .ok()
                .filter(|n| (1..=9).contains(n))
                .map(|n| n - 1);
            let by_label = options.iter().position(|o| o.eq_ignore_ascii_case(t));
            let Some(index) = by_label.or(by_number) else {
                return false;
            };
            core.pick(index);
        }
        _ => return false,
    }
    true
}

fn composer_key(s: &mut FullUi, ev: &KeyEvent, tx: &mut Tx) -> bool {
    let empty = s.composer.text.is_empty();
    if dialog_keys(&mut lock(&s.core), ev, empty) {
        return true;
    }
    match &ev.key {
        Key::Escape if lock(&s.core).streaming => {
            lock(&s.core).abort();
            true
        }
        Key::Enter if ev.mods.cmd => {
            submit(s, tx, true);
            true
        }
        Key::Enter if ev.mods.shift => false,
        Key::Up | Key::Down | Key::Tab | Key::Enter => slash_key(s, &ev.key, tx),
        _ => false,
    }
}

/// ↑/↓ move the slash list's highlight, Tab completes it, ⏎ runs it.
fn slash_key(s: &mut FullUi, key: &Key, tx: &mut Tx) -> bool {
    let matches = slash_matches(&s.composer.text, &lock(&s.core));
    if matches.is_empty() {
        return false;
    }
    let count = matches.len();
    let index = s.slash_index().min(count - 1);
    let (name, description) = &matches[index];
    match key {
        Key::Up => s.slash_pick = (s.composer.text.clone(), (index + count - 1) % count),
        Key::Down => s.slash_pick = (s.composer.text.clone(), (index + 1) % count),
        Key::Tab => s.composer.replace_text(format!("/{name} ")),
        _ => choose_slash(s, name, description, tx),
    }
    true
}

/// pi reports no arity, so a `<arg>` in the description marks a command that needs one.
fn needs_argument(description: &str) -> bool {
    description.contains('<')
}

/// ⏎ or a click on a slash row: run the command, or type it out when it still needs an argument.
fn choose_slash(s: &mut FullUi, name: &str, description: &str, tx: &mut Tx) {
    s.composer.focused = true;
    if needs_argument(description) {
        s.composer.replace_text(format!("/{name} "));
        return;
    }
    s.composer.replace_text(format!("/{name}"));
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

pub fn on_key(s: &mut FullUi, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if s.picker.open {
        if ev.is_command('o') {
            folder_picker::close(s);
            return true;
        }
        return folder_picker::on_key(s, ev, tx);
    }
    if dialog_keys(&mut lock(&s.core), ev, true) {
        return true;
    }
    if shell::is_sidebar_toggle(ev) {
        s.sidebar.toggle();
        return true;
    }
    if ev.is_command('n') {
        lock(&s.core).request_leave(Leave::New);
        s.page = Page::Chat;
        s.composer.focused = true;
        return true;
    }
    if ev.is_command('d') {
        lock(&s.core).end_run();
        return true;
    }
    if ev.is_command('o') {
        folder_picker::show(s);
        return true;
    }
    if ev.is_command('1') {
        s.page = Page::Chat;
        return true;
    }
    if ev.is_command('2') {
        s.open_scoreboard();
        return true;
    }
    if ev.key == Key::Escape && lock(&s.core).streaming {
        lock(&s.core).abort();
        return true;
    }
    false
}

pub fn poll(s: &mut FullUi, _t: f64, tx: &mut Tx) -> bool {
    let mut core = lock(&s.core);
    if let Some(next) = core.expire_toasts() {
        tx.wake_after(next + Duration::from_millis(50));
    }
    let picker = s.picker.poll();
    let mut changed = core.version != s.seen || picker;
    s.seen = core.version;
    let switched = core.session_file != s.seen_session;
    if switched {
        s.seen_session = core.session_file.clone();
        s.diffs.clear();
    }
    if changed || switched {
        let built = parts::tool_diffs(&core, |id| s.diffs.contains_key(id));
        changed |= !built.is_empty();
        s.diffs.extend(
            built
                .into_iter()
                .map(|(id, files)| (id, DiffState::new(files))),
        );
    }
    let second = core.turn_started.map(|started| started.elapsed().as_secs());
    if second != s.ticked {
        s.ticked = second;
        changed = true;
    }
    if let Some(started) = core.turn_started {
        let into = started.elapsed().subsec_millis() as u64;
        tx.wake_after(Duration::from_millis(1000 - into));
    }
    let relaunch = core.relaunch.take();
    if let Some((generation, text)) = core
        .editor_text
        .clone()
        .filter(|(g, _)| *g != s.editor_generation)
    {
        s.editor_generation = generation;
        s.composer.replace_text(text);
        changed = true;
    }
    if let Some(dialog) = core
        .dialog
        .as_ref()
        .filter(|d| d.generation != s.dialog_generation)
    {
        s.dialog_generation = dialog.generation;
        if let DialogKind::Input { prefill, .. } = &dialog.kind {
            s.dialog_edit = TextEdit::new(prefill.clone());
            s.dialog_edit.focused = true;
        }
    }
    let shore = s.page == Page::Chat && blank(&core) && s.shore_motion() == dither::Motion::Live;
    drop(core);
    if let Some((cwd, session)) = relaunch {
        let core = s.core.clone();
        std::thread::spawn(move || crate::core::start_pi(&core, Some(cwd), Some(session)));
    }
    if switched {
        tx.scroll_to_bottom_instant("transcript");
    }
    if shore {
        tx.wake_after(dither::FRAME);
    }
    changed || shore
}

fn session_row(s: &FullUi, core: &Core, summary: &Summary, k: &Kit) -> El {
    let current = core.session_file.as_deref() == Some(summary.path.to_string_lossy().as_ref());
    let path = summary.path.clone();
    let mut r = SidebarRow::new(summary.title.clone())
        .subtitle(summary.subtitle())
        .selected(current && s.page == Page::Chat)
        .on_select(move |s: &mut FullUi, _| {
            s.page = Page::Chat;
            lock(&s.core).request_leave(Leave::Open(path.clone()));
        });
    if current && core.streaming {
        r = r.status(RowStatus::Busy);
    } else if summary.week.unrated() {
        r = r.status(RowStatus::Attention);
    }
    r.build(k)
        .key(format!("session-{}", summary.path.display()))
}

fn sidebar(s: &FullUi, core: &Core, k: &Kit) -> El {
    let (unrated, recent): (Vec<&Summary>, Vec<&Summary>) =
        core.sessions.iter().partition(|x| x.week.unrated());
    let mut sb = Sidebar::new()
        .label("Sessions")
        .inset(0.0)
        .header_gap(2.0)
        .row_gap(12.0)
        .header(
            SidebarRow::new("New task")
                .icon(&PLUS)
                .detail(cmd("N"))
                .height(32.0)
                .on_select(|s: &mut FullUi, _| {
                    s.page = Page::Chat;
                    s.composer.focused = true;
                    lock(&s.core).request_leave(Leave::New);
                })
                .build(k)
                .key("new"),
        )
        .header(
            SidebarRow::new("Scoreboard")
                .icon(&CHART)
                .detail(cmd("2"))
                .height(32.0)
                .selected(s.page == Page::Scoreboard)
                .on_select(|s: &mut FullUi, _| s.open_scoreboard())
                .build(k)
                .key("scoreboard"),
        );
    if !unrated.is_empty() {
        sb = sb.section(
            SidebarSection::new("Unrated")
                .count(unrated.len())
                .rows(unrated.iter().map(|x| session_row(s, core, x, k))),
        );
    }
    if !recent.is_empty() {
        sb = sb.section(
            SidebarSection::new("Recent").rows(recent.iter().map(|x| session_row(s, core, x, k))),
        );
    }
    sb.build(k)
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
                .on_pointer_move(move |s: &mut FullUi, _| {
                    s.slash_pick = (s.composer.text.clone(), index);
                })
                .on_press_tx(move |s: &mut FullUi, tx| choose_slash(s, &target, &about, tx)),
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

fn composer_footer(s: &FullUi, core: &Core, k: &Kit) -> El {
    let mut status = row().gap(8.0).center_y().grow(1.0).shrink().min_w(0.0);
    if let Some(mode) = shell_mode(&s.composer.text) {
        status = status.child(parts::chip(mode, k));
    }
    if blank(core) {
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
fn send_button(s: &FullUi, core: &Core, k: &Kit) -> El {
    let empty = s.composer.text.trim().is_empty();
    if core.streaming && empty {
        return IconButton::new(&STOP)
            .variant(IconButtonVariant::Submit)
            .label("Stop")
            .shortcut("esc")
            .on_press(|s: &mut FullUi| lock(&s.core).abort())
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
        .on_press_tx(|s: &mut FullUi, tx: &mut Tx| submit(s, tx, false));
    let button = if core.streaming {
        button
            .label("Steer")
            .help(format!("Steer  ⏎ · Queue  {}", cmd("⏎")))
    } else {
        button.label("Send").shortcut("⏎")
    };
    button.build(k).key("send")
}

fn context_meter(core: &Core, k: &Kit) -> Option<El> {
    let percent = core.stats.context_percent.filter(|p| p.round() > 0.0)?;
    let meter = row()
        .key("context-meter")
        .gap(6.0)
        .center_y()
        .child(
            ProgressCircle::new((percent / 100.0) as f32)
                .size(14.0)
                .label("Context used")
                .build(k),
        )
        .child(
            text(format!("{percent:.0}%"))
                .font(k.ty.small.features("'tnum' 1"))
                .color(k.c.text_muted),
        );
    Some(meter)
}

fn composer(s: &FullUi, core: &Core, k: &Kit) -> El {
    let placeholder = match (&core.t.week.deal, core.streaming) {
        (_, true) => "Steer the agent…",
        (None, _) => "What are we working on?",
        (Some(_), _) => "Reply…",
    };
    let field = TextArea::new(&s.composer)
        .plain()
        .placeholder(placeholder)
        .width(None)
        .auto_grow(2, 10)
        .on_edit(|s: &mut FullUi| &mut s.composer)
        .on_submit(|s: &mut FullUi, tx: &mut Tx| submit(s, tx, false))
        .on_key(composer_key)
        .build(k)
        .key("composer");
    let frame = parts::panel(k, s.composer.focused)
        .key("composer-frame")
        .padding(6.0)
        .child(field)
        .child(composer_footer(s, core, k));
    let mut column = col().w_full().gap(8.0);
    if let Some(working) = parts::working(core, k) {
        column = column.child(working);
    }
    if let Some(list) = slash_list(&s.composer.text, s.slash_index(), core, k) {
        // A zero-height shelf the list hangs above, so opening it never moves the intro or the transcript.
        let shelf = row()
            .w_full()
            .h(0.0)
            .child(list.pin(Some(0.0), None, Some(0.0), Some(8.0)));
        column = column.child(shelf);
    }
    column.child(frame)
}

const COLUMN: f32 = 680.0;
/// A fan-out's cards sit side by side, so its session takes the wider column throughout.
const FANOUT_COLUMN: f32 = 820.0;

/// Sits on the composer, inset to its text, so reading it leads straight into typing.
fn empty_intro(k: &Kit) -> El {
    let c = &k.c;
    col()
        .key("empty-intro")
        .w_full()
        .gap(4.0)
        .px(14.0)
        .pb(6.0)
        .child(
            text("One task per session")
                .font(k.ty.body_medium)
                .color(c.text_base),
        )
        .child(
            text(format!("Describe the task below. The role and the agent stay hidden until you end the run with {}.", cmd("D")))
                .font(k.ty.ui)
                .color(c.text_muted)
                .wrap(),
        )
}

/// A sealed session takes no more input, so the composer gives way to the next step.
fn ended(k: &Kit) -> El {
    let c = &k.c;
    parts::panel(k, false).key("ended").padding(16.0).child(
        row()
            .gap(16.0)
            .center_y()
            .child(
                col()
                    .gap(4.0)
                    .grow(1.0)
                    .shrink()
                    .min_w(0.0)
                    .child(
                        text("This run has ended")
                            .font(k.ty.body_medium)
                            .color(c.text_base),
                    )
                    .child(
                        text("It’s scored and sealed. The next task starts in a new session.")
                            .font(k.ty.small)
                            .color(c.text_muted)
                            .wrap(),
                    ),
            )
            .child(
                Button::new("New task")
                    .variant(ButtonVariant::Contrast)
                    .shortcut(cmd("N"))
                    .show_shortcut()
                    .on_press(|s: &mut FullUi| {
                        s.page = Page::Chat;
                        s.composer.focused = true;
                        lock(&s.core).request_leave(Leave::New);
                    })
                    .build(k),
            ),
    )
}

/// No message yet: the intro and the shoreline show.
fn blank(core: &Core) -> bool {
    core.t.items.is_empty() && core.pending_prompt.is_none()
}

fn chat(s: &FullUi, core: &Core, k: &'static Kit, dark: bool) -> El {
    let fanout = core
        .t
        .items
        .iter()
        .any(|item| matches!(item, Item::Fanout { .. }));
    let width = if fanout { FANOUT_COLUMN } else { COLUMN };
    let empty = blank(core);
    let body: El = if empty {
        col()
    } else {
        parts::transcript::<FullUi>(core, k, width)
    };
    let scroll = col()
        .key("transcript")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .child(body)
        .scroll(ScrollSpec { stick_bottom: true });
    let mut bottom = col().w_full().max_w(width).gap(10.0);
    if empty {
        bottom = bottom.child(empty_intro(k));
    }
    if let Some(toasts) = parts::toasts(core, k) {
        bottom = bottom.child(toasts);
    }
    // A dialog or the guard takes the composer's place, so keys have one owner.
    if let Some(guard) = parts::leave_guard::<FullUi>(core, k) {
        bottom = bottom.child(guard);
    } else if let Some(dialog) =
        parts::dialog::<FullUi>(core, k, Some(&s.dialog_edit), |s: &mut FullUi| {
            &mut s.dialog_edit
        })
    {
        bottom = bottom.child(dialog);
    } else if core.t.week.done.is_some() {
        bottom = bottom.child(ended(k));
    } else {
        bottom = bottom.child(composer(s, core, k));
    }
    let mut stage = col().w_full().h_full().min_h(0.0).clip_rounded(0.0);
    if empty {
        stage = stage.child(dither::slot(dark, s.shore_motion()));
    }
    stage.child(scroll).child(
        col()
            .w_full()
            .items_center()
            .px(24.0)
            .pb(18.0)
            .pt(8.0)
            .child(bottom),
    )
}

fn toolbar(core: &Core, page: Page, k: &Kit) -> Toolbar {
    let title = match page {
        Page::Scoreboard => "Scoreboard".to_owned(),
        Page::Chat => core
            .t
            .name
            .clone()
            .or_else(|| core.t.first_prompt.clone())
            .or_else(|| core.pending_prompt.clone())
            .unwrap_or_else(|| "New task".to_owned()),
    };
    let title = title.lines().next().unwrap_or_default().to_owned();
    let mut trailing = row().gap(12.0).center_y();
    if core.t.week.unrated() {
        trailing = trailing.child(
            Button::new("End run")
                .variant(if core.can_end_run() {
                    ButtonVariant::Contrast
                } else {
                    ButtonVariant::Ghost
                })
                .disabled(!core.can_end_run())
                .control_size(ControlSize::Small)
                .shortcut(cmd("D"))
                .show_shortcut()
                .on_press(|s: &mut FullUi| lock(&s.core).end_run())
                .build(k)
                .key("end-run"),
        );
    }
    let title = text(title)
        .key("title")
        .font(k.ty.ui_medium)
        .color(k.c.text_base)
        .truncate()
        .shrink()
        .min_w(0.0);
    Toolbar::new().leading(title).trailing(trailing)
}

fn scoreboard_empty(k: &Kit) -> El {
    let c = &k.c;
    col()
        .key("scoreboard-empty")
        .w_full()
        .gap(10.0)
        .pt(100.0)
        .items_center()
        .child(text("No rated runs yet").font(k.ty.body_medium).color(c.text_base))
        .child(text(format!("Finish a task and end it with {}. Each role gets a table here, and an agent needs {} runs to rank.", cmd("D"), scoreboard::TOO_FEW)).font(k.ty.small).color(c.text_muted))
        .child(
            row().pt(8.0).child(
                Button::new("Start a task")
                .variant(ButtonVariant::Outline)
                .shortcut(cmd("1"))
                .show_shortcut()
                .on_press(|s: &mut FullUi| {
                    s.page = Page::Chat;
                    s.composer.focused = true;
                })
                .build(k),
            ),
        )
}

fn scoreboard_page(board: &Board, k: &Kit) -> El {
    let c = &k.c;
    if board.runs == 0 {
        return scoreboard_empty(k);
    }
    let numbers = k.ty.ui.features("'tnum' 1");
    let name_cell = |t: String, strong: bool| {
        text(t)
            .font(if strong { k.ty.ui_medium } else { k.ty.ui })
            .color(if strong { c.text_base } else { c.text_muted })
            .truncate()
            .grow(1.0)
            .min_w(160.0)
    };
    let cell = |t: String, w: f32| {
        row()
            .w(w)
            .justify_end()
            .child(text(t).font(numbers).color(c.text_muted))
    };
    let mut page = col().w_full().gap(22.0).px(28.0).py(22.0);
    let mut summary = col().gap(4.0).child(
        text(format!(
            "{} runs logged · agents under {} runs are marked too few and can’t rank first",
            board.runs,
            scoreboard::TOO_FEW
        ))
        .font(k.ty.small)
        .color(c.text_muted),
    );
    if let Some(line) = board.decisions.line() {
        summary = summary.child(
            text(format!("Role decided by {line}"))
                .font(k.ty.small.features("'tnum' 1"))
                .color(c.text_muted),
        );
    }
    page = page.child(summary);
    for table in &board.tables {
        let ranked = if table.fanout {
            "ranked by fan-out wins"
        } else {
            "ranked by mean score"
        };
        let mut t = col().w_full().gap(4.0).child(
            row()
                .gap(10.0)
                .center_y()
                .child(
                    text(table.role.clone())
                        .font(k.ty.body_medium)
                        .color(c.text_base),
                )
                .child(text(ranked).font(k.ty.small).color(c.text_muted)),
        );
        if let Some(line) = table.decisions.line() {
            t = t.child(
                text(format!("decided by {line}"))
                    .font(k.ty.small.features("'tnum' 1"))
                    .color(c.text_muted),
            );
        }
        let wins = table.fanout || table.rows.iter().any(|r| r.fanouts > 0);
        let mut header = row()
            .py(4.0)
            .child(name_cell("agent".into(), false))
            .child(cell("runs".into(), 60.0))
            .child(cell("nailed".into(), 70.0))
            .child(cell("score".into(), 70.0))
            .child(cell("your-min".into(), 80.0))
            .child(cell("cost/run".into(), 80.0));
        if wins {
            header = header.child(cell("wins".into(), 60.0));
        }
        header = header.child(cell("unrated".into(), 70.0));
        t = t.child(header);
        for (index, r) in table.rows.iter().enumerate() {
            let name = if r.too_few() {
                format!("{} · too few", r.name)
            } else {
                r.name.clone()
            };
            let fmt = |v: Option<f64>, d: usize| v.map_or("—".to_owned(), |v| format!("{v:.d$}"));
            let mut line = row()
                .key(format!("{}-{index}", table.role))
                .py(6.0)
                .border_sides([1.0, 0.0, 0.0, 0.0], c.border_weak)
                .child(name_cell(name, index == 0 && !r.too_few()))
                .child(cell(r.runs.to_string(), 60.0))
                .child(cell(fmt(r.nailed_pct(), 0) + "%", 70.0))
                .child(cell(fmt(r.mean_score(), 2), 70.0))
                .child(cell(fmt(r.median_your_min(), 1), 80.0))
                .child(cell(
                    r.cost_per_run()
                        .map_or("—".to_owned(), |v| format!("${v:.3}")),
                    80.0,
                ));
            if wins {
                line = line.child(cell(
                    if r.fanouts > 0 {
                        format!("{}/{}", r.wins, r.fanouts)
                    } else {
                        "—".into()
                    },
                    60.0,
                ));
            }
            t = t.child(line.child(cell(r.unrated.to_string(), 70.0)));
        }
        page = page.child(t);
    }
    let r = &board.reference;
    let fmt = |v: Option<f64>, d: usize| v.map_or("—".to_owned(), |v| format!("{v:.d$}"));
    page = page.child(
        col()
            .gap(4.0)
            .child(text("Claude Code reference").font(k.ty.body_medium).color(c.text_base))
            .child(
                text(format!(
                    "Last 30 days, {} sessions: median your-min {}, agent-min {}, turns {}. Different tasks and no blind deal, so read it as context.",
                    r.sessions,
                    fmt(r.your_minutes, 1),
                    fmt(r.agent_minutes, 1),
                    fmt(r.turns, 0)
                ))
                .font(k.ty.small)
                .color(c.text_muted)
                .wrap(),
            ),
    );
    col()
        .key("scoreboard-page")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .child(page)
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
}

pub fn view(s: &FullUi, cx: &mut Cx<FullUi>) -> El {
    let k = rune_theme_opencode::kit(cx.system_dark());
    let core = lock(&s.core);
    s.reduced_motion
        .store(cx.is_reduced_motion(), Ordering::Relaxed);
    let main = match (s.page, &s.board) {
        (Page::Scoreboard, Some(board)) => scoreboard_page(board, k),
        _ => chat(s, &core, k, cx.system_dark()),
    };
    let mut overlays = Vec::new();
    folder_picker::build(s, k, &mut overlays);
    let window = SplitView::new(main)
        .sidebar(
            &s.sidebar,
            |s: &mut FullUi| &mut s.sidebar,
            sidebar(s, &core, k),
        )
        .toolbar(toolbar(&core, s.page, k))
        .titlebar_inset(cx.titlebar_inset())
        .frame_inset(8.0)
        .toolbar_in_panel()
        .sidebar_padding(8.0, 10.0)
        .sidebar_toggle(
            SidebarToggle::new()
                .size(ControlSize::Large)
                .past_lights(16.0)
                .edge(18.0)
                .gap(4.0)
                .in_header(),
        )
        .pane_motion(PaneMotion::new().spring(Spring::new(0.3, 0.0)))
        .reduced_motion(cx.is_reduced_motion())
        .build(k);
    rune_kit::root(window, overlays).on_window_focus(|s: &mut FullUi, focused, tx| {
        s.window_focused = focused;
        if !focused {
            folder_picker::release_cmd(s, tx);
        }
    })
}

pub fn setup(s: &mut FullUi, waker: rune::Waker) {
    lock(&s.core).add_waker(Arc::new(move || waker()));
}
