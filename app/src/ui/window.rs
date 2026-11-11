//! The app's window: the session sidebar, then a chat (transcript, composer) or the scoreboard
//! page, under the week's rules.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime};

use rune::{
    Chord, ChordKey, ChordMod, Cx, El, Family, ImageCache, Key, KeyEvent, Spring, TextEdit,
    TextStyle, Tx, el, text,
};
use rune_kit::diff::DiffState;
use rune_kit::shell::{self, PaneMotion, PaneState, SidebarToggle, SplitView, Toolbar};
use rune_kit::{ControlSize, Kit};

use crate::core::{Core, DialogKind, Leave, Shared, lock, score_key};
use crate::folders;
use crate::scoreboard::{self, Board};
use crate::sprites;
use crate::transcript::{Block, Item, Shot};
use crate::ui::dither;
use crate::ui::parts::{self, HasCore};
use crate::ui::stage::{self, Stage};
use crate::ui::{develop, seal};
use folder_picker::FolderPicker;

mod board;
mod chat;
mod composer;
pub mod folder_picker;
pub mod rename;
mod sidebar;
mod viewer;

#[cfg(test)]
mod tests;

pub const WORDMARK_FONT: &[u8] = include_bytes!("../../assets/fonts/Fraunces.ttf");
const WORDMARK: TextStyle = TextStyle::new(20.0, 440.0)
    .family(Family::Named("Fraunces"))
    .variations("'SOFT' 100, 'WONK' 1, 'opsz' 144")
    .lh(24.0);
/// Sidebar width past the traffic lights that fits the wordmark and the toggle.
const WORDMARK_ROOM: f32 = 146.0;

#[derive(Clone, Copy, PartialEq)]
pub enum Page {
    Chat,
    Scoreboard,
}

/// What the selected chat's composer and dialog field hold. Each chat keeps its own, set aside in
/// `Window::drafts` while another chat is selected.
#[derive(Default)]
pub struct Draft {
    pub composer: TextEdit,
    /// Images pasted or dropped on the composer, sent with the next prompt.
    pub attachments: Vec<Shot>,
    pub dialog_edit: TextEdit,
    /// The core's last prefilled composer text this draft took.
    editor_generation: u64,
    /// The last dialog this draft has seen, so an input dialog's field is set up once.
    dialog_generation: u64,
}

/// What `catch_up` last saw of the core, to tell what changed since.
#[derive(Default)]
struct Seen {
    version: u64,
    task: u64,
    loads: u64,
    /// Whether the chat was sealed; None before the first look.
    done: Option<bool>,
    /// The selected chat's settled turns, so each new end of turn develops its answer once.
    turns: u64,
    /// Whether the deck showed, so the frame its hold ends rebuilds while nothing animates.
    dealing: bool,
}

pub struct Window {
    pub core: Shared,
    pub draft: Draft,
    /// Decodes prompt images for their tiles; set up with the window's waker.
    images: Option<ImageCache>,
    /// A file drag the composer would take is over the chat.
    dropping: bool,
    /// The image open full size over the window.
    pub viewing: Option<Shot>,
    renaming: Option<rename::Rename>,
    pub sidebar: PaneState,
    ended_collapsed: bool,
    /// History opened only to reveal a cycled-to session; restore it on cycling out.
    ended_cycle_expanded: bool,
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
    preview_sources: HashSet<String>,
    fanout_answers: HashMap<String, usize>,
    /// The working timer's second as last drawn.
    ticked: Option<u64>,
    /// Each sidebar row's last-updated label as last drawn.
    ages: Vec<String>,
    window_focused: bool,
    /// Reduce Motion as the last view saw it, for `poll`, which has no `Cx`.
    reduced_motion: AtomicBool,
    lanes: stage::LaneClock,
    tide: dither::Tide,
    /// Where the tide's slot and the composer's column were laid out, in window points.
    tide_rect: Option<[f32; 4]>,
    dock_rect: Option<[f32; 4]>,
    seal: seal::Sweep,
    develop: develop::Develop,
    /// The slash list's highlighted row, for the composer text it was moved on.
    slash_pick: (String, usize),
    seen: Seen,
    /// The other chats' drafts, by task ID.
    drafts: HashMap<u64, Draft>,
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

    fn preview_sources(&self) -> &HashSet<String> {
        &self.preview_sources
    }

    fn toggle_preview_source(&mut self, id: String) {
        if !self.preview_sources.remove(&id) {
            self.preview_sources.insert(id);
        }
    }

