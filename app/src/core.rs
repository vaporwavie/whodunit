//! Shared app state and one independent pi process per live chat.
//! Reader threads route records to their own task, even while another chat is selected.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::config::{self, AppConfig};
use crate::pi::{Pi, Spawn};
use crate::sessions::{self, Summary};
use crate::transcript::{END_TITLE, Item, Shot, Transcript, str_of};

#[cfg(test)]
mod tests;

pub type Shared = Arc<Mutex<Core>>;

/// A primary-modifier shortcut as this platform labels it: ⌘N on macOS, Ctrl+N elsewhere.
pub fn cmd(key: &str) -> String {
    if cfg!(target_os = "macos") {
        format!("⌘{key}")
    } else {
        format!("Ctrl+{key}")
    }
}

pub fn lock(shared: &Shared) -> MutexGuard<'_, Core> {
    shared
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[derive(Clone, Debug)]
pub enum DialogKind {
    Select(Vec<String>),
    Confirm(String),
    Input {
        placeholder: String,
        prefill: String,
        multiline: bool,
    },
}

/// A pi extension dialog (`extension_ui_request`), answered with `extension_ui_response`.
#[derive(Clone, Debug)]
pub struct Dialog {
    pub id: String,
    pub title: String,
    pub kind: DialogKind,
    pub highlight: Option<usize>,
    pub generation: u64,
}

impl Dialog {
    /// Ending a run can't be undone and reveals the agent, so its select needs ⏎ to commit.
    pub fn ends_run(&self) -> bool {
        self.title == END_TITLE
    }
}

/// The digit an end-run option answers to: `4 · nailed it, rough edges` → `("4", "nailed it, rough edges")`.
pub fn score_key(option: &str) -> Option<(&str, &str)> {
    let (key, label) = option.split_once(" · ")?;
    let digit = key.len() == 1 && key.chars().all(|c| c.is_ascii_digit());
    if !digit {
        return None;
    }
    Some((key, label))
}

/// Navigate without interrupting work in the current chat.
#[derive(Clone, Debug)]
pub enum Leave {
    New,
    /// A new task in another folder.
    NewIn(PathBuf),
    Open(PathBuf),
}

#[derive(Clone, Debug)]
pub struct Toast {
    pub text: String,
    pub level: String,
    pub until: Instant,
}

#[derive(Clone, Debug, Default)]
pub struct Stats {
    pub cost: f64,
    /// Tokens in the context pi sends, counted as pi counts them for compaction.
    pub context_tokens: Option<f64>,
    pub context_window: Option<f64>,
    /// `context_tokens` is the last compaction's estimate of what it left. pi has no count of
    /// its own until a response after the compaction reports usage.
    pub predicted: bool,
    /// The free tokens below which pi compacts on its own; `None` with auto-compaction off.
    pub reserve: Option<f64>,
}

impl Stats {
    /// Context used, in percent of where pi compacts on its own, so 100% is the next compaction.
    /// With auto-compaction off, in percent of the whole window.
    pub fn context_percent(&self) -> Option<f64> {
        let tokens = self.context_tokens?;
        let window = self.context_window.filter(|w| *w > 0.0)?;
        let limit = self
            .reserve
            .map(|reserve| window - reserve)
            .filter(|limit| *limit > 0.0)
            .unwrap_or(window);
        Some(tokens / limit * 100.0)
    }
}

#[derive(Clone, Debug)]
pub struct Command {
    pub name: String,
    pub description: String,
}

enum Pending {
    State,
    Entries,
    Stats,
    Commands,
    Generic(&'static str),
}

#[derive(Default)]
pub struct Task {
    pub cwd: PathBuf,
    pi: Option<Pi>,
    /// Records sent before the chat's process started, for it to read first.
    outbox: Vec<Value>,
    generation: u64,
    pending: HashMap<String, Pending>,
    pub t: Transcript,
    pub session_file: Option<String>,
    pub streaming: bool,
    /// When the agent started its current turn, for the working timer.
    pub turn_started: Option<Instant>,
    /// The first prompt stays visible through the deal until its message or fan-out arrives.
    pub pending_prompt: Option<String>,
    /// The images sent with `pending_prompt`, shown with it until pi echoes the message.
    pub pending_shots: Vec<Shot>,
    pub queue: (Vec<String>, Vec<String>),
    pub stats: Stats,
    pub dialog: Option<Dialog>,
    pub toasts: Vec<Toast>,
    pub statuses: BTreeMap<String, String>,
    pub widgets: BTreeMap<String, Vec<String>>,
    pub editor_text: Option<(u64, String)>,
    pub commands: Vec<Command>,
    pub exited: Option<String>,
    pub ready: bool,
    /// Bumped each time pi's entries replace the transcript, which the window lands at once.
    pub loads: u64,
}

impl Task {
    /// Records waiting for the chat's process to start, for tests that never start one.
    #[cfg(test)]
    pub(crate) fn sent_before_start(&self) -> &[Value] {
        &self.outbox
    }

