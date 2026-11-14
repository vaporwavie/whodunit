//! What the folder picker lists: recent folders and the subfolders of the one being browsed.

use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::Value;

use crate::config::{self, AppConfig};

#[derive(Clone, Debug)]
pub struct Recent {
    pub path: PathBuf,
    pub at: SystemTime,
}

const RECENT_SHOWN: usize = 6;
const FOLDER_LIMIT: usize = 400;

/// Folders opened from the app plus folders plain pi ran in (each session file's header names its
/// `cwd`), home first, then newest first, existing ones only.
pub fn recents(config: &AppConfig) -> Vec<Recent> {
    let mut latest: HashMap<PathBuf, SystemTime> = HashMap::new();
    let mut note = |path: PathBuf, at: SystemTime| {
        let entry = latest.entry(path).or_insert(at);
        *entry = (*entry).max(at);
    };
    for recent in &config.recent {
        note(
            recent.path.clone(),
            UNIX_EPOCH + Duration::from_secs(recent.at),
        );
    }
    for (path, at) in pi_session_folders() {
        note(path, at);
    }
    let mut recents: Vec<Recent> = latest
        .into_iter()
        .filter(|(path, _)| !config::is_temporary(path) && path.is_dir())
        .map(|(path, at)| Recent { path, at })
        .collect();
    let home = config::home();
    recents.sort_by_key(|r| (r.path != home, std::cmp::Reverse(r.at)));
    recents.truncate(RECENT_SHOWN);
    recents
}

fn pi_session_folders() -> Vec<(PathBuf, SystemTime)> {
    let Ok(read) = fs::read_dir(config::sessions_root()) else {
        return Vec::new();
    };
    read.filter_map(Result::ok)
        .filter_map(|dir| newest_session(&dir.path()))
        .filter_map(|(file, at)| Some((session_cwd(&file)?, at)))
        .collect()
}

fn newest_session(dir: &Path) -> Option<(PathBuf, SystemTime)> {
    fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "jsonl"))
        .filter_map(|p| {
            let at = fs::metadata(&p).and_then(|m| m.modified()).ok()?;
            Some((p, at))
        })
        .max_by_key(|(_, at)| *at)
}

fn session_cwd(file: &Path) -> Option<PathBuf> {
    let mut first = String::new();
    BufReader::new(fs::File::open(file).ok()?)
        .read_line(&mut first)
        .ok()?;
    let header: Value = serde_json::from_str(&first).ok()?;
    let cwd = header.get("cwd")?.as_str()?;
    Some(PathBuf::from(cwd))
}

/// Visible subfolders, sorted as Finder sorts them (case-insensitive).
pub fn subfolders(dir: &Path) -> Vec<String> {
    let Ok(read) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = read
        .filter_map(Result::ok)
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|name| !name.starts_with('.'))
        .collect();
    names.sort_by_key(|name| name.to_lowercase());
    names.truncate(FOLDER_LIMIT);
    names
}

/// "now", "5m", "3h", "2d", "4w": how long ago, in the space of a column.
pub fn ago(at: SystemTime, now: SystemTime) -> String {
    let secs = now.duration_since(at).map_or(0, |d| d.as_secs());
    match secs {
        0..60 => "now".to_owned(),
        60..3_600 => format!("{}m", secs / 60),
        3_600..86_400 => format!("{}h", secs / 3_600),
        86_400..1_209_600 => format!("{}d", secs / 86_400),
        _ => format!("{}w", secs / 604_800),
    }
}

/// How long `ago(at, now)` keeps reading the same.
pub fn ago_lasts(at: SystemTime, now: SystemTime) -> Duration {
    let Ok(elapsed) = now.duration_since(at) else {
        return at.duration_since(now).unwrap_or_default() + Duration::from_secs(60);
    };
    let unit = match elapsed.as_secs() {
        0..3_600 => 60,
        3_600..86_400 => 3_600,
        86_400..1_209_600 => 86_400,
        _ => 604_800,
    };
    Duration::from_secs((elapsed.as_secs() / unit + 1) * unit) - elapsed
}