    fn fanout_answer(&self, key: &str) -> Option<usize> {
        self.fanout_answers.get(key).copied()
    }

    fn show_fanout_answer(&mut self, key: String, index: usize) {
        self.fanout_answers.insert(key, index);
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

    fn develop(&self, key: &str) -> Option<f32> {
        self.develop.at(key)
    }
}

impl Window {
    pub fn new(core: Shared) -> Window {
        let mut composer = TextEdit::new("");
        composer.focused = true;
        // A window opened on a working chat finds the tide already in.
        let mut tide = dither::Tide::default();
        {
            let core = lock(&core);
            tide.snap(tide_rises(&core), tide_mood(&core));
        }
        Window {
            core,
            draft: Draft {
                composer,
                ..Draft::default()
            },
            images: None,
            dropping: false,
            viewing: None,
            renaming: None,
            sidebar: PaneState::new(264.0).range(180.0, 480.0),
            ended_collapsed: true,
            ended_cycle_expanded: false,
            sprite_tip: None,
            page: Page::Chat,
            board: None,
            board_rise_at: None,
            picker: FolderPicker::default(),
            diffs: HashMap::new(),
            copied_code: HashMap::new(),
            preview_sources: HashSet::new(),
            fanout_answers: HashMap::new(),
            ticked: None,
            ages: Vec::new(),
            window_focused: false,
            reduced_motion: AtomicBool::new(false),
            lanes: stage::LaneClock::default(),
            tide,
            tide_rect: None,
            dock_rect: None,
            seal: seal::Sweep::default(),
            develop: develop::Develop::default(),
            slash_pick: (String::new(), 0),
            seen: Seen::default(),
            drafts: HashMap::new(),
        }
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
            rename::save(self, tx);
            tx.settle();
        }
        self.page = page;
    }

    /// Shows the chat in this commit, so the next frame is already it. A chat that needs a pi
    /// process gets one from `poll`, off the UI thread, on the frame this asks for.
    pub(crate) fn leave(&mut self, leave: Leave, tx: &mut Tx) {
        rename::save(self, tx);
        self.show(Page::Chat, tx);
        let shared = self.core.clone();
        let mut core = lock(&shared);
        core.request_leave(leave);
        catch_up(self, &core, tx);
        start_soon(&core, tx);
    }

    /// Follow the sidebar's rows across group boundaries, including collapsed history.
    fn cycle_session(&mut self, forward: bool, tx: &mut Tx) {
        let path = {
            let core = lock(&self.core);
            let sessions: Vec<_> = sidebar::groups(&core)
                .flat_map(|(_, _, sessions)| sessions)
                .collect();
            let count = sessions.len();
            if count == 0 {
                return;
            }
            let current = core.path();
            let index = sessions
                .iter()
                .position(|s| Some(&s.path) == current.as_ref());
            let next = match (index, forward) {
                (Some(i), true) => (i + 1) % count,
                (Some(i), false) => (i + count - 1) % count,
                (None, true) => 0,
                (None, false) => count - 1,
            };
            let summary = sessions[next];
            if core.session_state(summary) == crate::core::SessionState::Ended {
                if self.ended_collapsed {
                    self.ended_cycle_expanded = true;
                    self.ended_collapsed = false;
                }
            } else if self.ended_cycle_expanded {
                self.ended_collapsed = true;
                self.ended_cycle_expanded = false;
            }
            summary.path.clone()
        };
        let key = format!("session-{}", path.display());
        self.leave(Leave::Open(path), tx);
        tx.scroll_into_view(key);
    }

    fn open_scoreboard(&mut self, tx: &mut Tx) {
        self.board = Some(scoreboard::compute());
        self.show(Page::Scoreboard, tx);
        self.board_rise_at = Some(Instant::now() + board::RISE_AFTER);
    }
}

/// Keys a dialog takes before the composer types them.
fn dialog_keys_tx(core: &mut Core, ev: &KeyEvent, empty_field: bool, tx: &mut Tx) -> bool {
    let fanout = parts::fanout_choice(core).is_some();
    let handled = dialog_keys(core, ev, empty_field);
    if fanout && handled {
        tx.settle();
    }
    handled
}

