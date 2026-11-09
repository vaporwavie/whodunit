//! The week's scoreboard: runs.jsonl joined with each run's session file, one table per role.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::Path;

use serde_json::Value;

use crate::config;
use crate::sessions;
use crate::transcript::str_of;

pub const TOO_FEW: usize = 3;

#[derive(Clone, Debug, Default)]
pub struct Row {
    pub agent: String,
    pub name: String,
    pub runs: usize,
    pub rated: usize,
    pub unrated: usize,
    pub nailed: usize,
    pub score_sum: f64,
    pub your_minutes: Vec<f64>,
    pub costs: Vec<f64>,
    pub fanouts: usize,
    pub wins: usize,
}

impl Row {
    pub fn nailed_pct(&self) -> Option<f64> {
        (self.rated > 0).then(|| 100.0 * self.nailed as f64 / self.rated as f64)
    }
    pub fn mean_score(&self) -> Option<f64> {
        (self.rated > 0).then(|| self.score_sum / self.rated as f64)
    }
    pub fn median_your_min(&self) -> Option<f64> {
        median(&self.your_minutes)
    }
    pub fn cost_per_run(&self) -> Option<f64> {
        (!self.costs.is_empty()).then(|| self.costs.iter().sum::<f64>() / self.costs.len() as f64)
    }
    pub fn win_share(&self) -> Option<f64> {
        (self.fanouts > 0).then(|| self.wins as f64 / self.fanouts as f64)
    }
    pub fn too_few(&self) -> bool {
        self.runs < TOO_FEW
    }
}

/// Who decided each deal's role: Jev on its own, you in the picker, `/role`, or the fallback.
#[derive(Clone, Debug, Default)]
pub struct Decisions {
    pub jev: usize,
    pub picker: usize,
    pub forced: usize,
    pub fallback: usize,
}

impl Decisions {
    pub fn add(&mut self, source: &str) {
        match source {
            "jev" => self.jev += 1,
            "picker" => self.picker += 1,
            "forced" => self.forced += 1,
            _ => self.fallback += 1,
        }
    }

    pub fn total(&self) -> usize {
        self.jev + self.picker + self.forced + self.fallback
    }

    /// Each decider as (source key, label, share of deals in whole percent), or None before
    /// the first deal.
    pub fn shares(&self) -> Option<[(&'static str, &'static str, f64); 4]> {
        let total = self.total();
        if total == 0 {
            return None;
        }
        let pct = |n: usize| (100.0 * n as f64 / total as f64).round();
        Some([
            ("jev", "Jev", pct(self.jev)),
            ("picker", "picker", pct(self.picker)),
            ("forced", "/role", pct(self.forced)),
            ("fallback", "fallback", pct(self.fallback)),
        ])
    }

    /// "Jev 80% · picker 15% · /role 5% · fallback 0%", or None before the first deal.
    pub fn line(&self) -> Option<String> {
        let shares = self.shares()?;
        let parts: Vec<String> = shares
            .iter()
            .map(|(_, label, share)| format!("{label} {share}%"))
            .collect();
        Some(parts.join(" · "))
    }
}

#[derive(Clone, Debug)]
pub struct Table {
    pub role: String,
    /// The role's `criteria` from roles.json: what kind of task gets dealt to it.
    pub criteria: String,
    pub fanout: bool,
    pub rows: Vec<Row>,
    pub decisions: Decisions,
}

#[derive(Clone, Debug, Default)]
pub struct Board {
    pub tables: Vec<Table>,
    pub runs: usize,
    pub decisions: Decisions,
}

fn median(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let mid = sorted.len() / 2;
    Some(if sorted.len() % 2 == 0 {
        (sorted[mid - 1] + sorted[mid]) / 2.0
    } else {
        sorted[mid]
    })
}

/// Each role's contenders and criteria, from roles.json.
fn role_sheets() -> BTreeMap<String, (Vec<String>, String)> {
    let Ok(text) = fs::read_to_string(config::week_dir().join("roles.json")) else {
        return BTreeMap::new();
    };
    let Ok(value) = serde_json::from_str::<Value>(&text) else {
        return BTreeMap::new();
    };
    let Some(roles) = value.get("roles").and_then(Value::as_object) else {
        return BTreeMap::new();
    };
    roles
        .iter()
        .map(|(id, role)| {
            let list = role
                .get("contenders")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default();
            let criteria = role
                .get("criteria")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            (id.clone(), (list, criteria))
        })
        .collect()
}

fn read_runs(path: &Path) -> Vec<Value> {
    let Ok(file) = fs::File::open(path) else {
        return Vec::new();
    };
    BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .filter_map(|l| serde_json::from_str(&l).ok())
        .collect()
}