    pub fn working(&self) -> bool {
        self.exited.is_none()
            && self.t.week.done.is_none()
            && self.dialog.is_none()
            && (self.streaming
                || self.pending_prompt.is_some()
                || !self.widgets.is_empty()
                || !self.statuses.is_empty()
                || self.pending.values().any(|p| {
                    matches!(
                        p,
                        Pending::Generic("prompt")
                            | Pending::Generic("bash")
                            | Pending::Generic("compact")
                    )
                }))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionState {
    Working,
    Idle,
    Ended,
}

pub struct Core {
    pub config: AppConfig,
    pub sessions: Vec<Summary>,
    pub version: u64,
    task: Task,
    tasks: HashMap<u64, Task>,
    next_generation: u64,
    next_id: u64,
    wakers: Vec<Arc<dyn Fn() + Send + Sync>>,
    /// Chats selected before their pi process exists. The window starts them off its UI
    /// thread with `start_unstarted`.
    pub unstarted: Vec<u64>,
}

impl std::ops::Deref for Core {
    type Target = Task;

    fn deref(&self) -> &Task {
        &self.task
    }
}

impl std::ops::DerefMut for Core {
    fn deref_mut(&mut self) -> &mut Task {
        &mut self.task
    }
}

const END_AFTER_TURN: &str = "End the run when the turn finishes. Esc stops it now.";

pub const APP_COMMANDS: [(&str, &str); 5] = [
    ("new", "Start another task; other chats keep working"),
    ("compact", "Compact the context"),
    ("name", "Name this session: /name <title>"),
    ("export", "Export this session to HTML"),
    ("scoreboard", "Open the week's scoreboard"),
];

impl Core {
    pub fn new(config: AppConfig, cwd: PathBuf) -> Core {
        Core {
            config,
            task: Task {
                cwd,
                ..Task::default()
            },
            tasks: HashMap::new(),
            next_generation: 0,
            next_id: 0,
            wakers: Vec::new(),
            version: 1,
            sessions: Vec::new(),
            unstarted: Vec::new(),
        }
    }

    pub fn add_waker(&mut self, waker: Arc<dyn Fn() + Send + Sync>) {
        self.wakers.push(waker);
    }

    pub fn wakers(&self) -> Vec<Arc<dyn Fn() + Send + Sync>> {
        self.wakers.clone()
    }

    fn touch(&mut self) {
        self.version += 1;
    }

    pub fn toast(&mut self, text: impl Into<String>, level: &str) {
        let text = text.into();
        let seconds = if level == "error" { 10 } else { 6 };
        self.toasts.retain(|t| t.text != text);
        self.toasts.push(Toast {
            text,
            level: level.to_owned(),
            until: Instant::now() + Duration::from_secs(seconds),
        });
        self.touch();
    }

    /// Drops expired toasts; returns the time until the next one expires.
    pub fn expire_toasts(&mut self) -> Option<Duration> {
        let now = Instant::now();
        let before = self.toasts.len();
        self.toasts.retain(|t| t.until > now);
        if self.toasts.len() != before {
            self.touch();
        }
        self.toasts.iter().map(|t| t.until - now).min()
    }

    fn send(&mut self, mut record: Value, pending: Option<Pending>) -> bool {
        self.next_id += 1;
        let id = format!("app-{}", self.next_id);
        record["id"] = json!(id);
        let sent = match &self.pi {
            Some(pi) => pi.send(&record),
            // Shown before its process started: it reads these once it has.
            None if self.exited.is_none() => {
                self.outbox.push(record);
                true
            }
            None => false,
        };
        if sent {
            if let Some(pending) = pending {
                self.pending.insert(id, pending);
            }
        } else {
            self.pending_prompt = None;
            self.toast(
                "pi isn’t running in this chat. Start a new task or reopen the app to resume it.",
                "error",
            );
        }
        sent
    }

    /// pi writes a session file only after its first assistant message, so a fan-out session
    /// that hasn't been followed up yet is listed from memory.
    pub fn refresh_sessions(&mut self) {
        self.sessions = sessions::list(&config::sessions_root());
        self.sync_live_sessions();
        self.touch();
    }

    fn sync_live_sessions(&mut self) {
        for task in self.tasks.values().chain(std::iter::once(&self.task)) {
            let Some(file) = &task.session_file else {
                continue;
            };
            let Some(title) = task.t.title().or_else(|| task.pending_prompt.clone()) else {
                continue;
            };
            let summary = Summary {
                path: PathBuf::from(file),
                cwd: task.cwd.clone(),
                title,
                week: task.t.week.clone(),
            };
            if let Some(existing) = self.sessions.iter_mut().find(|s| s.path == summary.path) {
                *existing = summary;
            } else {
                self.sessions.insert(0, summary);
            }
        }
    }

    pub fn task_id(&self) -> u64 {
        self.task.generation
    }

    /// The live chat, selected or not, that has `path` open.
    fn live_task(&self, path: &Path) -> Option<&Task> {
        self.tasks
            .values()
            .chain(std::iter::once(&self.task))
            .find(|t| {
                t.session_file
                    .as_deref()
                    .is_some_and(|f| Path::new(f) == path)
            })
    }

    pub fn session_state(&self, summary: &Summary) -> SessionState {
        let task = self.live_task(&summary.path);
        let week = task.map_or(&summary.week, |t| &t.t.week);
        if week.done.is_some() {
            SessionState::Ended
        } else if task.is_some_and(Task::working) {
            SessionState::Working
        } else {
            SessionState::Idle
        }
    }

    pub fn tasks_waiting(&self, summary: &Summary) -> bool {
        self.live_task(&summary.path)
            .is_some_and(|task| task.dialog.is_some())
    }

    fn park_task(&mut self, task: Task) {
        if task.generation != 0
            && ((!task.ready && task.session_file.is_some())
                || task.working()
                || task.t.week.deal.is_some()
                || !task.t.items.is_empty()
                || task.t.title().is_some()
                || task.dialog.is_some())
        {
            self.tasks.insert(task.generation, task);
        }
    }

    /// Temporarily selects a task under the shared lock to reuse the same record handlers.
    /// The visible task is restored before any window can read the state.
    fn for_task(&mut self, generation: u64, f: impl FnOnce(&mut Core)) {
        if self.generation == generation {
            f(self);
        } else if let Some(mut task) = self.tasks.remove(&generation) {
            std::mem::swap(&mut self.task, &mut task);
            let visible = task.generation;
            self.tasks.insert(visible, task);
            f(self);
            let mut task = self.tasks.remove(&visible).expect("visible task");
            std::mem::swap(&mut self.task, &mut task);
            self.tasks.insert(generation, task);
        }
        self.sync_live_sessions();
    }

    pub fn has_unrated_answer(&self) -> bool {
        self.t.week.unrated() && self.t.has_answer()
    }

    /// Ending opens once the turn has finished with an answer and nothing else is asking.
    pub fn can_end_run(&self) -> bool {
        self.has_unrated_answer() && !self.streaming && self.dialog.is_none()
    }

    pub fn reveal(&self) -> Option<String> {
        self.t.week.done.as_ref()?;
        let deal = self.t.week.deal.as_ref()?;
        Some(config::agent_name(&deal.agent))
    }

    // Commands the views call.

    pub fn submit(&mut self, text: &str, follow_up: bool) {
        self.submit_with(text, follow_up, &[]);
    }

    /// `submit` with images attached. Returns whether they went out with a prompt: slash and
    /// shell commands don't take images, and a refused prompt sends nothing, so the composer
    /// keeps them for the next try.
    pub fn submit_with(&mut self, text: &str, follow_up: bool, shots: &[Shot]) -> bool {
        let text = text.trim_end();
        if text.trim().is_empty() && shots.is_empty() {
            return false;
        }
        if self.t.week.done.is_some() && !text.starts_with('/') {
            self.toast(
                format!("This run has ended. {} for the next task.", cmd("N")),
                "info",
            );
            return false;
        }
        if let Some(command) = text.strip_prefix('!') {
            let exclude = command.starts_with('!');
            let command = command.trim_start_matches('!').trim().to_owned();
            self.t.items.push(Item::Bash {
                command: command.clone(),
                output: String::new(),
                exit_code: None,
                running: true,
            });
            self.send(
                json!({"type": "bash", "command": command, "excludeFromContext": exclude}),
                Some(Pending::Generic("bash")),
            );
            self.touch();
            return false;
        }
        if let Some(rest) = text.strip_prefix('/') {
            let (name, args) = rest.split_once(' ').unwrap_or((rest, ""));
            match name {
                "new" => {
                    self.request_leave(Leave::New);
                    return false;
                }
                "done" if self.streaming => {
                    self.toast(END_AFTER_TURN, "info");
                    return false;
                }
                "compact" => {
                    let mut record = json!({"type": "compact"});
                    if !args.trim().is_empty() {
                        record["customInstructions"] = json!(args.trim());
                    }
                    self.send(record, Some(Pending::Generic("compact")));
                    return false;
                }
                "name" => {
                    self.send(
                        json!({"type": "set_session_name", "name": args.trim()}),
                        Some(Pending::Generic("name")),
                    );
                    return false;
                }
                "export" => {
                    self.send(
                        json!({"type": "export_html"}),
                        Some(Pending::Generic("export_html")),
                    );
                    return false;
                }
                "model" | "models" | "scoped-models" => {
                    self.toast("No model switching this week. House rule 3: a switch counts the run as failed.", "warning");
                    return false;
                }
                _ => {}
            }
        }
        if self.pending_prompt.is_some() && self.t.week.deal.is_none() {
            self.toast(
                "Still dealing an agent. Send this after the first answer.",
                "info",
            );
            return false;
        }
        // An extension command such as `/done` ignores images; they stay in the composer.
        let shots = if text.starts_with('/') { &[] } else { shots };
        let first = self.t.week.deal.is_none() && !self.t.has_answer() && !text.starts_with('/');
        if first {
            self.pending_prompt = Some(text.to_owned());
            self.pending_shots = shots.to_vec();
        }
        let mut record = json!({"type": "prompt", "message": text});
        if !shots.is_empty() {
            record["images"] = shots.iter().map(Shot::to_block).collect();
        }
        if self.streaming {
            record["streamingBehavior"] = json!(if follow_up { "followUp" } else { "steer" });
        }
        let sent = self.send(record, Some(Pending::Generic("prompt")));
        self.sync_live_sessions();
        self.touch();
        sent && !shots.is_empty()
    }

    pub fn end_run(&mut self) {
        if self.dialog.is_some() || self.t.week.done.is_some() {
            return;
        }
        if self.t.week.deal.is_none() {
            self.toast("Nothing to end yet. Send the task first.", "info");
            return;
        }
        if self.streaming {
            self.toast(END_AFTER_TURN, "info");
            return;
        }
        self.send(
            json!({"type": "prompt", "message": "/done"}),
            Some(Pending::Generic("prompt")),
        );
    }

    /// Esc while working: take back the queue, then abort.
    pub fn abort(&mut self) {
        if !self.streaming {
            return;
        }
        self.send(
            json!({"type": "clear_queue"}),
            Some(Pending::Generic("clear_queue")),
        );
        self.send(json!({"type": "abort"}), Some(Pending::Generic("abort")));
    }

    /// Selects the chat now, so the window draws it in the frame that asked: a live chat as it
    /// is, a saved one from its file, a new one empty. One without a pi process yet waits in
    /// `unstarted`.
    pub fn request_leave(&mut self, leave: Leave) {
        let (cwd, session) = match leave {
            Leave::New => (self.cwd.clone(), None),
            Leave::NewIn(cwd) => (cwd, None),
            Leave::Open(path) => {
                if let Some(generation) = self.live_task(&path).map(|t| t.generation) {
                    if let Some(task) = self.tasks.remove(&generation) {
                        let previous = std::mem::replace(&mut self.task, task);
                        self.park_task(previous);
                        self.touch();
                    }
                    return;
                }
                let cwd = self
                    .sessions
                    .iter()
                    .find(|s| s.path == path)
                    .map_or_else(|| self.cwd.clone(), |s| s.cwd.clone());
                (cwd, Some(path))
            }
        };
        let t = session
            .as_deref()
            .and_then(sessions::transcript)
            .unwrap_or_default();
        let generation = self.open_task(cwd, session.as_deref(), t);
        self.unstarted.push(generation);
        self.touch();
    }

    pub fn answer_dialog(&mut self, answer: Option<Value>) {
        let Some(dialog) = self.dialog.take() else {
            return;
        };
        let mut record = json!({"type": "extension_ui_response", "id": dialog.id});
        match (answer, &dialog.kind) {
            (None, _) => record["cancelled"] = json!(true),
            (Some(Value::Bool(yes)), DialogKind::Confirm(_)) => record["confirmed"] = json!(yes),
            (Some(value), _) => record["value"] = value,
        }
        if let Some(pi) = &self.pi {
            pi.send(&record);
        }
        self.touch();
    }

    /// Commits the highlighted option; an end-run select with nothing chosen yet stays open.
    pub fn commit(&mut self) {
        let Some(index) = self.dialog.as_ref().and_then(|d| d.highlight) else {
            return;
        };
        self.pick(index);
    }

    pub fn choose(&mut self, index: usize) {
        let Some(dialog) = self.dialog.as_mut() else {
            return;
        };
        dialog.highlight = Some(index);
        self.touch();
    }

    pub fn pick(&mut self, index: usize) {
        let Some(DialogKind::Select(options)) = self.dialog.as_ref().map(|d| d.kind.clone()) else {
            return;
        };
        let Some(option) = options.get(index) else {
            return;
        };
        self.answer_dialog(Some(json!(option)));
    }

    pub fn move_highlight(&mut self, delta: i64) {
        let Some(dialog) = self.dialog.as_mut() else {
            return;
        };
        let DialogKind::Select(options) = &dialog.kind else {
            return;
        };
        let n = options.len().max(1) as i64;
        let next = match dialog.highlight {
            Some(index) => (index as i64 + delta).rem_euclid(n),
            None if delta > 0 => 0,
            None => n - 1,
        };
        dialog.highlight = Some(next as usize);
        self.touch();
    }

    // Records from pi.

    fn apply(&mut self, record: Value) {
        let kind = str_of(&record, "type");
        match kind.as_str() {
            "response" => self.on_response(&record),
            "extension_ui_request" => self.on_ui_request(&record),
            "agent_start" => {
                self.streaming = true;
                self.turn_started = Some(Instant::now());
            }
            "agent_settled" => {
                self.streaming = false;
                self.turn_started = None;
                self.pending_prompt = None;
                self.refresh_stats();
                self.refresh_sessions();
                if self.session_file.is_none() {
                    self.send(json!({"type": "get_state"}), Some(Pending::State));
                }
            }
            "message_start" => {
                let message = record.get("message").unwrap_or(&Value::Null);
                match message.get("role").and_then(Value::as_str) {
                    Some("assistant") => self.t.items.push(Item::Assistant {
                        blocks: Vec::new(),
                        streaming: true,
                        error: None,
                    }),
                    Some("user") => {
                        self.pending_prompt = None;
                        self.pending_shots.clear();
                        self.t.add_message(message);
                        self.refresh_stats();
                    }
                    _ => {}
                }
            }
            // The response's usage, then its tool results: each moves the context pi checks
            // before it asks the model again.
            "turn_end" => self.refresh_stats(),
            "message_update" => {
                if let Some(update) = record.get("assistantMessageEvent") {
                    self.t.apply_update(update);
                }
            }
            "message_end" => {
                let message = record.get("message").unwrap_or(&Value::Null);
                match message.get("role").and_then(Value::as_str) {
                    Some("user") => {}
                    Some("assistant") => {
                        self.t.add_message(message);
                        self.refresh_stats();
                    }
                    _ => self.t.add_message(message),
                }
            }
            "tool_execution_start" | "tool_execution_update" | "tool_execution_end" => {
                let run = self
                    .t
                    .tools
                    .entry(str_of(&record, "toolCallId"))
                    .or_default();
                run.running = kind != "tool_execution_end";
                let result = record.get("result").or_else(|| record.get("partialResult"));
                if let Some(result) = result {
                    run.output = crate::transcript::tool_output(result);
                    run.patch = crate::transcript::result_patch(result);
                }
                run.is_error = record
                    .get("isError")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
            }
            "bash_execution_update" => {
                if let Some(Item::Bash {
                    output,
                    running: true,
                    ..
                }) = self
                    .t
                    .items
                    .iter_mut()
                    .rev()
                    .find(|i| matches!(i, Item::Bash { .. }))
                {
                    output.push_str(
                        record
                            .get("delta")
                            .and_then(Value::as_str)
                            .unwrap_or_default(),
                    );
                }
            }
            "queue_update" => {
                let list = |key: &str| -> Vec<String> {
                    record
                        .get(key)
                        .and_then(Value::as_array)
                        .map(|a| {
                            a.iter()
                                .map(|v| v.as_str().unwrap_or_default().to_owned())
                                .collect()
                        })
                        .unwrap_or_default()
                };
                self.queue = (list("steering"), list("followUp"));
            }
            "entry_appended" => {
                if let Some(entry) = record.get("entry") {
                    let had_done = self.t.week.done.is_some();
                    let had_deal = self.t.week.deal.is_some();
                    if str_of(entry, "customType") == crate::transcript::FANOUT_VIEW {
                        self.t.add_entry(entry);
                        if let Some(Item::Fanout { prompt, shots, .. }) =
                            self.task.t.items.last_mut()
                        {
                            *prompt = self.task.pending_prompt.take();
                            *shots = std::mem::take(&mut self.task.pending_shots);
                        }
                    } else {
                        self.t.add_entry(entry);
                    }
                    if !had_deal && self.t.week.deal.is_some() {
                        self.send(json!({"type": "get_state"}), Some(Pending::State));
                    }
                    if !had_done && self.t.week.done.is_some() {
                        self.refresh_stats();
                        self.refresh_sessions();
                    }
                }
            }
            "session_info_changed" => {
                self.t.name = record
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                self.refresh_sessions();
            }
            "compaction_start" => self
                .statuses
                .insert("compaction".into(), "Compacting context…".into())
                .map_or((), |_| ()),
            "compaction_end" => {
                self.statuses.remove("compaction");
                // pi counts nothing until the next response, so the meter shows the
                // compaction's own estimate of what it kept rather than the old size.
                if let Some(after) = record
                    .pointer("/result/estimatedTokensAfter")
                    .and_then(Value::as_f64)
                {
                    self.stats.context_tokens = Some(after);
                    self.stats.predicted = true;
                }
                self.refresh_stats();
                if record.get("errorMessage").and_then(Value::as_str).is_some() {
                    self.toast("Compaction failed.", "error");
                } else if !record
                    .get("aborted")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                {
                    self.t
                        .items
                        .push(Item::Notice("Context compacted".to_owned()));
                }
            }
            "auto_retry_start" => {
                let attempt = record.get("attempt").and_then(Value::as_i64).unwrap_or(1);
                self.statuses.insert(
                    "retry".into(),
                    format!(
                        "Retrying ({attempt}) after: {}",
                        str_of(&record, "errorMessage")
                    ),
                );
            }
            "auto_retry_end" => {
                self.statuses.remove("retry");
                if !record
                    .get("success")
                    .and_then(Value::as_bool)
                    .unwrap_or(true)
                {
                    self.toast(
                        "Gave up retrying a provider error. Details after /done.",
                        "error",
                    );
                }
            }
            "extension_error" => self.toast(
                format!("Extension error: {}", str_of(&record, "error")),
                "error",
            ),
            _ => return,
        }
        self.touch();
    }

    fn on_ui_request(&mut self, record: &Value) {
        let id = str_of(record, "id");
        let title = str_of(record, "title");
        let kind = match str_of(record, "method").as_str() {
            "select" => {
                let options = record
                    .get("options")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .map(|v| v.as_str().unwrap_or_default().to_owned())
                            .collect()
                    })
                    .unwrap_or_default();
                DialogKind::Select(options)
            }
            "confirm" => DialogKind::Confirm(str_of(record, "message")),
            "input" => DialogKind::Input {
                placeholder: str_of(record, "placeholder"),
                prefill: String::new(),
                multiline: false,
            },
            "editor" => DialogKind::Input {
                placeholder: String::new(),
                prefill: str_of(record, "prefill"),
                multiline: true,
            },
            "notify" => {
                let level = record
                    .get("notifyType")
                    .and_then(Value::as_str)
                    .unwrap_or("info")
                    .to_owned();
                return self.toast(str_of(record, "message"), &level);
            }
            "setStatus" => {
                let key = str_of(record, "statusKey");
                match record.get("statusText").and_then(Value::as_str) {
                    Some(text) => self.statuses.insert(key, text.to_owned()),
                    None => self.statuses.remove(&key),
                };
                return;
            }
            "setWidget" => {
                let key = str_of(record, "widgetKey");
                match record.get("widgetLines").and_then(Value::as_array) {
                    Some(lines) => self.widgets.insert(
                        key,
                        lines
                            .iter()
                            .map(|l| l.as_str().unwrap_or_default().to_owned())
                            .collect(),
                    ),
                    None => self.widgets.remove(&key),
                };
                return;
            }
            "set_editor_text" => {
                let generation = self.editor_text.as_ref().map_or(1, |(g, _)| g + 1);
                self.editor_text = Some((generation, str_of(record, "text")));
                return;
            }
            _ => return,
        };
        let mut dialog = Dialog {
            id,
            title,
            kind,
            highlight: Some(0),
            generation: self.version,
        };
        if dialog.ends_run() {
            dialog.highlight = None;
        }
        self.dialog = Some(dialog);
    }

