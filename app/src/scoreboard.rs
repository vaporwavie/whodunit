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

impl Table {
    /// The number this role ranks by: mean score, or the share of fan-outs won.
    pub fn rank_value(&self, r: &Row) -> Option<f64> {
        if self.fanout {
            r.win_share()
        } else {
            r.mean_score()
        }
    }

    /// The agent winning this role: the top row with enough runs and something to rank by.
    /// None until one has `TOO_FEW` runs.
    pub fn leader(&self) -> Option<Lead<'_>> {
        let mut ranked = self
            .rows
            .iter()
            .filter_map(|r| Some((r, self.rank_value(r)?)))
            .filter(|(r, _)| !r.too_few());
        let (row, value) = ranked.next()?;
        let margin = match ranked.next() {
            None => Margin::Alone,
            Some((next, v)) if value - v < 1e-9 => Margin::Tied {
                runner_up: next.name.clone(),
                cheaper: row.cost_per_run().unwrap_or(f64::MAX)
                    < next.cost_per_run().unwrap_or(f64::MAX),
            },
            Some((next, v)) => Margin::Ahead {
                runner_up: next.name.clone(),
                by: value - v,
            },
        };
        Some(Lead { row, margin })
    }

    /// The leader's ranking number: "4.25 mean score" or "5/8 fan-outs won".
    pub fn lead_figure(&self, r: &Row) -> String {
        if self.fanout {
            format!("{}/{} fan-outs won", r.wins, r.fanouts)
        } else {
            format!("{} mean score", fmt(r.mean_score(), 2, ""))
        }
    }

    /// The gap in this role's units: "0.58" points of mean score, or "38 pts" of win share.
    fn gap(&self, by: f64) -> String {
        if self.fanout {
            format!("{:.0} pts", by * 100.0)
        } else {
            format!("{by:.2}")
        }
    }

    /// How far the leader stands ahead, as a sentence: "ahead of Gemini 4 Argon by 0.58".
    pub fn margin_line(&self, margin: &Margin) -> String {
        match margin {
            Margin::Alone => "the only agent ranked so far".to_owned(),
            Margin::Ahead { runner_up, by } => {
                format!("ahead of {runner_up} by {}", self.gap(*by))
            }
            Margin::Tied {
                runner_up,
                cheaper: true,
            } => format!("level with {runner_up}, and cheaper"),
            Margin::Tied { runner_up, .. } => format!("level with {runner_up}"),
        }
    }

    /// The same, short enough for a tile: "+0.58", "+38 pts", "tied, cheaper" or "unopposed".
    pub fn margin_short(&self, margin: &Margin) -> String {
        match margin {
            Margin::Alone => "unopposed".to_owned(),
            Margin::Ahead { by, .. } => format!("+{}", self.gap(*by)),
            Margin::Tied { cheaper: true, .. } => "tied, cheaper".to_owned(),
            Margin::Tied { .. } => "tied".to_owned(),
        }
    }
}

/// How far a role's leader stands ahead of the next ranked agent.
#[derive(Clone, Debug, PartialEq)]
pub enum Margin {
    /// No other agent has enough runs to rank.
    Alone,
    /// Ahead by `by`: mean score points, or win share (0 to 1) for fan-out roles.
    Ahead { runner_up: String, by: f64 },
    /// Level on the ranking number; the cheaper agent leads.
    Tied { runner_up: String, cheaper: bool },
}

#[derive(Clone, Debug)]
pub struct Lead<'a> {
    pub row: &'a Row,
    pub margin: Margin,
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
    if !board.tables.is_empty() {
        out.push_str("\n## Leaders\n\n| role | leader | ranked by | lead |\n|---|---|---|---|\n");
        for table in &board.tables {
            match table.leader() {
                Some(lead) => out.push_str(&format!(
                    "| {} | **{}** | {} | {} |\n",
                    table.role,
                    lead.row.name,
                    table.lead_figure(lead.row),
                    table.margin_line(&lead.margin),
                )),
                None => out.push_str(&format!(
                    "| {} | — | — | no agent has {TOO_FEW} runs yet |\n",
                    table.role
                )),
            }
        }
    }
    for table in &board.tables {
        let leader = table.leader().map(|lead| lead.row.agent.clone());
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
            } else if leader.as_ref() == Some(&row.agent) {
                format!("**{}** *(leads)*", row.name)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn row(name: &str, runs: usize, score_sum: f64, cost: f64, fanouts: usize, wins: usize) -> Row {
        Row {
            agent: name.to_owned(),
            name: name.to_owned(),
            runs,
            rated: runs,
            score_sum,
            costs: vec![cost; runs],
            fanouts,
            wins,
            ..Row::default()
        }
    }

    fn table(fanout: bool, rows: Vec<Row>) -> Table {
        Table {
            role: "builder".to_owned(),
            criteria: String::new(),
            fanout,
            rows,
            decisions: Decisions::default(),
        }
    }

    #[test]
    fn the_leader_is_ahead_of_the_next_ranked_agent() {
        let t = table(
            false,
            vec![
                row("Opus", 4, 18.0, 0.4, 0, 0),
                row("Sol", 3, 11.0, 0.1, 0, 0),
            ],
        );
        let lead = t.leader().unwrap();
        assert_eq!(lead.row.name, "Opus");
        assert_eq!(t.margin_line(&lead.margin), "ahead of Sol by 0.83");
        assert_eq!(t.margin_short(&lead.margin), "+0.83");
        assert_eq!(t.lead_figure(lead.row), "4.50 mean score");
    }

    #[test]
    fn too_few_runs_never_lead_or_trail() {
        let t = table(
            false,
            vec![
                row("Opus", 3, 12.0, 0.4, 0, 0),
                row("Sol", 2, 10.0, 0.1, 0, 0),
            ],
        );
        let lead = t.leader().unwrap();
        assert_eq!(lead.margin, Margin::Alone);
        assert_eq!(t.margin_short(&lead.margin), "unopposed");
        assert!(
            table(false, vec![row("Sol", 2, 10.0, 0.1, 0, 0)])
                .leader()
                .is_none()
        );
    }

    #[test]
    fn a_tie_goes_to_the_cheaper_agent() {
        let t = table(
            false,
            vec![
                row("Kimi", 3, 12.0, 0.02, 0, 0),
                row("Opus", 3, 12.0, 0.4, 0, 0),
            ],
        );
        assert_eq!(
            t.margin_line(&t.leader().unwrap().margin),
            "level with Opus, and cheaper"
        );
    }

    #[test]
    fn fanout_roles_lead_by_win_share() {
        let t = table(
            true,
            vec![
                row("Astra", 8, 30.0, 0.06, 8, 5),
                row("Argon", 8, 34.0, 0.02, 8, 3),
            ],
        );
        let lead = t.leader().unwrap();
        assert_eq!(t.lead_figure(lead.row), "5/8 fan-outs won");
        assert_eq!(t.margin_line(&lead.margin), "ahead of Argon by 25 pts");
    }

    #[test]
    fn markdown_names_each_leader() {
        let board = Board {
            tables: vec![table(
                false,
                vec![
                    row("Opus", 4, 18.0, 0.4, 0, 0),
                    row("Sol", 3, 11.0, 0.1, 0, 0),
                ],
            )],
            runs: 7,
            decisions: Decisions::default(),
        };
        let md = markdown(&board);
        assert!(md.contains("| builder | **Opus** | 4.50 mean score | ahead of Sol by 0.83 |"));
        assert!(md.contains("| **Opus** *(leads)* | 4 |"));
    }
}
