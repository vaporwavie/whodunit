//! The conversation as the app draws it, built from pi session entries and live events.
//! Model and provider fields are never copied in: the dealt agent stays hidden until `/done`.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde_json::{Value, json};

pub const DEAL: &str = "whodunit.deal";
pub const DONE: &str = "whodunit.done";
pub const SWITCH: &str = "whodunit.switch";
pub const FANOUT_VIEW: &str = "whodunit.fanout-view";
pub const FANOUT_PICK: &str = "whodunit.fanout-pick";
/// Title of `/done`'s select; its options read `5 · nailed it, great` down to `1 · failed`.
pub const END_TITLE: &str = "End this run";

/// The largest image the composer takes. pi scales images down before a model sees them, so
/// this only keeps a stray file from flooding the pipe.
pub const MAX_SHOT_BYTES: usize = 20 * 1024 * 1024;
/// File types the composer takes, as pi passes them to models.
pub const SHOT_EXTENSIONS: [&str; 5] = ["png", "jpg", "jpeg", "gif", "webp"];

/// An image attached to a prompt: its bytes, decoded once from pi's base64.
#[derive(Clone, PartialEq)]
pub struct Shot {
    pub mime: &'static str,
    pub bytes: Arc<[u8]>,
}

impl std::fmt::Debug for Shot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Shot({}, {} bytes)", self.mime, self.bytes.len())
    }
}

impl Shot {
    /// PNG, JPEG, GIF or WebP, told by its first bytes rather than a name.
    pub fn from_bytes(bytes: impl Into<Arc<[u8]>>) -> Option<Shot> {
        let bytes = bytes.into();
        let mime = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            "image/png"
        } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
            "image/jpeg"
        } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
            "image/gif"
        } else if bytes.len() > 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
            "image/webp"
        } else {
            return None;
        };
        Some(Shot { mime, bytes })
    }

    /// A dropped or pasted file, or why it can't go with a prompt.
    pub fn from_file(path: &Path) -> Result<Shot, String> {
        let name = path.file_name().map_or_else(
            || path.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        let size = std::fs::metadata(path)
            .map_err(|e| format!("{name}: {e}"))?
            .len();
        if size > MAX_SHOT_BYTES as u64 {
            return Err(format!(
                "{name} is over {} MB.",
                MAX_SHOT_BYTES / 1024 / 1024
            ));
        }
        let bytes = std::fs::read(path).map_err(|e| format!("{name}: {e}"))?;
        Shot::from_bytes(bytes)
            .ok_or_else(|| format!("{name} isn’t a PNG, JPEG, GIF or WebP image."))
    }

    /// An `image` content block, as pi stores and sends them.
    pub fn from_block(block: &Value) -> Option<Shot> {
        if block.get("type").and_then(Value::as_str) != Some("image") {
            return None;
        }
        let data = block.get("data").and_then(Value::as_str)?;
        Shot::from_bytes(BASE64.decode(data).ok()?)
    }

    /// pi's `ImageContent`.
    pub fn to_block(&self) -> Value {
        json!({"type": "image", "data": BASE64.encode(&self.bytes), "mimeType": self.mime})
    }

    /// Width and height in pixels, read from the file's header, so a view can size the image
    /// before it decodes.
    pub fn dimensions(&self) -> Option<(u32, u32)> {
        let b = &self.bytes[..];
        let be16 = |at: usize| Some(u16::from_be_bytes(b.get(at..at + 2)?.try_into().ok()?) as u32);
        let le16 = |at: usize| Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?) as u32);
        let le24 = |at: usize| {
            let x = b.get(at..at + 3)?;
            Some(x[0] as u32 | (x[1] as u32) << 8 | (x[2] as u32) << 16)
        };
        match self.mime {
            "image/png" => {
                let be32 =
                    |at: usize| Some(u32::from_be_bytes(b.get(at..at + 4)?.try_into().ok()?));
                Some((be32(16)?, be32(20)?))
            }
            "image/gif" => Some((le16(6)?, le16(8)?)),
            "image/jpeg" => {
                // Walk the markers to the first start-of-frame, which holds the size.
                let mut at = 2;
                loop {
                    if *b.get(at)? != 0xFF {
                        return None;
                    }
                    let marker = *b.get(at + 1)?;
                    if matches!(marker, 0xC0..=0xCF) && !matches!(marker, 0xC4 | 0xC8 | 0xCC) {
                        return Some((be16(at + 7)?, be16(at + 5)?));
                    }
                    at += match marker {
                        0xFF => 1,
                        0x01 | 0xD0..=0xD8 => 2,
                        _ => 2 + be16(at + 2)? as usize,
                    };
                }
            }
            _ => match b.get(12..16)? {
                b"VP8X" => Some((1 + le24(24)?, 1 + le24(27)?)),
                b"VP8L" => {
                    let x = b.get(21..25)?;
                    let w = 1 + ((x[1] as u32 & 0x3F) << 8 | x[0] as u32);
                    let h =
                        1 + ((x[3] as u32 & 0x0F) << 10 | (x[2] as u32) << 2 | (x[1] as u32) >> 6);
                    Some((w, h))
                }
                b"VP8 " => Some((le16(26)? & 0x3FFF, le16(28)? & 0x3FFF)),
                _ => None,
            },
        }
    }

    /// The file extension for its format.
    pub fn extension(&self) -> &'static str {
        match self.mime {
            "image/png" => "png",
            "image/jpeg" => "jpg",
            "image/gif" => "gif",
            _ => "webp",
        }
    }

    /// Writes the image to a temporary file, named for its content, and opens it in the
    /// system's viewer (Preview on macOS).
    pub fn open_externally(&self) -> Result<(), String> {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.bytes.hash(&mut hasher);
        let path = std::env::temp_dir().join(format!(
            "whodunit-image-{:016x}.{}",
            hasher.finish(),
            self.extension()
        ));
        std::fs::write(&path, &self.bytes).map_err(|e| format!("Couldn’t save the image: {e}"))?;
        let opener = if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        std::process::Command::new(opener)
            .arg(&path)
            .spawn()
            .map(drop)
            .map_err(|e| format!("Couldn’t open the image: {e}"))
    }

    /// The format, for a tile that can't show the picture itself.
    pub fn kind(&self) -> &'static str {
        match self.mime {
            "image/png" => "PNG",
            "image/jpeg" => "JPEG",
            "image/gif" => "GIF",
            _ => "WebP",
        }
    }
}

