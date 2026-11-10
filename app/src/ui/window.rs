//! The app's window: the session sidebar, then a chat (transcript, composer) or the scoreboard
//! page, under the week's rules.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use rune::{
    ClipboardContent, Cx, El, Family, Icon, ImageCache, Key, KeyEvent, ScrollSpec, Spring,
    TextEdit, TextStyle, Transition, Tx, col, el, row, text,
};
use rune_kit::diff::DiffState;
use rune_kit::shell::{
    self, PaneMotion, PaneState, RowStatus, Sidebar, SidebarHeader, SidebarRow, SidebarSection,
    SidebarToggle, SplitView, Toolbar,
};
use rune_kit::{
    Button, ButtonVariant, ControlSize, IconButton, IconButtonVariant, Kit, ProgressCircle,
    Tooltip, popover,
};

use crate::config;
use crate::core::{
    APP_COMMANDS, Core, DialogKind, Leave, SessionState, Shared, cmd, lock, score_key,
};
use crate::scoreboard::{self, Board};
use crate::sessions::Summary;
use crate::sprites;
use crate::transcript::{SHOT_EXTENSIONS, Shot};
use crate::ui::dither;
use crate::ui::folder_picker::{self, FolderPicker};
use crate::ui::parts::{self, HasCore};
use crate::ui::seal;
use crate::ui::stage::{self, Stage};

mod board;
mod chat;
#[cfg(test)]
mod tests;
mod viewer;

pub const WORDMARK_FONT: &[u8] = include_bytes!("../../assets/fonts/Fraunces.ttf");
const WORDMARK: TextStyle = TextStyle::new(20.0, 440.0)
    .family(Family::Named("Fraunces"))
    .variations("'SOFT' 100, 'WONK' 1, 'opsz' 144")
    .lh(24.0);
/// Sidebar width past the traffic lights that fits the wordmark and the toggle.
const WORDMARK_ROOM: f32 = 146.0;
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

#[derive(Clone, Copy, PartialEq)]
pub enum Page {
    Chat,
    Scoreboard,
}

/// What a chat's composer holds while another chat is selected.
#[derive(Default)]
struct Draft {
    composer: TextEdit,
    dialog_edit: TextEdit,
    attachments: Vec<Shot>,
    editor_generation: u64,
    dialog_generation: u64,
}

pub struct Window {
    pub core: Shared,
    pub composer: TextEdit,
    /// Images pasted or dropped on the composer, sent with the next prompt.
    pub attachments: Vec<Shot>,
    pub dialog_edit: TextEdit,
    /// Decodes prompt images for their tiles; set up with the window's waker.
    images: Option<ImageCache>,
    /// A file drag the composer would take is over the chat.
    dropping: bool,
    /// The image open full size over the window.
    pub viewing: Option<Shot>,
    pub sidebar: PaneState,
    ended_collapsed: bool,
    /// The tooltip of the sidebar icon the pointer rests on, by its key.
    sprite_tip: Option<String>,
    pub page: Page,
    pub board: Option<Board>,
    /// When the open board's decider fills start rising; None once they have, or for a board
    /// that should show its levels at once.
    board_rise_at: Option<Instant>,
    pub picker: FolderPicker,
    /// Diffs of the open session's `edit` and `write` calls, by call id.
    diffs: HashMap<String, DiffState>,
    copied_code: HashMap<String, Instant>,
    /// The working timer's second as last drawn.
    ticked: Option<u64>,
    window_focused: bool,
    /// Reduce Motion as the last view saw it, for `poll`, which has no `Cx`.
    reduced_motion: AtomicBool,
    lanes: stage::LaneClock,
    seal: seal::Sweep,
    seen_done: Option<bool>,
    /// The slash list's highlighted row, for the composer text it was moved on.
    slash_pick: (String, usize),
    seen: u64,
    seen_task: u64,
    seen_loads: u64,
    drafts: HashMap<u64, Draft>,
    editor_generation: u64,
    dialog_generation: u64,
}

impl HasCore for Window {
    fn core(&self) -> &Shared {
        &self.core
    }

