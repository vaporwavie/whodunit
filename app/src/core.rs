//! State both windows read: the pi process, the open session, dialogs and toasts.
//! The reader thread applies pi records here and wakes the windows; views only read it.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::config::{self, AppConfig};
use crate::pi::{Pi, Spawn};
use crate::sessions::{self, Summary};
use crate::transcript::{END_TITLE, Item, Transcript, str_of};

pub type Shared = Arc<Mutex<Core>>;

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

/// What to do once the unrated-run guard is answered.
#[derive(Clone, Debug)]
pub enum Leave {
    New,
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
    pub context_percent: Option<f64>,
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
    Switch(Option<String>),
    Generic(&'static str),
}

pub struct Core {
    pub config: AppConfig,
    pub cwd: PathBuf,
    pi: Option<Pi>,
    generation: u64,
    next_id: u64,
    pending: HashMap<String, Pending>,
    wakers: Vec<Arc<dyn Fn() + Send + Sync>>,
    pub version: u64,

    pub t: Transcript,
    pub session_file: Option<String>,
    pub streaming: bool,
    pub pending_prompt: Option<String>,
    pub queue: (Vec<String>, Vec<String>),
    pub stats: Stats,
    pub dialog: Option<Dialog>,
    pub leave: Option<Leave>,
    pub after_end: Option<Leave>,
    pub toasts: Vec<Toast>,
    pub statuses: BTreeMap<String, String>,
    pub widgets: BTreeMap<String, Vec<String>>,
    pub editor_text: Option<(u64, String)>,
    pub commands: Vec<Command>,
    pub sessions: Vec<Summary>,
    pub exited: Option<String>,
    pub ready: bool,
}

const END_AFTER_TURN: &str = "End the run when the turn finishes. Esc stops it now.";

pub const APP_COMMANDS: [(&str, &str); 5] = [
    (
        "new",
        "Start a new task (house rule 2: one task per session)",
    ),
    ("compact", "Compact the context"),
    ("name", "Name this session: /name <title>"),
    ("export", "Export this session to HTML"),
    ("scoreboard", "Open the week's scoreboard"),
];

impl Core {
    pub fn new(config: AppConfig, cwd: PathBuf) -> Core {
        Core {
            config,
            cwd,
            pi: None,
            generation: 0,
            next_id: 0,
            pending: HashMap::new(),
            wakers: Vec::new(),
            version: 1,
            t: Transcript::default(),
            session_file: None,
            streaming: false,
            pending_prompt: None,
            queue: (Vec::new(), Vec::new()),
            stats: Stats::default(),
            dialog: None,
            leave: None,
            after_end: None,
            toasts: Vec::new(),
            statuses: BTreeMap::new(),
            widgets: BTreeMap::new(),
            editor_text: None,
            commands: Vec::new(),
            sessions: Vec::new(),
            exited: None,
            ready: false,
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
        if let Some(pending) = pending {
            self.pending.insert(id, pending);
        }
        let sent = self.pi.as_ref().is_some_and(|pi| pi.send(&record));
        if !sent {
            self.toast("pi isn’t running. Restart it from the toolbar.", "error");
        }
        sent
    }

    /// pi writes a session file only after its first assistant message, so a fan-out session
    /// that hasn't been followed up yet is listed from memory.
    pub fn refresh_sessions(&mut self) {
        self.sessions = sessions::list(&config::session_dir(&self.cwd));
        if let (Some(file), Some(title)) = (&self.session_file, self.t.title()) {
            let path = PathBuf::from(file);
            if self.t.week.deal.is_some()
                && self.t.has_answer()
                && !self.sessions.iter().any(|s| s.path == path)
            {
                self.sessions.insert(
                    0,
                    Summary {
                        path,
                        title,
                        week: self.t.week.clone(),
                    },
                );
            }
        }
        self.touch();
    }

    pub fn has_unrated_answer(&self) -> bool {
        self.t.week.unrated() && self.t.has_answer()
    }

    /// Ending opens once the turn has finished with an answer and nothing else is asking.
    pub fn can_end_run(&self) -> bool {
        self.has_unrated_answer()
            && !self.streaming
            && self.dialog.is_none()
            && self.leave.is_none()
    }

    pub fn reveal(&self) -> Option<String> {
        self.t.week.done.as_ref()?;
        let deal = self.t.week.deal.as_ref()?;
        Some(config::agent_name(&deal.agent))
    }

    // Commands the views call.

    pub fn submit(&mut self, text: &str, follow_up: bool) {
        let text = text.trim_end();
        if text.trim().is_empty() {
            return;
        }
        if self.t.week.done.is_some() && !text.starts_with('/') {
            self.toast("This run has ended. ⌘N for the next task.", "info");
            return;
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
            return;
        }
        if let Some(rest) = text.strip_prefix('/') {
            let (name, args) = rest.split_once(' ').unwrap_or((rest, ""));
            match name {
                "new" => return self.request_leave(Leave::New),
                "done" if self.streaming => {
                    self.toast(END_AFTER_TURN, "info");
                    return;
                }
                "compact" => {
                    let mut record = json!({"type": "compact"});
                    if !args.trim().is_empty() {
                        record["customInstructions"] = json!(args.trim());
                    }
                    self.send(record, Some(Pending::Generic("compact")));
                    return;
                }
                "name" => {
                    self.send(
                        json!({"type": "set_session_name", "name": args.trim()}),
                        Some(Pending::Generic("name")),
                    );
                    return;
                }
                "export" => {
                    self.send(
                        json!({"type": "export_html"}),
                        Some(Pending::Generic("export_html")),
                    );
                    return;
                }
                "model" | "models" | "scoped-models" => {
                    self.toast("No model switching this week. House rule 3: a switch counts the run as failed.", "warning");
                    return;
                }
                _ => {}
            }
        }
        if self.pending_prompt.is_some() && self.t.week.deal.is_none() {
            self.toast(
                "Still dealing an agent. Send this after the first answer.",
                "info",
            );
            return;
        }
        let first = self.t.week.deal.is_none() && !self.t.has_answer() && !text.starts_with('/');
        if first {
            self.pending_prompt = Some(text.to_owned());
        }
        let mut record = json!({"type": "prompt", "message": text});
        if self.streaming {
            record["streamingBehavior"] = json!(if follow_up { "followUp" } else { "steer" });
        }
        self.send(record, Some(Pending::Generic("prompt")));
        self.touch();
    }

    pub fn end_run(&mut self) {
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

    /// House rule 4: leaving a session whose run hasn't ended asks first.
    pub fn request_leave(&mut self, leave: Leave) {
        if let Leave::Open(path) = &leave {
            if self.session_file.as_deref() == Some(path.to_string_lossy().as_ref()) {
                return;
            }
        }
        if self.has_unrated_answer() {
            self.leave = Some(leave);
            self.touch();
            return;
        }
        self.do_leave(leave);
    }

    pub fn answer_leave(&mut self, rate_first: Option<bool>) {
        let Some(leave) = self.leave.take() else {
            return;
        };
        match rate_first {
            Some(true) if self.streaming => {
                self.leave = Some(leave);
                self.toast(END_AFTER_TURN, "info");
            }
            Some(true) => {
                self.after_end = Some(leave);
                self.end_run();
            }
            Some(false) => self.do_leave(leave),
            None => {}
        }
        self.touch();
    }

    fn do_leave(&mut self, leave: Leave) {
        match leave {
            Leave::New => {
                self.send(json!({"type": "new_session"}), Some(Pending::Switch(None)));
            }
            Leave::Open(path) => {
                let path = path.to_string_lossy().into_owned();
                if self.session_file.as_deref() == Some(path.as_str()) {
                    return;
                }
                self.send(
                    json!({"type": "switch_session", "sessionPath": path.clone()}),
                    Some(Pending::Switch(Some(path))),
                );
            }
        }
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
            "agent_start" => self.streaming = true,
            "agent_settled" => {
                self.streaming = false;
                self.pending_prompt = None;
                self.send(json!({"type": "get_session_stats"}), Some(Pending::Stats));
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
                        self.t.add_message(message);
                    }
                    _ => {}
                }
            }
            "message_update" => {
                if let Some(update) = record.get("assistantMessageEvent") {
                    self.t.apply_update(update);
                }
            }
            "message_end" => {
                let message = record.get("message").unwrap_or(&Value::Null);
                if message.get("role").and_then(Value::as_str) != Some("user") {
                    self.t.add_message(message);
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
                        if let Some(Item::Fanout { prompt, .. }) = self.t.items.last_mut() {
                            *prompt = self.pending_prompt.take();
                        }
                    } else {
                        self.t.add_entry(entry);
                    }
                    if !had_deal && self.t.week.deal.is_some() {
                        self.pending_prompt = None;
                        self.send(json!({"type": "get_state"}), Some(Pending::State));
                    }
                    if !had_done && self.t.week.done.is_some() {
                        self.send(json!({"type": "get_session_stats"}), Some(Pending::Stats));
                        self.refresh_sessions();
                        if let Some(leave) = self.after_end.take() {
                            self.do_leave(leave);
                        }
                    }
                }
            }
            "session_info_changed" => {
                self.t.name = record
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            }
            "compaction_start" => self
                .statuses
                .insert("compaction".into(), "Compacting context…".into())
                .map_or((), |_| ()),
            "compaction_end" => {
                self.statuses.remove("compaction");
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
            }
            Pending::Stats => {
                self.stats.cost = data.get("cost").and_then(Value::as_f64).unwrap_or(0.0);
                self.stats.context_percent = data
                    .pointer("/contextUsage/percent")
                    .and_then(Value::as_f64);
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
            Pending::Switch(path) => {
                if data
                    .get("cancelled")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                {
                    return;
                }
                self.t = Transcript::default();
                self.stats = Stats::default();
                self.pending_prompt = None;
                self.queue = (Vec::new(), Vec::new());
                self.session_file = path;
                self.load_session();
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
            }
            Pending::Generic("export_html") => {
                let path = str_of(&data, "path");
                self.toast(format!("Exported to {path}"), "info");
            }
            Pending::Generic(_) => {}
        }
        self.touch();
    }

