//! Reads pi session files for the sidebar, reopened chats and the scoreboard's automatic metrics.

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde_json::Value;

use crate::transcript::{
    DEAL, DONE, Deal, FANOUT_PICK, SWITCH, Transcript, Week, content_text, str_of,
};

#[derive(Clone, Debug)]
pub struct Summary {
    pub path: PathBuf,
    /// The folder pi ran in, from the session file's header.
    pub cwd: PathBuf,
    pub title: String,
    pub week: Week,
}

impl Summary {
    /// The deal, once the run has ended and its role and agent can show. The sidebar draws the
    /// role as an icon and names both in its tooltip.
    pub fn revealed(&self) -> Option<&Deal> {
        self.week.done.as_ref()?;
        self.week.deal.as_ref()
    }

    /// The folder, then the size and score once the run has ended and they can show; the role
    /// shows as the row's icon instead.
    pub fn subtitle(&self) -> String {
        let folder = self
            .cwd
            .file_name()
            .map_or_else(|| self.cwd.to_string_lossy(), |name| name.to_string_lossy())
            .into_owned();
        let (Some(deal), Some(done)) = (&self.week.deal, &self.week.done) else {
            return folder;
        };
        format!("{folder} · {} · {}/5", deal.size, done.score)
    }
}

const SIDEBAR_LIMIT: usize = 60;

/// Dealt sessions from every folder's session directory, newest first; sessions without a role are left out.
pub fn list(root: &Path) -> Vec<Summary> {
    let Ok(dirs) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut files: Vec<(SystemTime, PathBuf)> = dirs
        .filter_map(Result::ok)
        .filter_map(|dir| fs::read_dir(dir.path()).ok())
        .flatten()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "jsonl"))
        .filter_map(|p| {
            fs::metadata(&p)
                .and_then(|m| m.modified())
                .ok()
                .map(|t| (t, p))
        })
        .collect();
    files.sort_by(|a, b| b.0.cmp(&a.0));
    files
        .into_iter()
        .filter_map(|(_, path)| summarize(&path))
        .filter(|summary| summary.week.deal.is_some())
        .take(SIDEBAR_LIMIT)
        .collect()
}

/// A saved chat as its pi process will load it: the branch from the file's last entry, pi's
/// leaf on load, back to the root. `None` for pre-tree files without ids, which pi migrates.
pub fn transcript(path: &Path) -> Option<Transcript> {
    let text = fs::read_to_string(path).ok()?;
    let entries: Vec<Value> = text
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|entry| entry.get("type").and_then(Value::as_str) != Some("session"))
        .collect();
    let leaf = entries.last()?.get("id").and_then(Value::as_str)?;
    Some(Transcript::from_entries(&entries, Some(leaf)))
}

fn summarize(path: &Path) -> Option<Summary> {
    let file = fs::File::open(path).ok()?;
    let mut week = Week::default();
    let mut name = None;
    let mut first_prompt = None;
    let mut cwd = None;
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        if cwd.is_none() && line.starts_with(r#"{"type":"session""#) {
            cwd = serde_json::from_str::<Value>(&line)
                .ok()
                .and_then(|header| header.get("cwd")?.as_str().map(PathBuf::from));
            continue;
        }
        let interesting = line.starts_with(r#"{"type":"custom""#)
            || line.starts_with(r#"{"type":"session_info""#)
            || (first_prompt.is_none() && line.contains(r#""role":"user""#));
        if !interesting {
            continue;
        }
        let Ok(entry) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        match entry.get("type").and_then(Value::as_str) {
            Some("custom") => {
                week.apply_custom(
                    &str_of(&entry, "customType"),
                    entry.get("data").unwrap_or(&Value::Null),
                );
            }
            Some("custom_message")
                if str_of(&entry, "customType") == FANOUT_PICK && first_prompt.is_none() =>
            {
                let content = content_text(entry.get("content").unwrap_or(&Value::Null));
                first_prompt = content
                    .strip_prefix("User request:\n\n")
                    .and_then(|r| r.split("\n\nYour answer:").next())
                    .map(str::to_owned);
            }
            Some("session_info") => {
                name = entry.get("name").and_then(Value::as_str).map(str::to_owned)
            }
            Some("message") if first_prompt.is_none() => {
                let message = entry.get("message").unwrap_or(&Value::Null);
                if message.get("role").and_then(Value::as_str) == Some("user") {
                    first_prompt =
                        Some(content_text(message.get("content").unwrap_or(&Value::Null)));
                }
            }
            _ => {}
        }
    }
    let title = name.or(first_prompt)?;
    let title = title
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("Untitled")
        .trim()
        .to_owned();
    Some(Summary {
        path: path.to_owned(),
        cwd: cwd?,
        title,
        week,
    })
}

/// What a session file says about how a run went, measured the same way for every agent.
#[derive(Clone, Debug, Default)]
pub struct Metrics {
    pub your_minutes: f64,
    pub agent_minutes: f64,
    pub turns: usize,
    pub cost: f64,
    pub compactions: usize,
    pub switched: bool,
    pub week: Week,
}

const GAP_CAP_MS: f64 = 10.0 * 60_000.0;

fn usage_cost(value: &Value) -> f64 {
    value
        .pointer("/usage/cost/total")
        .and_then(Value::as_f64)
        .unwrap_or(0.0)
}

pub fn metrics(path: &Path) -> Option<Metrics> {
    let file = fs::File::open(path).ok()?;
    let mut m = Metrics::default();
    let mut users = 0usize;
    let mut turn_start: Option<f64> = None;
    let mut last_agent: Option<f64> = None;
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        let Ok(entry) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        match entry.get("type").and_then(Value::as_str) {
            Some("message") => {
                let message = entry.get("message").unwrap_or(&Value::Null);
                let at = message
                    .get("timestamp")
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0);
                match message.get("role").and_then(Value::as_str) {
                    Some("user") => {
                        if let (Some(start), Some(end)) = (turn_start, last_agent) {
                            m.agent_minutes += (end - start).max(0.0) / 60_000.0;
                        }
                        if let Some(end) = last_agent.filter(|_| users > 0) {
                            m.your_minutes += (at - end).clamp(0.0, GAP_CAP_MS) / 60_000.0;
                        }
                        users += 1;
                        turn_start = Some(at);
                        last_agent = None;
                    }
                    Some("assistant") => {
                        m.cost += usage_cost(message);
                        turn_start.get_or_insert(at);
                        last_agent = Some(at);
                    }
                    Some("toolResult") => {
                        m.cost += usage_cost(message);
                        last_agent = Some(at.max(last_agent.unwrap_or(0.0)));
                    }
                    _ => {}
                }
            }
            Some("usage") | Some("branch_summary") => m.cost += usage_cost(&entry),
            Some("compaction") => {
                m.cost += usage_cost(&entry);
                m.compactions += 1;
            }
            Some("custom") => {
                let custom_type = str_of(&entry, "customType");
                if custom_type == SWITCH {
                    m.switched = true;
                }
                if custom_type == DEAL || custom_type == DONE || custom_type == SWITCH {
                    m.week
                        .apply_custom(&custom_type, entry.get("data").unwrap_or(&Value::Null));
                }
            }
            _ => {}
        }
    }
    if let (Some(start), Some(end)) = (turn_start, last_agent) {
        m.agent_minutes += (end - start).max(0.0) / 60_000.0;
    }
    m.turns = users.saturating_sub(1);
    Some(m)
}
