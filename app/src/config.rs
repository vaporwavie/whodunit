//! Where pi and its files live, and the app's own remembered settings.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

pub fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

pub fn week_dir() -> PathBuf {
    home().join(".pi/agent/whodunit")
}

pub fn sessions_root() -> PathBuf {
    home().join(".pi/agent/sessions")
}

/// pi's per-folder session directory: `--<path with / \ : as ->--`.
pub fn session_dir(cwd: &Path) -> PathBuf {
    let raw = cwd.to_string_lossy();
    let trimmed = raw.trim_start_matches('/');
    let encoded: String = trimmed
        .chars()
        .map(|c| {
            if matches!(c, '/' | '\\' | ':') {
                '-'
            } else {
                c
            }
        })
        .collect();
    sessions_root().join(format!("--{encoded}--"))
}

/// pi's default `compaction.reserveTokens`.
const DEFAULT_RESERVE_TOKENS: f64 = 16_384.0;
/// The model pi keys compaction overrides by: the selection, not the model it routes to.
const SELECTED_MODEL: &str = "whodunit/auto";

/// The free tokens below which pi compacts a session in `cwd` on its own, read the way pi
/// reads them: the project's `.pi/settings.json` over the global one, then the override for
/// `whodunit/auto`, the plain setting, the default. `None` when auto-compaction is off.
pub fn compaction_reserve(cwd: &Path) -> Option<f64> {
    let read = |path: PathBuf| {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .and_then(|v| v.get("compaction").cloned())
            .unwrap_or_default()
    };
    let mut compaction = read(home().join(".pi/agent/settings.json"));
    merge(&mut compaction, read(cwd.join(".pi/settings.json")));
    if compaction
        .get("enabled")
        .and_then(serde_json::Value::as_bool)
        == Some(false)
    {
        return None;
    }
    let tokens = |v: Option<&serde_json::Value>| v.and_then(serde_json::Value::as_f64);
    let overridden = compaction
        .get("modelOverrides")
        .and_then(|o| o.get(SELECTED_MODEL))
        .and_then(|o| o.get("reserveTokens"));
    tokens(overridden)
        .or_else(|| tokens(compaction.get("reserveTokens")))
        .or(Some(DEFAULT_RESERVE_TOKENS))
}

/// Deep-merges `over` into `base`, objects key by key, as pi layers project settings.
fn merge(base: &mut serde_json::Value, over: serde_json::Value) {
    match (base, over) {
        (serde_json::Value::Object(base), serde_json::Value::Object(over)) => {
            for (key, value) in over {
                merge(base.entry(key).or_insert(serde_json::Value::Null), value);
            }
        }
        (base, over) if !over.is_null() => *base = over,
        _ => {}
    }
}

#[derive(Serialize, Deserialize, Default, Clone)]
pub struct AppConfig {
    #[serde(default)]
    pub pi_bin: String,
    /// Folders pi ran in from the app, newest first, for the folder picker and the next launch.
    #[serde(default)]
    pub recent: Vec<RecentDir>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct RecentDir {
    pub path: PathBuf,
    /// Unix seconds.
    pub at: u64,
}

const RECENT_LIMIT: usize = 12;

impl AppConfig {
    fn file() -> PathBuf {
        week_dir().join("app.json")
    }

    pub fn load() -> AppConfig {
        std::fs::read_to_string(Self::file())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn remember(&mut self, path: &Path) {
        let at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        self.recent.retain(|r| r.path != path);
        self.recent.insert(
            0,
            RecentDir {
                path: path.to_owned(),
                at,
            },
        );
        self.recent.truncate(RECENT_LIMIT);
    }

    pub fn last_cwd(&self) -> Option<PathBuf> {
        self.recent
            .iter()
            .map(|r| r.path.clone())
            .find(|p| p.is_dir())
    }

    pub fn save(&self) {
        let _ = std::fs::create_dir_all(week_dir());
        if let Ok(text) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(Self::file(), text);
        }
    }
}

pub struct LoginEnv {
    pub vars: Vec<(String, String)>,
    pub pi_bin: Option<String>,
}

/// An app opened from Finder gets launchd's bare environment, with no node, pi or API keys: ask a
/// login shell once per launch. Kept in memory only, since it carries secrets.
pub fn login_env() -> &'static LoginEnv {
    static ENV: std::sync::OnceLock<LoginEnv> = std::sync::OnceLock::new();
    ENV.get_or_init(|| {
        const MARK: &str = "__whodunit_env__";
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_owned());
        let script = format!("command -v pi; print -rn -- {MARK}; env -0");
        let output = Command::new(&shell)
            .args(["-lic", &script])
            .stdin(std::process::Stdio::null())
            .output();
        let Ok(output) = output else {
            return LoginEnv {
                vars: Vec::new(),
                pi_bin: None,
            };
        };
        let text = String::from_utf8_lossy(&output.stdout);
        let Some((head, env)) = text.rsplit_once(MARK) else {
            return LoginEnv {
                vars: Vec::new(),
                pi_bin: None,
            };
        };
        let pi_bin = head
            .lines()
            .map(str::trim)
            .filter(|line| line.starts_with('/'))
            .last()
            .map(str::to_owned);
        let vars = env
            .split('\0')
            .filter_map(|pair| pair.split_once('='))
            .filter(|(k, _)| !matches!(*k, "PWD" | "OLDPWD" | "SHLVL" | "_"))
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .collect();
        LoginEnv { vars, pi_bin }
    })
}

/// Display names for dealt agents, from the week's roles.json (`agents` map), for the reveal.
pub fn agent_names() -> BTreeMap<String, String> {
    #[derive(Deserialize)]
    struct Roles {
        #[serde(default)]
        agents: BTreeMap<String, String>,
    }
    std::fs::read_to_string(week_dir().join("roles.json"))
        .ok()
        .and_then(|s| serde_json::from_str::<Roles>(&s).ok())
        .map(|r| r.agents)
        .unwrap_or_default()
}

pub fn agent_name(agent: &str) -> String {
    let names = agent_names();
    let Some(name) = names.get(agent) else {
        return agent.rsplit('/').next().unwrap_or(agent).to_owned();
    };
    name.clone()
}

/// Role ids in roles.json order, and whether each fans out, for the scoreboard.
pub fn roles() -> Vec<(String, bool)> {
    let Ok(text) = std::fs::read_to_string(week_dir().join("roles.json")) else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Vec::new();
    };
    let Some(roles) = value.get("roles").and_then(|r| r.as_object()) else {
        return Vec::new();
    };
    roles
        .iter()
        .map(|(id, role)| {
            (
                id.clone(),
                role.get("fanout")
                    .and_then(|f| f.as_bool())
                    .unwrap_or(false),
            )
        })
        .collect()
}

pub fn short_path(path: &Path) -> String {
    let home = home();
    match path.strip_prefix(&home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}