    fn on_response(&mut self, record: &Value) {
        let id = str_of(record, "id");
        let Some(pending) = self.pending.remove(&id) else {
            return;
        };
        let ok = record
            .get("success")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let data = record.get("data").cloned().unwrap_or(Value::Null);
        if !ok {
            let error = str_of(record, "error");
            if let Pending::Generic("prompt") = pending {
                self.pending_prompt = None;
            }
            self.toast(format!("{}: {error}", str_of(record, "command")), "error");
            return;
        }
        match pending {
            Pending::State => {
                self.session_file = data
                    .get("sessionFile")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                self.streaming = data
                    .get("isStreaming")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                if !self.streaming {
                    self.turn_started = None;
                }
                self.ready = true;
                self.refresh_sessions();
            }
            Pending::Entries => {
                let entries = data
                    .get("entries")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                let leaf = data.get("leafId").and_then(Value::as_str);
                self.t = Transcript::from_entries(&entries, leaf);
                self.loads += 1;
            }
            Pending::Stats => {
                self.stats.cost = data.get("cost").and_then(Value::as_f64).unwrap_or(0.0);
                let usage = data.get("contextUsage");
                let number = |key: &str| usage.and_then(|u| u.get(key)).and_then(Value::as_f64);
                self.stats.context_window = number("contextWindow");
                match number("tokens") {
                    Some(tokens) => {
                        self.stats.context_tokens = Some(tokens);
                        self.stats.predicted = false;
                    }
                    // Unknown since a compaction: its estimate stands until a response counts.
                    None if self.stats.predicted && usage.is_some() => {}
                    None => {
                        self.stats.context_tokens = None;
                        self.stats.predicted = false;
                    }
                }
                self.stats.reserve = config::compaction_reserve(&self.cwd);
            }
            Pending::Commands => {
                let list = data
                    .get("commands")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                self.commands = list
                    .iter()
                    .filter(|c| !str_of(c, "name").starts_with("aw-"))
                    .map(|c| Command {
                        name: str_of(c, "name"),
                        description: str_of(c, "description"),
                    })
                    .collect();
            }
            Pending::Generic("clear_queue") => {
                let mut restored: Vec<String> = Vec::new();
                for key in ["steering", "followUp"] {
                    if let Some(list) = data.get(key).and_then(Value::as_array) {
                        restored.extend(list.iter().filter_map(Value::as_str).map(str::to_owned));
                    }
                }
                if !restored.is_empty() {
                    let generation = self.editor_text.as_ref().map_or(1, |(g, _)| g + 1);
                    self.editor_text = Some((generation, restored.join("\n")));
                }
            }
            Pending::Generic("bash") => {
                if let Some(Item::Bash {
                    output,
                    exit_code,
                    running,
                    ..
                }) = self
                    .t
                    .items
                    .iter_mut()
                    .rev()
                    .find(|i| matches!(i, Item::Bash { running: true, .. }))
                {
                    *output = str_of(&data, "output");
                    *exit_code = data.get("exitCode").and_then(Value::as_i64);
                    *running = false;
                }
                // A `!` command's output joins the context.
                self.refresh_stats();
            }
            Pending::Generic("export_html") => {
                let path = str_of(&data, "path");
                self.toast(format!("Exported to {path}"), "info");
            }
            Pending::Generic(_) => {}
        }
        self.touch();
    }