pub fn dialog_keys(core: &mut Core, ev: &KeyEvent, empty_field: bool) -> bool {
    if ev.mods.cmd {
        return false;
    }
    let Some(dialog) = core.dialog.as_ref() else {
        return false;
    };
    let select = matches!(dialog.kind, DialogKind::Select(_));
    let confirm = matches!(dialog.kind, DialogKind::Confirm(_));
    if let Some(parts::FanoutChoice { answers, index, .. }) = parts::fanout_choice(core) {
        let count = answers.len();
        let next = match &ev.key {
            Key::Left | Key::Up => (index + count - 1) % count,
            Key::Right | Key::Down => (index + 1) % count,
            Key::Enter => {
                core.pick(index);
                return true;
            }
            Key::Text(t) if empty_field => {
                let by_label = answers.iter().position(|a| a.label.eq_ignore_ascii_case(t));
                let by_number = t
                    .parse::<usize>()
                    .ok()
                    .filter(|n| (1..=9).contains(n) && *n <= count)
                    .map(|n| n - 1);
                let Some(next) = by_label.or(by_number) else {
                    return false;
                };
                next
            }
            Key::Escape => {
                core.answer_dialog(None);
                return true;
            }
            _ => return false,
        };
        core.choose(next);
        return true;
    }
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

pub fn on_key(s: &mut Window, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if s.renaming.is_some() && rename::on_key(s, ev, tx) {
        return true;
    }
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
    if dialog_keys_tx(&mut lock(&s.core), ev, true, tx) {
        return true;
    }
    if shell::is_sidebar_toggle(ev) {
        s.sidebar.toggle();
        return true;
    }
    for (key, forward) in [('[', false), (']', true)] {
        if Chord::new(ChordKey::Char(key))
            .with(ChordMod::Primary)
            .with(ChordMod::Shift)
            .matches_tx(ev, tx)
        {
            s.cycle_session(forward, tx);
            return true;
        }
    }
    if ev.is_command('n') {
        s.leave(Leave::New, tx);
        s.draft.composer.focused = true;
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
    if ev.key == Key::Escape && lock(&s.core).can_stop() {
        composer::stop(s, tx);
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
            s.draft.dialog_edit.focused = true;
        }
    } else if core.t.week.done.is_none() {
        s.draft.composer.focused = true;
    }
}

/// Brings the window's own state up to the core: the selected chat's drafts, diffs and
/// prefilled text. Another chat, or this one reloaded from pi, lands whole in one frame: nothing
/// grows, reveals or springs over from what was on screen. Returns whether any of it changed.
fn catch_up(s: &mut Window, core: &Core, tx: &mut Tx) -> bool {
    let mut changed = core.version != s.seen.version;
    s.seen.version = core.version;
    let switched = core.task_id() != s.seen.task;
    if switched {
        let draft = s.drafts.remove(&core.task_id()).unwrap_or_default();
        let previous = std::mem::replace(&mut s.draft, draft);
        s.drafts.insert(s.seen.task, previous);
        s.seen.task = core.task_id();
        s.viewing = None;
        s.renaming = None;
        s.draft.composer.focused = true;
        s.diffs.clear();
        tx.scroll_to_bottom_instant("transcript");
        changed = true;
    }
    let reloaded = core.loads != s.seen.loads;
    s.seen.loads = core.loads;
    let done = core.t.week.done.is_some();
    if switched || reloaded {
        s.copied_code.clear();
        s.preview_sources.clear();
        s.fanout_answers.clear();
        s.seal.clear();
        s.develop.clear();
        s.tide.snap(tide_rises(core), tide_mood(core));
        tx.settle();
    } else if done && s.seen.done == Some(false) && s.page == Page::Chat {
        s.seal.start(Instant::now());
        tx.settle();
    }
    if let Some(choice) = parts::fanout_choice(core) {
        // Keep the answer on screen while pi records the pick and returns to the transcript.
        s.fanout_answers
            .insert(format!("item-{}", choice.item_index), choice.index);
    }
    s.seen.done = Some(done);
    if !switched
        && !reloaded
        && core.turns != s.seen.turns
        && s.page == Page::Chat
        && let Some(index) = answer_to_develop(core)
    {
        s.develop.start(Instant::now(), format!("item-{index}"));
    }
    s.seen.turns = core.turns;
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
        .filter(|(g, _)| *g != s.draft.editor_generation)
    {
        s.draft.editor_generation = generation;
        s.draft.composer.replace_text(text);
        changed = true;
    }
    if let Some(dialog) = core
        .dialog
        .as_ref()
        .filter(|d| d.generation != s.draft.dialog_generation)
    {
        s.draft.dialog_generation = dialog.generation;
        if let DialogKind::Input { prefill, .. } = &dialog.kind {
            s.draft.dialog_edit = TextEdit::new(prefill.clone());
            s.draft.dialog_edit.focused = true;
        }
    }
    changed
}

/// The answer a settled turn ends on: the transcript's last item, when it's the agent's and
/// has text. A fan-out, a shell command or a failed reply gets no pass.
fn answer_to_develop(core: &Core) -> Option<usize> {
    let index = core.t.items.len().checked_sub(1)?;
    let Item::Assistant { blocks, error, .. } = &core.t.items[index] else {
        return None;
    };
    let text = blocks
        .iter()
        .flatten()
        .any(|block| matches!(block, Block::Text(body) if !body.is_empty()));
    (error.is_none() && text).then_some(index)
}

/// A chat selected without a pi process gets one from `poll`, off the UI thread. Ask for that
/// frame, since a switch leaves nothing animating to bring it.
fn start_soon(core: &Core, tx: &mut Tx) {
    if !core.unstarted.is_empty() {
        tx.wake_after(Duration::ZERO);
    }
}

pub fn poll(s: &mut Window, t: f64, tx: &mut Tx) -> bool {
    let renamed = rename::poll(s, tx);
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
    let mut changed = renamed || s.copied_code.len() != before;
    changed |= s.picker.poll();
    changed |= catch_up(s, &core, tx);
    let sweeping = s.seal.progress().is_some() || s.develop.progress().is_some();
    s.seal
        .tick(Instant::now(), s.shore_motion(), s.page == Page::Chat);
    s.develop
        .tick(Instant::now(), s.shore_motion(), s.page == Page::Chat);
    changed |= sweeping;
    if (s.seal.progress().is_some() || s.develop.progress().is_some())
        && s.shore_motion() == dither::Motion::Live
    {
        tx.wake_after(dither::FRAME);
    }
    let second = core.turn_started.map(|started| started.elapsed().as_secs());
    if second != s.ticked {
        s.ticked = second;
        changed = true;
    }
    let wall = SystemTime::now();
    let ages: Vec<String> = core
        .sessions
        .iter()
        .map(|summary| folders::ago(summary.updated, wall))
        .collect();
    if ages != s.ages {
        s.ages = ages;
        changed = true;
    }
    if let Some(next) = core
        .sessions
        .iter()
        .map(|summary| folders::ago_lasts(summary.updated, wall))
        .min()
    {
        tx.wake_after(next + Duration::from_millis(10));
    }
    if let Some(started) = core.turn_started {
        let into = started.elapsed().subsec_millis() as u64;
        tx.wake_after(Duration::from_millis(1000 - into));
    }
    let start = !core.unstarted.is_empty();
    let at_work = Stage::of(&core);
    let dealing = matches!(at_work, Some(Stage::Dealing { .. }));
    changed |= dealing != s.seen.dealing;
    s.seen.dealing = dealing;
    if let Some(left) = core
        .dealing_since
        .filter(|_| dealing)
        .and_then(|since| stage::DEAL_HOLD.checked_sub(since.elapsed()))
    {
        tx.wake_after(left);
    }
    s.lanes.track(at_work.as_ref());
    let tide = (s.tide.level(), s.tide.shade());
    s.tide.tick(
        t,
        s.page == Page::Chat && tide_rises(&core),
        tide_mood(&core),
        live_thought(&core),
        s.shore_motion(),
    );
    changed |= (s.tide.level(), s.tide.shade()) != tide;
    // A troubled chat can hold its tide in while idle, and the water still moves.
    let live = s.page == Page::Chat
        && (chat::blank(&core) || at_work.is_some() || s.tide.level() > 0.0)
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

/// Whether the chat's tide should be in: its agent works or thinks, or trouble holds it in red,
/// outside a fan-out's pick. A stopped pi doesn't hold it, or the water would run for good.
fn tide_rises(core: &Core) -> bool {
    !chat::blank(core)
        && parts::fanout_choice(core).is_none()
        && (Stage::of(core).is_some_and(|stage| stage.tide())
            || (core.troubled() && core.exited.is_none() && core.t.week.done.is_none()))
}

/// The live message's thought while it's the last thing it streams: the tide's net fires on it.
/// Only its length and a hash of it reach the tide; none of it shows.
fn live_thought(core: &Core) -> Option<dither::Thought<'_>> {
    let Some(Item::Assistant {
        blocks,
        streaming: true,
        ..
    }) = core.t.items.last()
    else {
        return None;
    };
    let (index, Some(Block::Thinking(text))) = blocks
        .iter()
        .enumerate()
        .rev()
        .find(|(_, block)| block.is_some())?
    else {
        return None;
    };
    core.streaming.then_some(dither::Thought {
        key: core.t.items.len() << 8 | index,
        text,
    })
}

/// The tide's colour: red while the run is in trouble, haze while the live message thinks.
fn tide_mood(core: &Core) -> dither::Mood {
    if core.troubled() {
        dither::Mood::Trouble
    } else if live_thought(core).is_some() {
        dither::Mood::Thinking
    } else {
        dither::Mood::Working
    }
}

fn toolbar(s: &Window, core: &Core, k: &Kit) -> Toolbar {
    let title = match s.page {
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
    let title = if s.page == Page::Chat {
        rename::title(s, title, k)
    } else {
        text(title)
            .key("title")
            .font(k.ty.ui_medium)
            .color(k.c.text_base)
            .truncate()
    };
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
    let side = sidebar::build(s, &core, k, &mut overlays);
    folder_picker::build(s, k, &mut overlays);
    viewer::build(s, k, &mut overlays);
    let mut window =
        SplitView::new(main).sidebar(&s.sidebar, |s: &mut Window| &mut s.sidebar, side);
    if s.sidebar.width() - cx.titlebar_inset() >= WORDMARK_ROOM {
        window = window.sidebar_title(wordmark(k, cx.titlebar_inset() > 0.0));
    }
    let window = window
        .toolbar(toolbar(s, &core, k))
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
        s.develop
            .tick(Instant::now(), s.shore_motion(), s.page == Page::Chat);
        s.window_focused = focused;
        if !focused {
            folder_picker::release_cmd(s, tx);
        }
    })
}