    fn diff(&self, id: &str) -> Option<&DiffState> {
        self.diffs.get(id)
    }

    fn diff_mut(&mut self, id: &str) -> &mut DiffState {
        self.diffs.entry(id.to_owned()).or_default()
    }

    fn copied_code(&self) -> &HashMap<String, Instant> {
        &self.copied_code
    }

    fn mark_code_copied(&mut self, id: String) {
        self.copied_code
            .insert(id, Instant::now() + parts::COPIED_FOR);
    }

    fn image(&self, shot: &Shot) -> Option<rune::Image> {
        Some(self.images.as_ref()?.load(shot.bytes.clone()))
    }

    fn view_shot(&mut self, shot: Shot) {
        self.viewing = Some(shot);
    }

    fn stage_motion(&self) -> dither::Motion {
        self.shore_motion()
    }

    fn lane_age(&self, label: &str) -> f32 {
        self.lanes.age(label)
    }
}

impl Window {
    pub fn new(core: Shared) -> Window {
        let mut composer = TextEdit::new("");
        composer.focused = true;
        Window {
            core,
            composer,
            attachments: Vec::new(),
            dialog_edit: TextEdit::new(""),
            images: None,
            dropping: false,
            viewing: None,
            sidebar: PaneState::new(264.0).range(180.0, 480.0),
            ended_collapsed: true,
            sprite_tip: None,
            page: Page::Chat,
            board: None,
            board_rise_at: None,
            picker: FolderPicker::default(),
            diffs: HashMap::new(),
            copied_code: HashMap::new(),
            ticked: None,
            window_focused: false,
            reduced_motion: AtomicBool::new(false),
            lanes: stage::LaneClock::default(),
            seal: seal::Sweep::default(),
            seen_done: None,
            slash_pick: (String::new(), 0),
            seen: 0,
            seen_task: 0,
            seen_loads: 0,
            drafts: HashMap::new(),
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

    /// Pages switch often, and often from the keyboard, so the new one lands whole in one frame.
    fn show(&mut self, page: Page, tx: &mut Tx) {
        if self.page != page {
            tx.settle();
        }
        self.page = page;
    }

    /// Shows the chat in this commit, so the next frame is already it. A chat that needs a pi
    /// process gets one from `poll`, off the UI thread, on the frame this asks for.
    pub(crate) fn leave(&mut self, leave: Leave, tx: &mut Tx) {
        self.show(Page::Chat, tx);
        let shared = self.core.clone();
        let mut core = lock(&shared);
        core.request_leave(leave);
        catch_up(self, &core, tx);
        start_soon(&core, tx);
    }

    fn open_scoreboard(&mut self, tx: &mut Tx) {
        self.board = Some(scoreboard::compute());
        self.show(Page::Scoreboard, tx);
        self.board_rise_at = Some(Instant::now() + board::RISE_AFTER);
    }
}

fn submit(s: &mut Window, tx: &mut Tx, follow_up: bool) {
    let text = s.composer.text.clone();
    if text.trim() == "/scoreboard" {
        s.composer.clear();
        s.open_scoreboard(tx);
        return;
    }
    if lock(&s.core).dialog.is_some() {
        return;
    }
    s.composer.clear();
    s.show(Page::Chat, tx);
    let shared = s.core.clone();
    let mut core = lock(&shared);
    if core.submit_with(&text, follow_up, &s.attachments) {
        s.attachments.clear();
    }
    // `/new` leaves as the New task button does.
    catch_up(s, &core, tx);
    start_soon(&core, tx);
    tx.scroll_to_bottom("transcript");
}

/// Keys a dialog takes before the composer types them.
pub fn dialog_keys(core: &mut Core, ev: &KeyEvent, empty_field: bool) -> bool {
    if ev.mods.cmd {
        return false;
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

fn composer_key(s: &mut Window, ev: &KeyEvent, tx: &mut Tx) -> bool {
    let empty = s.composer.text.is_empty();
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
        Key::Backspace if empty && !ev.mods.cmd => s.attachments.pop().is_some(),
        _ => false,
    }
}

/// Files a drop or paste can attach, whatever their extension's case.
fn is_shot_file(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| SHOT_EXTENSIONS.iter().any(|x| x.eq_ignore_ascii_case(e)))
}

/// Adds images to the composer. A sealed chat or an open dialog takes none, as it takes no prompt.
fn attach(s: &mut Window, shots: impl IntoIterator<Item = Result<Shot, String>>) {
    let mut core = lock(&s.core);
    if core.t.week.done.is_some() || core.dialog.is_some() {
        return;
    }
    for shot in shots {
        match shot {
            Ok(shot) => s.attachments.push(shot),
            Err(error) => core.toast(error, "error"),
        }
    }
    s.composer.focused = true;
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
fn choose_slash(s: &mut Window, name: &str, description: &str, tx: &mut Tx) {
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

pub fn on_key(s: &mut Window, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if s.viewing.is_some() {
        // Esc closes the image, and only the image: it never also stops the agent.
        if ev.key == Key::Escape {
            viewer::close(s);
            return true;
        }
        return false;
    }
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
        s.leave(Leave::New, tx);
        s.composer.focused = true;
        return true;
    }
    if ev.key == Key::Enter && ev.mods.cmd && !ev.mods.shift {
        request_end(s, tx);
        return true;
    }
    if ev.is_command('o') {
        folder_picker::show(s);
        return true;
    }
    if ev.is_command('1') {
        s.show(Page::Chat, tx);
        return true;
    }
    if ev.is_command('2') {
        s.open_scoreboard(tx);
        return true;
    }
    if ev.is_command('i') {
        focus_composer(s, tx);
        return true;
    }
    if ev.key == Key::Escape && lock(&s.core).streaming {
        lock(&s.core).abort();
        return true;
    }
    false
}

fn request_end(s: &mut Window, tx: &mut Tx) {
    s.show(Page::Chat, tx);
    lock(&s.core).end_run();
}

/// ⌘I: back to the chat with the keyboard in its composer, or in the field of the dialog that
/// stands in for it. A sealed chat has neither.
fn focus_composer(s: &mut Window, tx: &mut Tx) {
    s.show(Page::Chat, tx);
    let core = lock(&s.core);
    if let Some(dialog) = &core.dialog {
        if matches!(dialog.kind, DialogKind::Input { .. }) {
            s.dialog_edit.focused = true;
        }
    } else if core.t.week.done.is_none() {
        s.composer.focused = true;
    }
}

/// Brings the window's own state up to the core: the selected chat's drafts, diffs and
/// prefilled text. Another chat, or this one reloaded from pi, lands whole in one frame: nothing
/// grows, reveals or springs over from what was on screen. Returns whether any of it changed.
fn catch_up(s: &mut Window, core: &Core, tx: &mut Tx) -> bool {
    let mut changed = core.version != s.seen;
    s.seen = core.version;
    let switched = core.task_id() != s.seen_task;
    if switched {
        let draft = s.drafts.remove(&core.task_id()).unwrap_or_default();
        let previous = Draft {
            composer: std::mem::replace(&mut s.composer, draft.composer),
            dialog_edit: std::mem::replace(&mut s.dialog_edit, draft.dialog_edit),
            attachments: std::mem::replace(&mut s.attachments, draft.attachments),
            editor_generation: s.editor_generation,
            dialog_generation: s.dialog_generation,
        };
        s.drafts.insert(s.seen_task, previous);
        s.seen_task = core.task_id();
        s.viewing = None;
        s.composer.focused = true;
        s.editor_generation = draft.editor_generation;
        s.dialog_generation = draft.dialog_generation;
        s.diffs.clear();
        tx.scroll_to_bottom_instant("transcript");
        changed = true;
    }
    let reloaded = core.loads != s.seen_loads;
    s.seen_loads = core.loads;
    let done = core.t.week.done.is_some();
    if switched || reloaded {
        s.copied_code.clear();
        s.seal.clear();
        tx.settle();
    } else if done && s.seen_done == Some(false) && s.page == Page::Chat {
        s.seal.start(Instant::now());
        tx.settle();
    }
    s.seen_done = Some(done);
    if changed {
        let built = parts::tool_diffs(core, |id| s.diffs.contains_key(id));
        s.diffs.extend(
            built
                .into_iter()
                .map(|(id, files)| (id, DiffState::new(files))),
        );
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
    changed
}

/// A chat selected without a pi process gets one from `poll`, off the UI thread. Ask for that
/// frame, since a switch leaves nothing animating to bring it.
fn start_soon(core: &Core, tx: &mut Tx) {
    if !core.unstarted.is_empty() {
        tx.wake_after(Duration::ZERO);
    }
}

pub fn poll(s: &mut Window, _t: f64, tx: &mut Tx) -> bool {
    let shared = s.core.clone();
    let mut core = lock(&shared);
    if let Some(next) = core.expire_toasts() {
        tx.wake_after(next + Duration::from_millis(50));
    }
    let now = Instant::now();
    let before = s.copied_code.len();
    s.copied_code.retain(|_, until| *until > now);
    if let Some(until) = s.copied_code.values().min() {
        tx.wake_after(*until - now);
    }
    let mut changed = s.copied_code.len() != before;
    changed |= s.picker.poll();
    changed |= catch_up(s, &core, tx);
    let sweeping = s.seal.progress().is_some();
    s.seal
        .tick(Instant::now(), s.shore_motion(), s.page == Page::Chat);
    changed |= sweeping;
    if s.seal.progress().is_some() && s.shore_motion() == dither::Motion::Live {
        tx.wake_after(dither::FRAME);
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
    let start = !core.unstarted.is_empty();
    let at_work = Stage::of(&core);
    s.lanes.track(at_work.as_ref());
    let live = s.page == Page::Chat
        && (chat::blank(&core) || at_work.is_some())
        && s.shore_motion() == dither::Motion::Live;
    drop(core);
    if let Some(at) = s.board_rise_at {
        let now = Instant::now();
        if now >= at {
            s.board_rise_at = None;
            changed = true;
        } else {
            tx.wake_after(at - now);
        }
    }
    if start {
        std::thread::spawn(move || crate::core::start_unstarted(&shared));
    }
    if live {
        tx.wake_after(dither::FRAME);
    }
    changed || live
}

/// One session in the sidebar; `names` are the agents' display names, read once per sidebar.
fn session_row(
    s: &Window,
    core: &Core,
    summary: &Summary,
    names: &BTreeMap<String, String>,
    k: &Kit,
    layer: &mut Vec<El>,
) -> El {
    let current = core.session_file.as_deref() == Some(summary.path.to_string_lossy().as_ref());
    let path = summary.path.clone();
    let mut r = SidebarRow::new(summary.title.clone())
        .subtitle(summary.subtitle())
        .selected(current && s.page == Page::Chat)
        .on_select(move |s: &mut Window, tx| s.leave(Leave::Open(path.clone()), tx));
    // The role's and the agent's sprites, only once the run has ended and they can show.
    // The role's sits in the title's line, so it drops to centre on both lines.
    let deal = summary.revealed();
    if let Some(deal) = deal {
        let file = summary.path.display();
        let role = sprite_tip(
            s,
            format!("role-{file}"),
            sprites::role_icon(&deal.role),
            deal.role.clone(),
            k,
            layer,
        );
        let agent = sprite_tip(
            s,
            format!("agent-{file}"),
            sprites::model_icon(&deal.agent),
            config::agent_name_in(names, &deal.agent),
            k,
            layer,
        );
        r = r.avatar(role.offset(0.0, ROLE_ICON_DROP)).trailing(agent);
    }
    if core.session_state(summary) == SessionState::Working {
        r = r.status(RowStatus::Busy);
    } else if core.tasks_waiting(summary) {
        r = r.status(RowStatus::Attention);
    }
    let mut r = r.build(k);
    if let Some(deal) = deal {
        r = r.label(format!(
            "{}, {}, {}, {}",
            summary.title,
            deal.role,
            config::agent_name_in(names, &deal.agent),
            summary.subtitle()
        ));
    }
    r.key(format!("session-{}", summary.path.display()))
}

/// Half the subtitle's line: the row puts its icon in the title's line, and the role icon
/// drops by this to centre on both lines.
const ROLE_ICON_DROP: f32 = 8.0;
const SPRITE_TIP_DELAY: Duration = Duration::from_millis(500);

/// A sidebar sprite that names itself in a tooltip while the pointer rests on it. `key` names
/// the tooltip; the sprite itself is keyed `{key}-icon`.
fn sprite_tip(
    s: &Window,
    key: String,
    art: &sprites::Art,
    name: String,
    k: &Kit,
    layer: &mut Vec<El>,
) -> El {
    let id = key.clone();
    let icon = sprite(art)
        .key(format!("{key}-icon"))
        .hover_delays(SPRITE_TIP_DELAY, Duration::ZERO)
        .on_hover_change(move |s: &mut Window, inside, _| {
            if inside {
                s.sprite_tip = Some(id.clone());
            } else if s.sprite_tip.as_ref() == Some(&id) {
                s.sprite_tip = None;
            }
        });
    let open = s.sprite_tip.as_ref() == Some(&key);
    Tooltip::new(key)
        .trigger(icon)
        .text(k, name)
        .open(open)
        .build(k, layer)
}

fn sidebar(s: &Window, core: &Core, k: &Kit, layer: &mut Vec<El>) -> El {
    // Only ended rows name their agent, so the file is read only while some show.
    let shown = |summary: &&Summary| {
        !s.ended_collapsed || core.session_state(summary) != SessionState::Ended
    };
    let names = if core
        .sessions
        .iter()
        .filter(shown)
        .any(|summary| summary.revealed().is_some())
    {
        config::agent_names()
    } else {
        BTreeMap::new()
    };
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
                .on_select(|s: &mut Window, tx| {
                    s.leave(Leave::New, tx);
                    s.composer.focused = true;
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
                .on_select(|s: &mut Window, tx| s.open_scoreboard(tx))
                .build(k)
                .key("scoreboard"),
        );
    for (state, label) in [
        (SessionState::Working, "Working"),
        (SessionState::Idle, "Idle"),
        (SessionState::Ended, "Ended"),
    ] {
        let sessions: Vec<_> = core
            .sessions
            .iter()
            .filter(|summary| core.session_state(summary) == state)
            .collect();
        if sessions.is_empty() {
            continue;
        }
        if state == SessionState::Ended {
            let mut ended = col()
                .key("Ended")
                .role("group")
                .label(label)
                .w_full()
                .gap(2.0)
                .child(
                    SidebarHeader::new(label)
                        .count(sessions.len())
                        .collapsed(s.ended_collapsed)
                        .on_toggle(|s: &mut Window, _| s.ended_collapsed = !s.ended_collapsed)
                        .build(k),
                );
            if !s.ended_collapsed {
                ended = ended.child(
                    col()
                        .key("ended-list")
                        .role("list")
                        .label("Ended sessions")
                        .focus_group()
                        .focus_follows()
                        .w_full()
                        .max_h(240.0)
                        .gap(2.0)
                        .scroll(ScrollSpec {
                            stick_bottom: false,
                        })
                        .scroll_edge_fade(16.0)
                        .children(
                            sessions
                                .iter()
                                .map(|summary| session_row(s, core, summary, &names, k, layer)),
                        ),
                );
            }
            sb = sb.footer(ended);
        } else {
            sb = sb.section(
                SidebarSection::new(label).count(sessions.len()).rows(
                    sessions
                        .iter()
                        .map(|summary| session_row(s, core, summary, &names, k, layer)),
                ),
            );
        }
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
                .on_pointer_move(move |s: &mut Window, _| {
                    s.slash_pick = (s.composer.text.clone(), index);
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
    if let Some(mode) = shell_mode(&s.composer.text) {
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
    let empty = s.composer.text.trim().is_empty() && s.attachments.is_empty();
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
    if s.attachments.is_empty() {
        return None;
    }
    let tiles = s.attachments.iter().enumerate().map(|(index, shot)| {
        let remove = IconButton::new(&REMOVE)
            .variant(IconButtonVariant::Neutral)
            .control_size(ControlSize::Small)
            .label("Remove image")
            .on_press(move |s: &mut Window| {
                if index < s.attachments.len() {
                    s.attachments.remove(index);
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
    let field = rune::text_input(&s.composer, style)
        .key("input")
        .w_full()
        .color(c.text_base)
        .placeholder(placeholder, c.text_faint)
        .caret(c.text_base, k.selection())
        .label(placeholder)
        .on_edit(|s: &mut Window| &mut s.composer)
        .on_submit(|s: &mut Window, tx: &mut Tx| submit(s, tx, false))
        .on_input_key(composer_key)
        .on_input_paste(composer_paste);
    col()
        .role("textbox")
        .w_full()
        .padding(8.0)
        .child(rune::auto_grow(field, 2, 10))
}

fn composer(s: &Window, core: &Core, k: &Kit) -> El {
    let placeholder = match (&core.t.week.deal, core.streaming) {
        (_, true) => "Steer the agent…",
        (None, _) => "What are we working on?",
        (Some(_), _) => "Reply…",
    };
    let field = composer_field(s, placeholder, k).key("composer");
    let focused = s.composer.focused || s.dropping;
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
    let title = text(title)
        .key("title")
        .font(k.ty.ui_medium)
        .color(k.c.text_base)
        .truncate()
        .shrink()
        .min_w(0.0);
    Toolbar::new().leading(title)
}

fn sprite(art: &sprites::Art) -> El {
    el().size(art.width as f32 * art.pt, art.height as f32 * art.pt)
        .pixels(&art.pixels)
}

fn wordmark(k: &Kit, past_lights: bool) -> El {
    let lead = if past_lights { 0.0 } else { shell::INSET };
    text("Whodunit")
        .key("wordmark")
        .font(WORDMARK)
        .color(k.c.text_base)
        .pl(lead)
        .truncate()
}

pub fn view(s: &Window, cx: &mut Cx<Window>) -> El {
    let k = rune_theme_opencode::kit(cx.system_dark());
    let core = lock(&s.core);
    s.reduced_motion
        .store(cx.is_reduced_motion(), Ordering::Relaxed);
    let main = match (s.page, &s.board) {
        (Page::Scoreboard, Some(board)) => board::page(board, s.board_rise_at.is_none(), k),
        _ => chat::build(s, &core, k, cx.system_dark()),
    };
    let mut overlays = Vec::new();
    let side = sidebar(s, &core, k, &mut overlays);
    folder_picker::build(s, k, &mut overlays);
    viewer::build(s, k, &mut overlays);
    let mut window =
        SplitView::new(main).sidebar(&s.sidebar, |s: &mut Window| &mut s.sidebar, side);
    if s.sidebar.width() - cx.titlebar_inset() >= WORDMARK_ROOM {
        window = window.sidebar_title(wordmark(k, cx.titlebar_inset() > 0.0));
    }
    let window = window
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
    rune_kit::root(window, overlays).on_window_focus(|s: &mut Window, focused, tx| {
        s.seal
            .tick(Instant::now(), s.shore_motion(), s.page == Page::Chat);
        s.window_focused = focused;
        if !focused {
            folder_picker::release_cmd(s, tx);
        }
    })
}

/// A fixed halfway frame for snapshots of the sealing sweep.
pub fn scene_sealing(s: &mut Window) {
    crate::fixtures::scene_reveal(&s.core);
    let core = lock(&s.core);
    s.seen_task = core.task_id();
    s.seen_loads = core.loads;
    s.seen_done = Some(true);
    let now = Instant::now();
    s.seal.start(now);
    s.seal
        .tick(now + Duration::from_millis(325), dither::Motion::Live, true);
}

pub fn setup(s: &mut Window, waker: rune::Waker) {
    s.images = Some(ImageCache::new(waker.clone()));
    lock(&s.core).add_waker(Arc::new(move || waker()));
}
