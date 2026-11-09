//! The app's window: the session sidebar, then a chat (transcript, composer) or the scoreboard
//! page, under the week's rules.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use rune::{
    ClipboardContent, Color, Cx, El, Family, Icon, ImageCache, Key, KeyEvent, ScrollSpec, Shape,
    Spring, TextEdit, TextStyle, Transition, Tx, col, el, row, text,
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
use crate::transcript::{Item, SHOT_EXTENSIONS, Shot};
use crate::ui::dither;
use crate::ui::folder_picker::{self, FolderPicker};
use crate::ui::parts::{self, HasCore};
use crate::ui::seal;
use crate::ui::stage::{self, Stage};

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
        self.board_rise_at = Some(Instant::now() + BOARD_RISE_AFTER);
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
            close_viewer(s);
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
        && (blank(&core) || at_work.is_some())
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
            text("One task per chat. As many chats as you need.")
                .font(k.ty.body_medium)
                .color(c.text_base),
        )
        .child(
            text(format!("Start another task with {} while this one works. End each run with {} to score it and reveal the role and agent.", cmd("N"), cmd("⏎")))
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
                        text("Done & dusted.")
                            .font(k.ty.body_medium)
                            .color(c.text_base),
                    )
                    .child(
                        text("This chat is scored and sealed. Start another task or return to an open chat.")
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
                    .on_press_tx(|s: &mut Window, tx| {
                        s.leave(Leave::New, tx);
                        s.composer.focused = true;
                    })
                    .build(k),
            ),
    )
}

/// No message yet: the intro and the shoreline show.
fn blank(core: &Core) -> bool {
    core.t.items.is_empty() && core.pending_prompt.is_none()
}