/// The working chat with its tide `level` of the way in (before easing), without a pi process.
pub fn scene_tide(s: &mut Window, level: f32) {
    crate::fixtures::scene_working(&s.core);
    s.tide.hold(level, dither::Mood::Working);
}

/// The chat thinking, its tide in and hazed, its net firing on a burst of the thought.
pub fn scene_thinking(s: &mut Window) {
    crate::fixtures::scene_thinking(&s.core);
    s.tide.hold(1.0, dither::Mood::Thinking);
    let pulse = dither::Pulse {
        spike: 0.8,
        activity: 0.5,
        seed: 412,
    };
    s.tide.hold_thought(0.3, pulse);
}

/// The chat deep in a long thought: the haze heated, the net busy.
pub fn scene_thinking_deep(s: &mut Window) {
    crate::fixtures::scene_thinking(&s.core);
    s.tide.hold(1.0, dither::Mood::Thinking);
    let pulse = dither::Pulse {
        spike: 1.0,
        activity: 0.9,
        seed: 87,
    };
    s.tide.hold_thought(1.0, pulse);
}

/// The chat retrying after a provider error, its tide in red.
pub fn scene_trouble(s: &mut Window) {
    crate::fixtures::scene_trouble(&s.core);
    s.tide.hold(1.0, dither::Mood::Trouble);
}

/// A fixed halfway frame of the pass that develops an answer at the end of its turn.
pub fn scene_developing(s: &mut Window) {
    crate::fixtures::scene_chat(&s.core);
    let core = lock(&s.core);
    s.seen.task = core.task_id();
    s.seen.loads = core.loads;
    s.seen.turns = core.turns;
    let index = answer_to_develop(&core).expect("the chat ends on an answer");
    let now = Instant::now();
    s.develop.start(now, format!("item-{index}"));
    s.develop
        .tick(now + Duration::from_millis(300), dither::Motion::Live, true);
}

/// A fixed halfway frame for snapshots of the sealing sweep.
pub fn scene_sealing(s: &mut Window) {
    crate::fixtures::scene_reveal(&s.core);
    let core = lock(&s.core);
    s.seen.task = core.task_id();
    s.seen.loads = core.loads;
    s.seen.done = Some(true);
    let now = Instant::now();
    s.seal.start(now);
    s.seal
        .tick(now + Duration::from_millis(325), dither::Motion::Live, true);
}

pub fn setup(s: &mut Window, waker: rune::Waker) {
    s.images = Some(ImageCache::new(waker.clone()));
    lock(&s.core).add_waker(Arc::new(move || waker()));
}
