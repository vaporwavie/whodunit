//! The default mode: an ordinary agent chat window (sessions, transcript, composer) under the week's rules.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use rune::{
    Cx, El, FileDialogOptions, Icon, Key, KeyEvent, ScrollSpec, Spring, TextEdit, Transition, Tx,
    col, row, text,
};
use rune_kit::shell::{
    self, PaneMotion, PaneState, RowStatus, Sidebar, SidebarRow, SidebarSection, SidebarToggle,
    SplitView, Toolbar,
};
use rune_kit::{Button, ButtonVariant, ControlSize, Kit, TextArea, popover};

use crate::config;
use crate::core::{APP_COMMANDS, Core, DialogKind, Leave, Shared, lock, score_key, start_pi};
use crate::scoreboard::{self, Board};
use crate::sessions::Summary;
use crate::transcript::Item;
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
        Key::Up | Key::Down | Key::Tab | Key::Enter => slash_key(s, &ev.key),
        _ => false,
    }
}

/// ↑/↓ move the slash list's highlight; Tab completes it, and so does ⏎ until the text is that command.
fn slash_key(s: &mut FullUi, key: &Key) -> bool {
    let matches = slash_matches(&s.composer.text, &lock(&s.core));
    if matches.is_empty() {
        return false;
    }
    let count = matches.len();
    let index = s.slash_index().min(count - 1);
    let step = match key {
        Key::Up => count - 1,
        Key::Down => 1,
        _ => {
            let (name, _) = &matches[index];
            if matches!(key, Key::Enter) && s.composer.text == format!("/{name}") {
                return false;
            }
            s.composer.replace_text(format!("/{name} "));
            return true;
        }
    };
    s.slash_pick = (s.composer.text.clone(), (index + step) % count);
    true
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
        pick_folder(tx);
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

fn pick_folder(tx: &mut Tx) {
    let options = FileDialogOptions {
        title: "Run pi in which folder?".into(),
        ..Default::default()
    };
    tx.open_folder(options, |s: &mut FullUi, result, _tx| {
        if let Ok(Some(path)) = result {
            let core = s.core.clone();
            std::thread::spawn(move || start_pi(&core, Some(PathBuf::from(path))));
        }
    });
}

pub fn poll(s: &mut FullUi, _t: f64, tx: &mut Tx) -> bool {
    let mut core = lock(&s.core);
    if let Some(next) = core.expire_toasts() {
        tx.wake_after(next + Duration::from_millis(50));
    }
    let mut changed = core.version != s.seen;
    s.seen = core.version;
    let switched = core.session_file != s.seen_session;
    if switched {
        s.seen_session = core.session_file.clone();
    }
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
    drop(core);
    if switched {
        tx.scroll_to_bottom_instant("transcript");
    }
    changed
}

fn session_row(s: &FullUi, core: &Core, summary: &Summary, k: &Kit) -> El {
    let current = core.session_file.as_deref() == Some(summary.path.to_string_lossy().as_ref());
    let path = summary.path.clone();
    let mut r = SidebarRow::new(summary.title.clone())
        .height(44.0)
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
                .detail("⌘N")
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
                .detail("⌘2")
                .height(32.0)
                .selected(s.page == Page::Scoreboard)
                .on_select(|s: &mut FullUi, _| s.open_scoreboard())
                .build(k)
                .key("scoreboard"),
        )
        .footer(
            SidebarRow::new(config::short_path(&core.cwd))
                .icon(&FOLDER)
                .detail("⌘O")
                .height(32.0)
                .on_select(|_: &mut FullUi, tx| pick_folder(tx))
                .build(k)
                .key("folder"),
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

/// One footer height for every state, so a hint or a Stop button never moves the transcript.
const FOOTER_H: f32 = 24.0;

fn slash_list(input: &str, highlighted: usize, core: &Core, k: &Kit) -> Option<El> {
    let c = &k.c;
    let matches = slash_matches(input, core);
    if matches.is_empty() {
        return None;
    }
    let highlighted = highlighted.min(matches.len() - 1);
    let mut list = col().gap(2.0).padding(6.0);
    for (index, (name, description)) in matches.into_iter().enumerate() {
        let mut item = row()
            .center_y()
            .gap(10.0)
            .px(8.0)
            .py(4.0)
            .radius(k.radius.sm);
        if index == highlighted {
            item = item.fill(c.hover);
        }
        list = list.child(
            item.child(text(format!("/{name}")).font(k.ty.mono).color(c.text_base))
                .child(
                    text(description)
                        .font(k.ty.small)
                        .color(c.text_muted)
                        .truncate()
                        .shrink(),
                ),
        );
    }
    Some(
        popover::raised(k, list, k.radius.md)
            .key("slash")
            .w_full()
            .transition(Transition::none()),
    )
}

fn composer_footer(core: &Core, hint: &str, k: &Kit) -> El {
    let c = &k.c;
    let mut status = row().gap(8.0).center_y().grow(1.0).shrink().min_w(0.0);
    if let Some(line) = parts::busy_line(core) {
        status = status
            .child(rune_kit::Spinner::new().size(12.0).build(k))
            .child(
                text(line)
                    .font(k.ty.small)
                    .color(c.text_muted)
                    .truncate()
                    .shrink(),
            );
    }
    if let Some(queue) = parts::queue_line(core, k) {
        status = status.child(queue);
    }
    let mut footer = row()
        .key("composer-footer")
        .w_full()
        .h(FOOTER_H)
        .px(8.0)
        .gap(8.0)
        .center_y()
        .child(status)
        .child(text(hint.to_owned()).font(k.ty.small).color(c.text_muted));
    if core.streaming {
        footer = footer.child(
            Button::new("Stop")
                .variant(ButtonVariant::Ghost)
                .control_size(ControlSize::Small)
                .shortcut("esc")
                .show_shortcut()
                .on_press(|s: &mut FullUi| lock(&s.core).abort())
                .build(k)
                .key("stop"),
        );
    }
    footer
}

fn composer(s: &FullUi, core: &Core, k: &Kit) -> El {
    let (placeholder, hint) = match (&core.t.week.deal, core.streaming) {
        (_, true) => ("Steer the agent…", "⌘⏎ queues a follow-up"),
        (None, _) => (
            "Describe one task. Jev picks the role, the deck picks the agent.",
            "⏎ sends",
        ),
        (Some(_), _) if core.t.week.done.is_some() => {
            ("This run has ended. ⌘N for the next task.", "")
        }
        (Some(_), _) => ("Reply…", "⌘D ends the run · !cmd runs a shell command"),
    };
    let sealed = core.t.week.done.is_some();
    let field = TextArea::new(&s.composer)
        .plain()
        .placeholder(placeholder)
        .width(None)
        .auto_grow(2, 10)
        .disabled(sealed)
        .on_edit(|s: &mut FullUi| &mut s.composer)
        .on_submit(|s: &mut FullUi, tx: &mut Tx| submit(s, tx, false))
        .on_key(composer_key)
        .build(k)
        .key("composer");
    let frame = parts::panel(k, s.composer.focused)
        .key("composer-frame")
        .padding(6.0)
        .child(field)
        .child(composer_footer(core, hint, k));
    let mut column = col().w_full().gap(8.0);
    if let Some(list) = slash_list(&s.composer.text, s.slash_index(), core, k) {
        column = column.child(list);
    }
    column.child(frame)
}

const COLUMN: f32 = 680.0;
/// A fan-out's cards sit side by side, so its session takes the wider column throughout.
const FANOUT_COLUMN: f32 = 820.0;

fn chat(s: &FullUi, core: &Core, k: &'static Kit) -> El {
    let c = &k.c;
    let fanout = core
        .t
        .items
        .iter()
        .any(|item| matches!(item, Item::Fanout { .. }));
    let width = if fanout { FANOUT_COLUMN } else { COLUMN };
    let body: El = if core.t.items.is_empty() && core.pending_prompt.is_none() {
        col()
            .w_full()
            .gap(10.0)
            .pt(100.0)
            .items_center()
            .child(
                text("One task per session")
                    .font(k.ty.body_medium)
                    .color(c.text_base),
            )
            .child(
                text("The role and the agent stay hidden until you end the run with ⌘D.")
                    .font(k.ty.small)
                    .color(c.text_muted),
            )
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
    } else {
        bottom = bottom.child(composer(s, core, k));
    }
    col().w_full().h_full().min_h(0.0).child(scroll).child(
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
    let numbers = k.ty.small.features("'tnum' 1");
    let mut trailing = row().gap(12.0).center_y();
    if let Some(line) = parts::role_line(core) {
        trailing = trailing.child(text(line).font(k.ty.small).color(k.c.text_muted));
    }
    if let Some(percent) = core.stats.context_percent {
        trailing = trailing.child(
            text(format!("{percent:.0}% context"))
                .font(numbers)
                .color(k.c.text_muted),
        );
    }
    if core.reveal().is_some() {
        trailing = trailing.child(
            text(format!("${:.3}", core.stats.cost))
                .font(numbers)
                .color(k.c.text_muted),
        );
    }
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
                .shortcut("⌘D")
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
        .child(text(format!("Finish a task and end it with ⌘D. Each role gets a table here, and an agent needs {} runs to rank.", scoreboard::TOO_FEW)).font(k.ty.small).color(c.text_muted))
        .child(
            row().pt(8.0).child(
                Button::new("Start a task")
                .variant(ButtonVariant::Outline)
                .shortcut("⌘1")
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
    let main = match (s.page, &s.board) {
        (Page::Scoreboard, Some(board)) => scoreboard_page(board, k),
        _ => chat(s, &core, k),
    };
    SplitView::new(main)
        .sidebar(
            &s.sidebar,
            |s: &mut FullUi| &mut s.sidebar,
            sidebar(s, &core, k),
        )
        .toolbar(toolbar(&core, s.page, k))
        .titlebar_inset(cx.titlebar_inset())
        .frame_inset(8.0)
        .toolbar_in_panel()
        .sidebar_padding(18.0, 10.0)
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
        .build(k)
}

pub fn setup(s: &mut FullUi, waker: rune::Waker) {
    lock(&s.core).add_waker(Arc::new(move || waker()));
}