pub fn compute() -> Board {
    let records = read_runs(&config::week_dir().join("runs.jsonl"));
    let mut deals: HashMap<String, Value> = HashMap::new();
    let mut dones: HashMap<String, Value> = HashMap::new();
    let mut fanouts: HashMap<String, Value> = HashMap::new();
    for record in records {
        let session = str_of(&record, "sessionId");
        match str_of(&record, "type").as_str() {
            "deal" => deals.insert(session, record),
            "done" => dones.insert(session, record),
            "fanout" => fanouts.insert(session, record),
            _ => None,
        };
    }

    let roles = config::roles();
    let sheets = role_sheets();
    let mut tables: Vec<Table> = roles
        .iter()
        .map(|(role, fanout)| {
            let (contenders, criteria) = sheets.get(role).cloned().unwrap_or_default();
            let rows = contenders
                .iter()
                .map(|agent| Row {
                    agent: agent.clone(),
                    name: config::agent_name(agent),
                    ..Row::default()
                })
                .collect();
            Table {
                role: role.clone(),
                criteria,
                fanout: *fanout,
                rows,
                decisions: Decisions::default(),
            }
        })
        .collect();

    let mut decisions = Decisions::default();
    for (session, deal) in &deals {
        let role = str_of(deal, "role");
        let agent = str_of(deal, "agent");
        let source = str_of(deal, "source");
        decisions.add(&source);
        let Some(table) = tables.iter_mut().find(|t| t.role == role) else {
            continue;
        };
        table.decisions.add(&source);
        let file = str_of(deal, "sessionFile");
        let metrics = (!file.is_empty())
            .then(|| sessions::metrics(Path::new(&file)))
            .flatten();
        let fanout = fanouts.get(session);
        let row_for = |rows: &mut Vec<Row>, agent: &str| -> usize {
            if let Some(i) = rows.iter().position(|r| r.agent == agent) {
                return i;
            }
            rows.push(Row {
                agent: agent.to_owned(),
                name: config::agent_name(agent),
                ..Row::default()
            });
            rows.len() - 1
        };
        if let Some(fanout) = fanout {
            for entry in fanout
                .get("entries")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let contender = str_of(entry, "agent");
                let i = row_for(&mut table.rows, &contender);
                let row = &mut table.rows[i];
                row.runs += 1;
                row.fanouts += 1;
                let mut cost = entry.get("costUsd").and_then(Value::as_f64).unwrap_or(0.0);
                if contender == agent {
                    row.wins += 1;
                    cost += metrics.as_ref().map_or(0.0, |m| m.cost);
                }
                row.costs.push(cost);
            }
        }
        let i = row_for(&mut table.rows, &agent);
        let row = &mut table.rows[i];
        if fanout.is_none() {
            row.runs += 1;
            if let Some(m) = &metrics {
                row.costs.push(m.cost);
            }
        }
        if let Some(m) = &metrics {
            row.your_minutes.push(m.your_minutes);
        }
        match dones.get(session) {
            Some(done) => {
                row.rated += 1;
                row.score_sum += done.get("score").and_then(Value::as_f64).unwrap_or(0.0);
                if str_of(done, "outcome") == "nailed it" {
                    row.nailed += 1;
                }
            }
            None => row.unrated += 1,
        }
    }

    for table in &mut tables {
        let fanout = table.fanout;
        table.rows.sort_by(|a, b| {
            let rank = |r: &Row| {
                if fanout {
                    r.win_share()
                } else {
                    r.mean_score().map(|s| s / 5.0)
                }
            };
            a.too_few()
                .cmp(&b.too_few())
                .then(rank(b).unwrap_or(-1.0).total_cmp(&rank(a).unwrap_or(-1.0)))
                .then(
                    a.cost_per_run()
                        .unwrap_or(f64::MAX)
                        .total_cmp(&b.cost_per_run().unwrap_or(f64::MAX)),
                )
        });
    }

    Board {
        tables,
        runs: deals.len(),
        decisions,
    }
}

fn fmt(value: Option<f64>, digits: usize, suffix: &str) -> String {
    value.map_or("—".to_owned(), |v| format!("{v:.digits$}{suffix}"))
}

pub fn markdown(board: &Board) -> String {
    let mut out = format!("# Whodunit scoreboard\n\n{} runs logged.\n", board.runs);
    if let Some(line) = board.decisions.line() {
        out.push_str(&format!("\nRole decided by: {line}.\n"));
    }
    for table in &board.tables {
        let ranked_by = if table.fanout {
            "fan-out win share"
        } else {
            "mean score"
        };
        out.push_str(&format!("\n## {} (ranked by {ranked_by})\n\n", table.role));
        if let Some(line) = table.decisions.line() {
            out.push_str(&format!("Role decided by: {line}.\n\n"));
        }
        out.push_str("| agent | runs | nailed % | mean score | median your-min | cost/run | fan-out wins | unrated |\n|---|---|---|---|---|---|---|---|\n");
        for row in &table.rows {
            let name = if row.too_few() {
                format!("{} *(too few)*", row.name)
            } else {
                row.name.clone()
            };
            let wins = if row.fanouts > 0 {
                format!("{}/{}", row.wins, row.fanouts)
            } else {
                "—".to_owned()
            };
            out.push_str(&format!(
                "| {name} | {} | {} | {} | {} | {} | {wins} | {} |\n",
                row.runs,
                fmt(row.nailed_pct(), 0, "%"),
                fmt(row.mean_score(), 2, ""),
                fmt(row.median_your_min(), 1, ""),
                row.cost_per_run()
                    .map_or("—".to_owned(), |v| format!("${v:.3}")),
                row.unrated,
            ));
        }
    }
    out
}