    fn load_session(&mut self) {
        self.send(json!({"type": "get_state"}), Some(Pending::State));
        self.send(json!({"type": "get_entries"}), Some(Pending::Entries));
        self.send(json!({"type": "get_session_stats"}), Some(Pending::Stats));
        self.refresh_sessions();
    }
}

/// Starts (or restarts in another folder) the pi process behind `shared`.
pub fn start_pi(shared: &Shared, cwd: Option<PathBuf>) {
    let login = crate::config::login_env();
    let (spec_bin, cwd, generation) = {
        let mut core = lock(shared);
        if let Some(cwd) = cwd {
            core.config.remember(&cwd);
            core.config.save();
            core.cwd = cwd;
        }
        core.pi = None;
        core.generation += 1;
        core.t = Transcript::default();
        core.session_file = None;
        core.streaming = false;
        core.exited = None;
        core.ready = false;
        core.pending.clear();
        core.dialog = None;
        if !Path::new(&core.config.pi_bin).is_file()
            && let Some(bin) = &login.pi_bin
        {
            core.config.pi_bin = bin.clone();
            core.config.save();
        }
        (
            core.config.pi_bin.clone(),
            core.cwd.clone(),
            core.generation,
        )
    };
    let for_records = shared.clone();
    let for_exit = shared.clone();
    let spawned = Pi::spawn(
        Spawn {
            bin: &spec_bin,
            env: &login.vars,
            cwd: &cwd,
            args: &["--model", "whodunit/auto"],
        },
        move |record| {
            let wakers = {
                let mut core = lock(&for_records);
                if core.generation != generation {
                    return;
                }
                core.apply(record);
                core.wakers.clone()
            };
            wakers.iter().for_each(|wake| wake());
        },
        move |stderr| {
            let wakers = {
                let mut core = lock(&for_exit);
                if core.generation != generation {
                    return;
                }
                core.pi = None;
                core.streaming = false;
                let tail: Vec<&str> = stderr.lines().rev().take(6).collect();
                core.exited = Some(tail.into_iter().rev().collect::<Vec<_>>().join("\n"));
                core.touch();
                core.wakers.clone()
            };
            wakers.iter().for_each(|wake| wake());
        },
    );
    let mut core = lock(shared);
    match spawned {
        Ok(pi) => {
            core.pi = Some(pi);
            core.send(json!({"type": "get_commands"}), Some(Pending::Commands));
            core.load_session();
        }
        Err(error) => {
            core.exited = Some(format!("Couldn’t start {spec_bin}: {error}"));
            core.touch();
        }
    }
}