/// The images in a `content` array; none for a plain string.
pub fn shots_of(content: &Value) -> Vec<Shot> {
    content
        .as_array()
        .map(|blocks| blocks.iter().filter_map(Shot::from_block).collect())
        .unwrap_or_default()
}

#[derive(Clone, Debug)]
pub enum Block {
    Text(String),
    Thinking(String),
    Tool {
        id: String,
        name: String,
        args: String,
        /// What a `write` call puts in its file, for its diff.
        written: Option<String>,
    },
}

#[derive(Clone, Debug)]
pub struct FanAnswer {
    pub label: String,
    pub ok: bool,
    pub error: Option<String>,
    pub text: String,
}

#[derive(Clone, Debug)]
pub enum Item {
    User {
        text: String,
        shots: Vec<Shot>,
    },
    Assistant {
        blocks: Vec<Option<Block>>,
        streaming: bool,
        error: Option<String>,
    },
    Bash {
        command: String,
        output: String,
        exit_code: Option<i64>,
        running: bool,
    },
    Fanout {
        prompt: Option<String>,
        /// Images that went with the prompt to every contender.
        shots: Vec<Shot>,
        answers: Vec<FanAnswer>,
        picked: Option<String>,
    },
    Notice(String),
}

impl Item {
    /// A prompt with no images.
    pub fn user(text: impl Into<String>) -> Item {
        Item::User {
            text: text.into(),
            shots: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct ToolRun {
    pub output: String,
    pub is_error: bool,
    pub running: bool,
    /// The unified patch an `edit` reports in its result's details.
    pub patch: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct Deal {
    pub role: String,
    pub agent: String,
    pub size: String,
}

#[derive(Clone, Debug, Default)]
pub struct Done {
    pub score: i64,
}

#[derive(Clone, Debug, Default)]
pub struct Week {
    pub deal: Option<Deal>,
    pub done: Option<Done>,
    pub switched: bool,
}

impl Week {
    pub fn unrated(&self) -> bool {
        self.deal.is_some() && self.done.is_none()
    }

    /// Applies one `whodunit.*` custom entry; false for any other entry type.
    pub fn apply_custom(&mut self, custom_type: &str, data: &Value) -> bool {
        match custom_type {
            DEAL => {
                self.deal = Some(Deal {
                    role: str_of(data, "role"),
                    agent: str_of(data, "agent"),
                    size: str_of(data, "size"),
                });
            }
            DONE => {
                self.done = Some(Done {
                    score: data.get("score").and_then(Value::as_i64).unwrap_or(0),
                });
            }
            SWITCH => self.switched = true,
            _ => return false,
        }
        true
    }
}

pub fn str_of(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

/// Joined text of a `content` that is a string or an array of text blocks.
pub fn content_text(content: &Value) -> String {
    if let Some(text) = content.as_str() {
        return text.to_owned();
    }
    let Some(blocks) = content.as_array() else {
        return String::new();
    };
    blocks
        .iter()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|block| block.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("")
}

pub fn args_summary(args: &Value) -> String {
    let Some(map) = args.as_object() else {
        return args.to_string();
    };
    for key in ["command", "path", "file_path", "pattern", "query", "url"] {
        if let Some(value) = map.get(key).and_then(Value::as_str) {
            return value.lines().next().unwrap_or_default().to_owned();
        }
    }
    let text = args.to_string();
    if text.len() > 160 {
        format!("{}…", &text[..text.floor_char_boundary(160)])
    } else {
        text
    }
}

fn tool_block(id: String, name: String, arguments: &Value) -> Block {
    let written = (name == "write")
        .then(|| arguments.get("content").and_then(Value::as_str))
        .flatten()
        .map(str::to_owned);
    Block::Tool {
        id,
        name,
        args: args_summary(arguments),
        written,
    }
}

/// The patch in a tool result's `details`, which only `edit` sends.
pub fn result_patch(result: &Value) -> Option<String> {
    result
        .pointer("/details/patch")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

pub fn blocks_of(message: &Value) -> Vec<Option<Block>> {
    let Some(content) = message.get("content").and_then(Value::as_array) else {
        return vec![Some(Block::Text(content_text(
            message.get("content").unwrap_or(&Value::Null),
        )))];
    };
    content
        .iter()
        .map(|block| match block.get("type").and_then(Value::as_str) {
            Some("text") => Some(Block::Text(str_of(block, "text"))),
            Some("thinking") => Some(Block::Thinking(str_of(block, "thinking"))),
            Some("toolCall") => Some(tool_block(
                str_of(block, "id"),
                str_of(block, "name"),
                block.get("arguments").unwrap_or(&Value::Null),
            )),
            _ => None,
        })
        .collect()
}

pub fn tool_output(result: &Value) -> String {
    content_text(result.get("content").unwrap_or(result))
}

/// The fan-out prompt from the extension's context message: "User request:\n\n<prompt>\n\nYour answer:".
fn fanout_prompt(content: &str) -> Option<String> {
    let rest = content.strip_prefix("User request:\n\n")?;
    let end = rest.find("\n\nYour answer:")?;
    Some(rest[..end].to_owned())
}

#[derive(Default)]
pub struct Transcript {
    pub items: Vec<Item>,
    pub tools: HashMap<String, ToolRun>,
    pub week: Week,
    pub name: Option<String>,
    pub first_prompt: Option<String>,
}

impl Transcript {
    /// Rebuilds from `get_entries`: walks the active branch from `leaf` to the root.
    pub fn from_entries(entries: &[Value], leaf: Option<&str>) -> Transcript {
        let by_id: HashMap<&str, &Value> = entries
            .iter()
            .filter_map(|e| e.get("id").and_then(Value::as_str).map(|id| (id, e)))
            .collect();
        let mut branch = Vec::new();
        let mut cursor = leaf.and_then(|id| by_id.get(id).copied());
        while let Some(entry) = cursor {
            branch.push(entry);
            cursor = entry
                .get("parentId")
                .and_then(Value::as_str)
                .and_then(|id| by_id.get(id).copied());
        }
        branch.reverse();
        let mut t = Transcript::default();
        for entry in branch {
            t.add_entry(entry);
        }
        t.name = entries
            .iter()
            .rev()
            .find(|e| e.get("type").and_then(Value::as_str) == Some("session_info"))
            .and_then(|e| e.get("name").and_then(Value::as_str))
            .map(str::to_owned);
        t
    }

    pub fn add_entry(&mut self, entry: &Value) {
        match entry.get("type").and_then(Value::as_str) {
            Some("message") => {
                if let Some(message) = entry.get("message") {
                    self.add_message(message);
                }
            }
            Some("custom") => {
                let custom_type = str_of(entry, "customType");
                let data = entry.get("data").cloned().unwrap_or(Value::Null);
                self.add_custom(&custom_type, &data);
            }
            Some("custom_message") => {
                let custom_type = str_of(entry, "customType");
                self.add_custom_message(
                    &custom_type,
                    entry.get("content").unwrap_or(&Value::Null),
                    entry
                        .get("display")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                );
            }
            Some("compaction") => self
                .items
                .push(Item::Notice("Context compacted".to_owned())),
            Some("branch_summary") => self
                .items
                .push(Item::Notice("Branch summarized".to_owned())),
            _ => {}
        }
    }

    pub fn add_custom(&mut self, custom_type: &str, data: &Value) {
        if self.week.apply_custom(custom_type, data) {
            return;
        }
        match custom_type {
            FANOUT_VIEW => {
                let answers = data
                    .get("answers")
                    .and_then(Value::as_array)
                    .map(|list| {
                        list.iter()
                            .map(|a| FanAnswer {
                                label: str_of(a, "label"),
                                ok: a.get("ok").and_then(Value::as_bool).unwrap_or(false),
                                error: a.get("error").and_then(Value::as_str).map(str::to_owned),
                                text: str_of(a, "text"),
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                self.items.push(Item::Fanout {
                    prompt: None,
                    shots: Vec::new(),
                    answers,
                    picked: None,
                });
            }
            FANOUT_PICK => {
                let label = str_of(data, "label");
                if let Some(Item::Fanout { picked, .. }) = self
                    .items
                    .iter_mut()
                    .rev()
                    .find(|i| matches!(i, Item::Fanout { .. }))
                {
                    *picked = Some(label);
                }
            }
            _ => {}
        }
    }

    pub fn add_custom_message(&mut self, custom_type: &str, content: &Value, display: bool) {
        let text = content_text(content);
        if custom_type == FANOUT_PICK {
            let prompt = fanout_prompt(&text);
            if let Some(Item::Fanout {
                prompt: slot,
                shots,
                ..
            }) = self
                .items
                .iter_mut()
                .rev()
                .find(|i| matches!(i, Item::Fanout { .. }))
            {
                if slot.is_none() {
                    *slot = prompt.clone();
                }
                if shots.is_empty() {
                    *shots = shots_of(content);
                }
            }
            if self.first_prompt.is_none() {
                self.first_prompt = prompt;
            }
            return;
        }
        if display && !text.is_empty() {
            self.items.push(Item::Notice(text));
        }
    }

    /// Adds a finished message (from an entry or `message_end`).
    pub fn add_message(&mut self, message: &Value) {
        match message.get("role").and_then(Value::as_str) {
            Some("user") => {
                let content = message.get("content").unwrap_or(&Value::Null);
                let text = content_text(content);
                if self.first_prompt.is_none() {
                    self.first_prompt = Some(text.clone());
                }
                self.items.push(Item::User {
                    text,
                    shots: shots_of(content),
                });
            }
            Some("assistant") => {
                let error = (str_of(message, "stopReason") == "error")
                    .then(|| str_of(message, "errorMessage"));
                let item = Item::Assistant {
                    blocks: blocks_of(message),
                    streaming: false,
                    error,
                };
                match self.items.last_mut() {
                    Some(
                        last @ Item::Assistant {
                            streaming: true, ..
                        },
                    ) => *last = item,
                    _ => self.items.push(item),
                }
            }
            Some("toolResult") => {
                let run = self.tools.entry(str_of(message, "toolCallId")).or_default();
                run.output = tool_output(message);
                run.is_error = message
                    .get("isError")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                run.running = false;
                run.patch = result_patch(message);
            }
            Some("bashExecution") => self.items.push(Item::Bash {
                command: str_of(message, "command"),
                output: str_of(message, "output"),
                exit_code: message.get("exitCode").and_then(Value::as_i64),
                running: false,
            }),
            Some("custom") => {
                let display = message
                    .get("display")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                self.add_custom_message(
                    &str_of(message, "customType"),
                    message.get("content").unwrap_or(&Value::Null),
                    display,
                );
            }
            Some("compactionSummary") => self
                .items
                .push(Item::Notice("Context compacted".to_owned())),
            _ => {}
        }
    }

    /// Applies one streaming `assistantMessageEvent`.
    pub fn apply_update(&mut self, update: &Value) {
        let Some(Item::Assistant { blocks, .. }) = self.items.last_mut() else {
            return;
        };
        let index = update
            .get("contentIndex")
            .and_then(Value::as_u64)
            .unwrap_or(0) as usize;
        if blocks.len() <= index {
            blocks.resize(index + 1, None);
        }
        let delta = update
            .get("delta")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let slot = &mut blocks[index];
        match update
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
        {
            "text_start" => *slot = Some(Block::Text(String::new())),
            "thinking_start" => *slot = Some(Block::Thinking(String::new())),
            "toolcall_start" => {
                *slot = Some(Block::Tool {
                    id: str_of(update, "id"),
                    name: str_of(update, "toolName"),
                    args: String::new(),
                    written: None,
                })
            }
            "text_delta" => match slot {
                Some(Block::Text(text)) => text.push_str(delta),
                _ => *slot = Some(Block::Text(delta.to_owned())),
            },
            "thinking_delta" => match slot {
                Some(Block::Thinking(text)) => text.push_str(delta),
                _ => *slot = Some(Block::Thinking(delta.to_owned())),
            },
            "text_end" => *slot = Some(Block::Text(str_of(update, "content"))),
            "thinking_end" => *slot = Some(Block::Thinking(str_of(update, "content"))),
            "toolcall_end" => {
                if let Some(call) = update.get("toolCall") {
                    *slot = Some(tool_block(
                        str_of(call, "id"),
                        str_of(call, "name"),
                        call.get("arguments").unwrap_or(&Value::Null),
                    ));
                }
            }
            _ => {}
        }
    }

    pub fn title(&self) -> Option<String> {
        let fanout_prompt = self.items.iter().find_map(|i| match i {
            Item::Fanout { prompt, .. } => prompt.clone(),
            _ => None,
        });
        let title = self
            .name
            .clone()
            .or_else(|| self.first_prompt.clone())
            .or(fanout_prompt)?;
        Some(
            title
                .lines()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("Untitled")
                .trim()
                .to_owned(),
        )
    }

    pub fn has_answer(&self) -> bool {
        self.items
            .iter()
            .any(|i| matches!(i, Item::Assistant { .. } | Item::Fanout { .. }))
    }
}