    /// Selects a new chat in `cwd` and parks the current one. A saved chat brings its transcript,
    /// so it shows whole, in its own sidebar group, before its pi process has loaded it.
    pub(crate) fn open_task(&mut self, cwd: PathBuf, session: Option<&Path>, t: Transcript) -> u64 {
        self.next_generation += 1;
        let task = Task {
            cwd,
            generation: self.next_generation,
            session_file: session.map(|path| path.to_string_lossy().into_owned()),
            t,
            ..Task::default()
        };
        let previous = std::mem::replace(&mut self.task, task);
        self.park_task(previous);
        self.next_generation
    }

    fn load_session(&mut self) {
        self.send(json!({"type": "get_state"}), Some(Pending::State));
        self.send(json!({"type": "get_entries"}), Some(Pending::Entries));
        self.refresh_stats();
        self.refresh_sessions();
    }

    /// Asks pi for cost and context, which it counts exactly as its compaction check does.
    /// Called whenever the context grows or shrinks, so the meter never waits for the turn's end.
    fn refresh_stats(&mut self) {
        self.send(json!({"type": "get_session_stats"}), Some(Pending::Stats));
    }
}

/// Starts the pi process of each chat selected without one. The first start waits on the login
/// shell, so the window calls this off its UI thread.
pub fn start_unstarted(shared: &Shared) {
    let unstarted = std::mem::take(&mut lock(shared).unstarted);
    for generation in unstarted {
        start_pi(shared, generation);
    }
}

/// Starts an independent pi process for a chat `request_leave` selected, wherever it is now.
fn start_pi(shared: &Shared, generation: u64) {
    let login = crate::config::login_env();
    let mut core = lock(shared);
    // A new task left before its process started is gone, and needs none.
    let Some((cwd, session)) = core
        .tasks
        .values()
        .chain(std::iter::once(&core.task))
        .find(|t| t.generation == generation)
        .map(|t| (t.cwd.clone(), t.session_file.clone()))
    else {
        return;
    };
    core.config.remember(&cwd);
    core.config.save();
    if !Path::new(&core.config.pi_bin).is_file()
        && let Some(bin) = &login.pi_bin
    {
        core.config.pi_bin = bin.clone();
        core.config.save();
    }
    let spec_bin = core.config.pi_bin.clone();
    let for_records = Arc::downgrade(shared);
    let for_exit = Arc::downgrade(shared);
    let mut args = vec!["--model", "whodunit/auto"];
    if let Some(path) = &session {
        args.extend(["--session", path.as_str()]);
    }
    let spawned = Pi::spawn(
        Spawn {
            bin: &spec_bin,
            env: &login.vars,
            cwd: &cwd,
            args: &args,
        },
        move |record| {
            let Some(shared) = for_records.upgrade() else {
                return;
            };
            let wakers = {
                let mut core = lock(&shared);
                core.for_task(generation, |core| core.apply(record));
                core.wakers()
            };
            wakers.iter().for_each(|wake| wake());
        },
        move |stderr| {
            let Some(shared) = for_exit.upgrade() else {
                return;
            };
            let wakers = {
                let mut core = lock(&shared);
                core.for_task(generation, |core| {
                    core.pi = None;
                    core.streaming = false;
                    core.turn_started = None;
                    core.pending_prompt = None;
                    let tail: Vec<&str> = stderr.lines().rev().take(6).collect();
                    core.exited = Some(tail.into_iter().rev().collect::<Vec<_>>().join("\n"));
                    core.touch();
                });
                core.wakers()
            };
            wakers.iter().for_each(|wake| wake());
        },
    );
    core.for_task(generation, |core| match spawned {
        Ok(pi) => {
            core.pi = Some(pi);
            core.send(json!({"type": "get_commands"}), Some(Pending::Commands));
            core.load_session();
            let outbox = std::mem::take(&mut core.outbox);
            if let Some(pi) = &core.pi {
                outbox.iter().for_each(|record| {
                    pi.send(record);
                });
            }
        }
        Err(error) => {
            core.outbox.clear();
            core.pending_prompt = None;
            core.exited = Some(format!("Couldn’t start {spec_bin}: {error}"));
        }
    });
    core.touch();
    let wakers = core.wakers();
    drop(core);
    wakers.iter().for_each(|wake| wake());
}
