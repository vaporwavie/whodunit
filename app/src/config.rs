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

#[derive(Serialize, Deserialize, Default, Clone)]
pub struct AppConfig {
    #[serde(default)]
    pub pi_bin: String,
    #[serde(default)]
    pub last_cwd: Option<PathBuf>,
}

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