fn chat(s: &Window, core: &Core, k: &'static Kit, dark: bool) -> El {
    let fanout = Stage::of(core).is_some_and(|stage| stage.wide())
        || core
            .t
            .items
            .iter()
            .any(|item| matches!(item, Item::Fanout { .. }));
    let width = if fanout { FANOUT_COLUMN } else { COLUMN };
    let empty = blank(core);
    let body: El = if empty {
        col()
    } else {
        parts::transcript::<Window>(core, k, width)
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
    // A dialog takes the composer's place, so keys have one owner.
    if let Some(dialog) =
        parts::dialog::<Window>(core, k, Some(&s.dialog_edit), |s: &mut Window| {
            &mut s.dialog_edit
        })
    {
        bottom = bottom.child(dialog);
    } else if core.t.week.done.is_some() {
        bottom = bottom.child(ended(k));
    } else {
        bottom = bottom.child(composer(s, core, k));
        if core.can_end_run() {
            bottom = bottom.child(
                row()
                    .key("finish-run")
                    .w_full()
                    .center_y()
                    .gap(12.0)
                    .pl(14.0)
                    .child(
                        text("Call it done?")
                            .font(k.ty.ui)
                            .color(k.c.text_muted)
                            .grow(1.0)
                            .shrink(),
                    )
                    .child(
                        Button::new("I am done")
                            .variant(ButtonVariant::Contrast)
                            .control_size(ControlSize::Large)
                            .shortcut(cmd("⏎"))
                            .show_shortcut()
                            .on_press_tx(request_end)
                            .build(k)
                            .key("end-run"),
                    ),
            );
        }
    }
    let mut stage = col().w_full().h_full().min_h(0.0).clip_rounded(0.0);
    // Images dropped anywhere on an open chat go to its composer.
    if core.dialog.is_none() && core.t.week.done.is_none() {
        stage = stage
            .on_drop_files(|s: &mut Window, files, _| {
                s.dropping = false;
                attach(s, files.iter().map(|p| Shot::from_file(p)));
            })
            .accepts_files(is_shot_file)
            .on_drop_hover(|s: &mut Window, over, _| s.dropping = over);
    }
    if empty {
        stage = stage.child(dither::slot(dark, s.shore_motion()));
    }
    stage = stage.child(scroll).child(
        col()
            .w_full()
            .items_center()
            .px(24.0)
            .pb(18.0)
            .pt(8.0)
            .child(bottom),
    );
    if let Some(progress) = s.seal.progress()
        && s.shore_motion() != dither::Motion::Still
    {
        stage = stage.child(seal::slot(progress, k.c.text_accent));
    }
    stage
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

fn scoreboard_empty(k: &Kit) -> El {
    let c = &k.c;
    col()
        .key("scoreboard-empty")
        .w_full()
        .gap(10.0)
        .pt(100.0)
        .items_center()
        .child(text("No rated runs yet").font(k.ty.body_medium).color(c.text_base))
        .child(text(format!("Finish a task and end it with {}. Each role gets a table here, and an agent needs {} runs to rank.", cmd("⏎"), scoreboard::TOO_FEW)).font(k.ty.small).color(c.text_muted))
        .child(
            row().pt(8.0).child(
                Button::new("Start a task")
                .variant(ButtonVariant::Outline)
                .shortcut(cmd("1"))
                .show_shortcut()
                .on_press_tx(|s: &mut Window, tx| {
                    s.show(Page::Chat, tx);
                    s.composer.focused = true;
                })
                .build(k),
            ),
        )
}

fn sprite(art: &sprites::Art) -> El {
    el().size(art.width as f32 * art.pt, art.height as f32 * art.pt)
        .pixels(&art.pixels)
}

fn count(n: usize, one: &str, many: &str) -> String {
    let noun = if n == 1 { one } else { many };
    format!("{n} {noun}")
}

/// Every agent on the board once, in the order the tables first list them, with its runs and
/// fan-out wins summed across roles.
fn contenders(board: &Board) -> Vec<(String, String, usize, usize)> {
    let mut out: Vec<(String, String, usize, usize)> = Vec::new();
    for r in board.tables.iter().flat_map(|t| &t.rows) {
        match out.iter_mut().find(|(agent, ..)| *agent == r.agent) {
            Some((_, _, runs, wins)) => {
                *runs += r.runs;
                *wins += r.wins;
            }
            None => out.push((r.agent.clone(), r.name.clone(), r.runs, r.wins)),
        }
    }
    out
}

fn roster(board: &Board, k: &Kit) -> El {
    let c = &k.c;
    let numbers = k.ty.small.features("'tnum' 1");
    let mut strip = row().key("roster").w_full().gap(10.0).flex_wrap();
    for (agent, name, runs, wins) in contenders(board) {
        let stat = if wins > 0 {
            format!(
                "{} · {}",
                count(runs, "run", "runs"),
                count(wins, "win", "wins")
            )
        } else {
            count(runs, "run", "runs")
        };
        strip = strip.child(
            col()
                .key(format!("roster-{agent}"))
                .w(128.0)
                .grow(1.0)
                .gap(2.0)
                .px(10.0)
                .py(12.0)
                .items_center()
                .fill(c.surface_raised)
                .border(1.0, c.border_weak)
                .radius(k.radius.md)
                .child(row().pb(8.0).child(sprite(sprites::model(&agent))))
                .child(
                    text(name)
                        .font(k.ty.ui_medium)
                        .color(c.text_base)
                        .truncate(),
                )
                .child(text(stat).font(numbers).color(c.text_muted)),
        );
    }
    strip
}

/// One tile per role with the agent winning it, so the week's verdict reads before the cards.
fn leaders(board: &Board, k: &Kit) -> El {
    let c = &k.c;
    let numbers = k.ty.small.features("'tnum' 1");
    let mut strip = row().key("leaders").w_full().gap(10.0).flex_wrap();
    for table in &board.tables {
        let lead = table.leader();
        let label = text(table.role.to_uppercase())
            .font(k.ty.small_medium.tracking(0.5))
            .color(c.text_muted)
            .truncate();
        let body = match &lead {
            Some(lead) => {
                let figure = if table.fanout {
                    format!("{}/{} won", lead.row.wins, lead.row.fanouts)
                } else {
                    fmt(lead.row.mean_score(), 2)
                };
                row()
                    .gap(10.0)
                    .center_y()
                    .child(sprite(sprites::model(&lead.row.agent)))
                    .child(
                        col()
                            .gap(2.0)
                            .shrink()
                            .min_w(0.0)
                            .child(
                                text(lead.row.name.clone())
                                    .font(k.ty.ui_medium)
                                    .color(c.text_base)
                                    .truncate(),
                            )
                            .child(
                                row()
                                    .gap(6.0)
                                    .child(text(figure).font(numbers).color(c.text_accent))
                                    .child(
                                        text(table.margin_short(&lead.margin))
                                            .font(numbers)
                                            .color(c.text_muted)
                                            .truncate()
                                            .shrink()
                                            .min_w(0.0),
                                    ),
                            ),
                    )
            }
            None => col()
                .h(36.0)
                .gap(2.0)
                .justify_center()
                .child(
                    text("No leader yet")
                        .font(k.ty.ui_medium)
                        .color(c.text_muted)
                        .truncate(),
                )
                .child(
                    text(format!(
                        "{} to rank",
                        count(scoreboard::TOO_FEW, "run", "runs")
                    ))
                    .font(numbers)
                    .color(c.text_faint)
                    .truncate(),
                ),
        };
        let (fill, border) = if lead.is_some() {
            (k.alpha(c.text_accent, 0.07), k.alpha(c.text_accent, 0.35))
        } else {
            (c.surface_raised, c.border_weak)
        };
        strip = strip.child(
            col()
                .key(format!("lead-{}", table.role))
                .w(150.0)
                .grow(1.0)
                .gap(10.0)
                .px(12.0)
                .py(12.0)
                .fill(fill)
                .border(1.0, border)
                .radius(k.radius.md)
                .child(label)
                .child(body),
        );
    }
    strip
}

/// A strip of tiles under a small caps label.
fn section(label: &str, body: El, k: &Kit) -> El {
    col()
        .w_full()
        .gap(10.0)
        .child(
            text(label.to_owned())
                .font(k.ty.small_medium.tracking(0.5))
                .color(k.c.text_muted),
        )
        .child(body)
}

fn fmt(v: Option<f64>, digits: usize) -> String {
    v.map_or("—".to_owned(), |v| format!("{v:.digits$}"))
}

/// Past this the page stops widening, so a table's figures stay near their names.
const PAGE_MAX_W: f32 = 1040.0;
/// From a table row's top to a figure's line, centring it on the 24-point sprite.
const FIGURE_DROP: f32 = 2.0;
const RANK_COL_W: f32 = 14.0;
/// The contender's sprite and name, before the figure columns start to give way.
const NAME_MIN_W: f32 = 100.0;
const TABLE_GAP: f32 = 12.0;

/// One figure column of a class table: its header, what it means on hover, and its width,
/// which every row shrinks alike towards `min` when the card is narrow, so columns stay lined up.
struct Column {
    label: &'static str,
    help: &'static str,
    width: f32,
    min: f32,
}

impl Column {
    /// The column the class ranks by, wide enough for "tied, cheaper" under the leader's figure.
    fn ranking(label: &'static str, help: &'static str) -> Self {
        Column {
            label,
            help,
            width: 80.0,
            min: 56.0,
        }
    }

    fn figure(label: &'static str, help: &'static str) -> Self {
        Column {
            label,
            help,
            width: 60.0,
            min: 46.0,
        }
    }

    fn cell(&self) -> El {
        col().w(self.width).shrink().min_w(self.min).items_end()
    }
}

/// The figure columns, the ranking one first: mean score, or fan-outs won.
fn columns(fanout: bool) -> Vec<Column> {
    let mut out = if fanout {
        vec![
            Column::ranking("WON", "Fan-outs won, which this class ranks by"),
            Column::figure("SCORE", "Mean score, 1 to 5"),
        ]
    } else {
        vec![Column::ranking(
            "SCORE",
            "Mean score, 1 to 5, which this class ranks by",
        )]
    };
    out.extend([
        Column::figure("RUNS", "Runs dealt in this class"),
        Column::figure("NAILED", "Rated runs scored 5 · nailed it"),
        Column::figure("COST", "Mean cost per run, at list price"),
        Column {
            width: 68.0,
            min: 60.0,
            ..Column::figure(
                "YOUR TIME",
                "Median minutes you spent per run, idle gaps capped at 10",
            )
        },
    ]);
    out
}

/// A figure and the small line under it, if any.
struct Cell {
    figure: String,
    note: Option<String>,
    /// The leader's ranking figure, drawn in the accent.
    lead: bool,
}

fn minutes(v: f64) -> String {
    if v < 0.05 {
        "<0.1 min".to_owned()
    } else {
        format!("{v:.1} min")
    }
}

/// One contender's cells, in the order of `columns`.
fn cells(table: &scoreboard::Table, r: &scoreboard::Row, lead: Option<&str>) -> Vec<Cell> {
    let plain = |figure: String| Cell {
        figure,
        note: None,
        lead: false,
    };
    let ranking = |figure: String| Cell {
        figure,
        note: lead.map(str::to_owned),
        lead: lead.is_some(),
    };
    let score = fmt(r.mean_score(), 2);
    let mut out = Vec::new();
    if table.fanout {
        let won = if r.fanouts > 0 {
            format!("{}/{}", r.wins, r.fanouts)
        } else {
            "—".to_owned()
        };
        out.push(ranking(won));
        out.push(plain(score));
    } else {
        out.push(ranking(score));
    }
    out.push(Cell {
        figure: r.runs.to_string(),
        note: (r.unrated > 0).then(|| format!("{} unrated", r.unrated)),
        lead: false,
    });
    out.push(plain(
        r.nailed_pct()
            .map_or("—".to_owned(), |pct| format!("{pct:.0}%")),
    ));
    out.push(plain(
        r.cost_per_run()
            .map_or("—".to_owned(), |cost| format!("${cost:.3}")),
    ));
    out.push(plain(r.median_your_min().map_or("—".to_owned(), minutes)));
    out
}

/// A table row's frame: the rank, the contender (or its header) growing, then the figures.
fn table_row(rank: El, name: El, figures: El) -> El {
    row()
        .w_full()
        .gap(TABLE_GAP)
        .items_start()
        .px(16.0)
        .child(row().w(RANK_COL_W).child(rank))
        .child(name)
        .child(figures)
}

/// The figure columns' span in a row: their widths and gaps, shrinking alike in every row
/// towards their minimums, so a blocked row's placeholder covers exactly the columns.
fn figure_span(fanout: bool) -> El {
    let columns = columns(fanout);
    let gaps = TABLE_GAP * columns.len().saturating_sub(1) as f32;
    let width = columns.iter().map(|column| column.width).sum::<f32>() + gaps;
    let min = columns.iter().map(|column| column.min).sum::<f32>() + gaps;
    row()
        .w(width)
        .shrink()
        .min_w(min)
        .gap(TABLE_GAP)
        .items_start()
}

fn figures(fanout: bool, cells: impl IntoIterator<Item = El>) -> El {
    let mut span = figure_span(fanout).key("figures");
    for cell in cells {
        span = span.child(cell);
    }
    span
}

/// A blocked row's height: a model sprite's.
const BLOCKED_H: f32 = 24.0;
/// Wider than any table's figure columns, whose span clips the hatch.
const HATCH_SPAN: f32 = 480.0;

/// Diagonal stripes of dither cells, one in four, rising to the right.
fn hatch(color: Color) -> Shape {
    let rows = (BLOCKED_H / DITHER_PT) as usize;
    let cols = (HATCH_SPAN / DITHER_PT) as usize;
    let mut shape = Shape::new();
    for row in 0..rows {
        for col in (0..cols).filter(|col| (col + row) % 4 == 0) {
            let (x, y) = (col as f32 * DITHER_PT, row as f32 * DITHER_PT);
            shape = shape.fill(
                [
                    [x, y],
                    [x + DITHER_PT, y],
                    [x + DITHER_PT, y + DITHER_PT],
                    [x, y + DITHER_PT],
                ],
                color,
            );
        }
    }
    shape
}

/// A contender short of runs: its figures held back behind a hatch until it ranks, with a
/// block per run it needs, filled for those it has.
fn blocked(fanout: bool, runs: usize, k: &Kit) -> El {
    let c = &k.c;
    let needed = scoreboard::TOO_FEW.saturating_sub(runs);
    let mut progress = row().gap(2.0);
    for run in 0..scoreboard::TOO_FEW {
        let done = run < runs;
        progress = progress.child(el().size(2.0 * DITHER_PT, 2.0 * DITHER_PT).fill(if done {
            c.text_muted
        } else {
            c.border_base
        }));
    }
    figure_span(fanout)
        .key("blocked")
        .h(BLOCKED_H)
        .center()
        .radius(k.radius.sm)
        .clip_rounded(k.radius.sm)
        .child(
            el().pin(Some(0.0), Some(0.0), None, None)
                .size(HATCH_SPAN, BLOCKED_H)
                .shape(hatch(c.border_base)),
        )
        .child(
            row()
                .gap(8.0)
                .center_y()
                .px(8.0)
                .h(20.0)
                .shrink()
                .fill(c.surface_raised)
                .radius(k.radius.sm)
                .child(progress)
                .child(
                    text(format!(
                        "Blocked · {} to rank",
                        count(needed, "more run", "more runs")
                    ))
                    .font(k.ty.small)
                    .color(c.text_muted)
                    .truncate()
                    .shrink()
                    .min_w(0.0),
                ),
        )
}

fn table_head(fanout: bool, k: &Kit) -> El {
    let c = &k.c;
    let label = |s: &str| {
        text(s.to_owned())
            .font(k.ty.small_medium.tracking(0.5))
            .color(c.text_muted)
    };
    let cells = columns(fanout)
        .into_iter()
        .map(|column| column.cell().child(label(column.label).help(column.help)));
    table_row(
        el(),
        row()
            .w(0.0)
            .grow(1.0)
            .min_w(NAME_MIN_W)
            .child(label("CONTENDER")),
        figures(fanout, cells),
    )
    .key("head")
    .py(8.0)
    .border_sides([1.0, 0.0, 0.0, 0.0], c.border_weak)
}

fn contender_row(
    table: &scoreboard::Table,
    r: &scoreboard::Row,
    rank: Option<usize>,
    lead: Option<&scoreboard::Margin>,
    k: &Kit,
) -> El {
    let c = &k.c;
    let leads = lead.is_some();
    let (ink, soft) = if rank.is_some() {
        (c.text_base, c.text_muted)
    } else {
        (c.text_faint, c.text_faint)
    };
    let figure_font = k.ty.ui.features("'tnum' 1");
    let note_font = k.ty.small.features("'tnum' 1");
    let margin = lead.map(|margin| table.margin_short(margin));
    let row_cells = columns(table.fanout)
        .into_iter()
        .zip(cells(table, r, margin.as_deref()))
        .map(|(column, cell)| {
            let (font, color) = if cell.lead {
                (k.ty.ui_medium.features("'tnum' 1"), c.text_accent)
            } else {
                (figure_font, ink)
            };
            let mut stack = column
                .cell()
                .pt(FIGURE_DROP)
                .child(text(cell.figure).font(font).color(color));
            if let Some(note) = cell.note {
                stack = stack.child(text(note).font(note_font).color(soft).truncate());
            }
            stack
        });
    let span = if rank.is_some() {
        figures(table.fanout, row_cells)
    } else {
        blocked(table.fanout, r.runs, k)
    };
    let name = row()
        .w(0.0)
        .grow(1.0)
        .min_w(NAME_MIN_W)
        .gap(10.0)
        .center_y()
        .child(
            sprite(sprites::model_small(&r.agent)).opacity(if rank.is_some() { 1.0 } else { 0.45 }),
        )
        .child(
            text(r.name.clone())
                .font(k.ty.ui_medium)
                .color(ink)
                .truncate()
                .help_when_truncated(r.name.clone())
                .shrink()
                .min_w(0.0),
        );
    let position = row().pt(FIGURE_DROP).child(
        text(rank.map_or("–".to_owned(), |rank| rank.to_string()))
            .font(k.ty.ui_medium.features("'tnum' 1"))
            .color(if leads { c.text_accent } else { soft }),
    );
    let mut line = table_row(position, name, span)
        .key(format!("row-{}", r.agent))
        .py(8.0)
        .border_sides([1.0, 0.0, 0.0, 0.0], c.border_weak);
    if leads {
        line = line.fill(k.alpha(c.text_accent, 0.07));
    }
    line
}

const DEALT_BAR_W: f32 = 96.0;
/// One dither cell, the size of a model sprite's pixel.
const DITHER_PT: f32 = 3.0;
/// Two dither rows.
const DEALT_BAR_H: f32 = 2.0 * DITHER_PT;
const DEALT_BAR_GAP: f32 = 2.0;
/// Past the page's own fade, so the decider bar sweeps in where it can be seen.
const BOARD_RISE_AFTER: Duration = Duration::from_millis(150);

/// A decider's stretch of the bar in one colour: Jev solid, then sparser dithers for the
/// picker, `/role` and the fallback, so each reads apart from its neighbour.
fn dealt_segment(decider: usize, width: f32, color: Color) -> Shape {
    let cols = (width / DITHER_PT).ceil() as usize;
    let mut shape = Shape::new();
    for row in 0..2 {
        for col in 0..cols {
            let on = match decider {
                0 => true,
                1 => (row + col) % 2 == 0,
                2 => row == 0 && col % 2 == 0,
                _ => row == 1 && col % 2 == 1,
            };
            if !on {
                continue;
            }
            let x = col as f32 * DITHER_PT;
            let right = (x + DITHER_PT).min(width);
            let top = row as f32 * DITHER_PT;
            shape = shape.fill(
                [
                    [x, top],
                    [right, top],
                    [right, top + DITHER_PT],
                    [x, top + DITHER_PT],
                ],
                color,
            );
        }
    }
    shape
}

/// Who decided this class's deals: a dithered bar, one stretch per decider, held left of its
/// track until `risen` and then sweeping in, beside each share that isn't zero with its
/// decider's icon. The shares wrap under the bar when the card is narrow.
fn dealt_line(decisions: &scoreboard::Decisions, risen: bool, k: &Kit) -> Option<El> {
    let c = &k.c;
    let shares = decisions.shares()?;
    let shown: Vec<_> = shares
        .iter()
        .enumerate()
        .filter(|(_, (.., share))| *share > 0.0)
        .collect();
    let room = DEALT_BAR_W - DEALT_BAR_GAP * shown.len().saturating_sub(1) as f32;
    let mut segments = row()
        .key("fill")
        .gap(DEALT_BAR_GAP)
        .offset(if risen { 0.0 } else { -DEALT_BAR_W }, 0.0)
        .offset_motion(Spring::new(0.45, 0.15));
    for (decider, (source, _, share)) in &shown {
        let width = (room * (*share / 100.0) as f32).max(2.0 * DITHER_PT);
        segments = segments.child(
            el().key(*source)
                .size(width, DEALT_BAR_H)
                .shape(dealt_segment(*decider, width, c.text_muted)),
        );
    }
    let font = k.ty.small.features("'tnum' 1");
    let mut line = row()
        .key("dealt")
        .gaps(4.0, 10.0)
        .flex_wrap()
        .center_y()
        .child(
            el().w(DEALT_BAR_W)
                .h(DEALT_BAR_H)
                .clip_rounded(0.0)
                .child(segments),
        )
        .child(text("Decided by").font(font).color(c.text_muted));
    for (_, (source, label, share)) in &shown {
        line = line.child(
            row()
                .key(format!("decider-{source}"))
                .gap(4.0)
                .center_y()
                .child(sprite(sprites::decider(source)))
                .child(
                    text(format!("{label} {share}%"))
                        .font(font)
                        .color(c.text_muted),
                ),
        );
    }
    Some(line)
}

fn sheet_header(table: &scoreboard::Table, risen: bool, k: &Kit) -> El {
    let c = &k.c;
    let mut title = row().gap(8.0).center_y().child(
        text(table.role.to_uppercase())
            .font(k.ty.body_medium.tracking(0.4))
            .color(c.text_base),
    );
    if table.fanout {
        title = title.child(text("fan-out").font(k.ty.small).color(c.text_faint));
    }
    let mut heading = col().grow(1.0).min_w(0.0).gap(4.0).child(title);
    if !table.criteria.is_empty() {
        heading = heading.child(
            text(table.criteria.clone())
                .font(k.ty.ui)
                .color(c.text_muted)
                .wrap()
                .shrink()
                .min_w(0.0),
        );
    }
    if let Some(dealt) = dealt_line(&table.decisions, risen, k) {
        heading = heading.child(dealt.pt(4.0));
    }
    row()
        .w_full()
        .gap(16.0)
        .items_center()
        .padding(16.0)
        .child(sprite(sprites::role(&table.role)))
        .child(heading)
}

/// One class: who decided its deals, then every contender as a table row, ranked ones first
/// and the leader tinted, with those still short of runs faded below.
fn role_sheet(table: &scoreboard::Table, risen: bool, k: &Kit) -> El {
    let c = &k.c;
    let mut card = col()
        .key(format!("role-{}", table.role))
        .w_full()
        .fill(c.surface_raised)
        .border(1.0, c.border_weak)
        .radius(k.radius.lg)
        .clip_rounded(k.radius.lg)
        .child(sheet_header(table, risen, k));
    if table.rows.is_empty() {
        return card.child(
            col()
                .key("no-runs")
                .w_full()
                .padding(16.0)
                .border_sides([1.0, 0.0, 0.0, 0.0], c.border_weak)
                .child(text("No runs yet.").font(k.ty.small).color(c.text_muted)),
        );
    }
    card = card.child(table_head(table.fanout, k));
    let lead = table.leader();
    let mut rank = 0;
    for r in &table.rows {
        let place = if r.too_few() {
            None
        } else {
            rank += 1;
            Some(rank)
        };
        let margin = lead
            .as_ref()
            .filter(|lead| std::ptr::eq(lead.row, r))
            .map(|lead| &lead.margin);
        card = card.child(contender_row(table, r, place, margin, k));
    }
    card
}

fn scoreboard_page(board: &Board, risen: bool, k: &Kit) -> El {
    if board.runs == 0 {
        return scoreboard_empty(k);
    }
    // One column of class tables, so each figure column lines up down the whole page.
    let mut sheets = col().w_full().gap(16.0);
    for table in &board.tables {
        sheets = sheets.child(role_sheet(table, risen, k));
    }
    let page = col()
        .w_full()
        .max_w(PAGE_MAX_W)
        .self_center()
        .gap(28.0)
        .px(28.0)
        .py(22.0)
        .child(section("LEADING EACH ROLE", leaders(board, k), k))
        .child(section("CONTENDERS", roster(board, k), k))
        .child(sheets);
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
        (Page::Scoreboard, Some(board)) => scoreboard_page(board, s.board_rise_at.is_none(), k),
        _ => chat(s, &core, k, cx.system_dark()),
    };
    let mut overlays = Vec::new();
    let side = sidebar(s, &core, k, &mut overlays);
    folder_picker::build(s, k, &mut overlays);
    viewer(s, k, &mut overlays);
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

fn close_viewer(s: &mut Window) {
    s.viewing = None;
    s.composer.focused = true;
}

/// An image opened from its tile: as large as the window allows, never past its own size, with
/// its format and size and a way to open it in the system's viewer to zoom. Esc, a click on
/// it or a click beside it closes it.
fn viewer(s: &Window, k: &Kit, layer: &mut Vec<El>) {
    let Some(shot) = &s.viewing else {
        return;
    };
    let c = &k.c;
    let image = s.image(shot).filter(|image| image.error().is_none());
    let size = shot.dimensions();
    let picture = match &image {
        Some(image) => {
            let mut picture = el()
                .key("viewer-image")
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .pixels_fit(image.pixels(), rune::SurfaceFit::Fit)
                .label(format!("{} image", shot.kind()))
                .on_press(close_viewer);
            // Never blown up past one image pixel per point.
            if let Some((w, h)) = size {
                picture = picture.max_w(w as f32).max_h(h as f32);
            }
            picture
        }
        None => col().key("viewer-image").padding(24.0).child(
            text(format!("This {} can’t be shown here.", shot.kind()))
                .font(k.ty.ui)
                .color(c.text_muted),
        ),
    };
    let about = match size {
        Some((w, h)) => format!("{} · {w} × {h}", shot.kind()),
        None => shot.kind().to_owned(),
    };
    let open_label = if cfg!(target_os = "macos") {
        "Open in Preview"
    } else {
        "Open"
    };
    let bar = row()
        .key("viewer-bar")
        .gap(12.0)
        .center_y()
        .pl(12.0)
        .pr(4.0)
        .py(4.0)
        .radius(k.radius.lg)
        .fill(c.surface_raised)
        .border(1.0, c.border_weak)
        .child(
            text(about)
                .font(k.ty.small.features("'tnum' 1"))
                .color(c.text_muted),
        )
        .child(
            Button::new(open_label)
                .variant(ButtonVariant::Ghost)
                .control_size(ControlSize::Small)
                .on_press(|s: &mut Window| {
                    let Some(shot) = s.viewing.clone() else {
                        return;
                    };
                    if let Err(error) = shot.open_externally() {
                        lock(&s.core).toast(error, "error");
                    }
                })
                .build(k),
        )
        .child(
            Button::new("Close")
                .variant(ButtonVariant::Ghost)
                .control_size(ControlSize::Small)
                .shortcut("esc")
                .show_shortcut()
                .on_press(close_viewer)
                .build(k),
        );
    let panel = col()
        .key("viewer")
        .role("dialog")
        .label("Image")
        .w_full()
        .h_full()
        .gap(12.0)
        .items_center()
        .justify_center()
        .child(picture)
        .child(bar)
        .on_press(close_viewer)
        .token_motion(k.motion.overlay)
        .modal(true, close_viewer);
    layer.push(
        el().key("viewer-scrim")
            .absolute(0.0, 0.0)
            .w_full()
            .h_full()
            .fill(c.scrim)
            .token_motion(k.motion.overlay)
            .a11y_hidden(),
    );
    layer.push(
        el().key("viewer-layer")
            .absolute(0.0, 0.0)
            .w_full()
            .h_full()
            .padding(40.0)
            .child(panel),
    );
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

#[cfg(test)]
mod tests;
