//! The scoreboard page: who leads each role, every contender, then one table per role.

use std::time::Duration;

use rune::{Color, El, ScrollSpec, Shape, Spring, col, el, row, text};
use rune_kit::{Button, ButtonVariant, Kit};

use super::{Page, Window, sprite};
use crate::core::cmd;
use crate::scoreboard::{self, Board};
use crate::sprites;

fn scoreboard_empty(k: &Kit) -> El {
    let c = &k.c;
    col()
        .key("scoreboard-empty")
        .w_full()
        .gap(10.0)
        .pt(100.0)
        .items_center()
        .child(text("No rated runs yet").font(k.ty.body_medium).color(c.text_base))
        .child(text(format!("Finish a task and end it with {}. Each role gets a table here, and an agent needs {} runs to rank.", cmd("⏎"), scoreboard::TOO_FEW)).font(k.ty.small).color(c.text_muted))
        .child(
            row().pt(8.0).child(
                Button::new("Start a task")
                .variant(ButtonVariant::Outline)
                .shortcut(cmd("1"))
                .show_shortcut()
                .on_press_tx(|s: &mut Window, tx| {
                    s.show(Page::Chat, tx);
                    s.draft.composer.focused = true;
                })
                .build(k),
            ),
        )
}
fn count(n: usize, one: &str, many: &str) -> String {
    let noun = if n == 1 { one } else { many };
    format!("{n} {noun}")
}

/// Every agent on the board once, in the order the tables first list them, with its runs and
/// fan-out wins summed across roles.
fn contenders(board: &Board) -> Vec<(String, String, usize, usize)> {
    let mut out: Vec<(String, String, usize, usize)> = Vec::new();
    for r in board.tables.iter().flat_map(|t| &t.rows) {
        match out.iter_mut().find(|(agent, ..)| *agent == r.agent) {
            Some((_, _, runs, wins)) => {
                *runs += r.runs;
                *wins += r.wins;
            }
            None => out.push((r.agent.clone(), r.name.clone(), r.runs, r.wins)),
        }
    }
    out
}

fn roster(board: &Board, k: &Kit) -> El {
    let c = &k.c;
    let numbers = k.ty.small.features("'tnum' 1");
    let mut strip = row().key("roster").w_full().gap(10.0).flex_wrap();
    for (agent, name, runs, wins) in contenders(board) {
        let stat = if wins > 0 {
            format!(
                "{} · {}",
                count(runs, "run", "runs"),
                count(wins, "win", "wins")
            )
        } else {
            count(runs, "run", "runs")
        };
        strip = strip.child(
            col()
                .key(format!("roster-{agent}"))
                .w(128.0)
                .grow(1.0)
                .gap(2.0)
                .px(10.0)
                .py(12.0)
                .items_center()
                .fill(c.surface_raised)
                .border(1.0, c.border_weak)
                .radius(k.radius.md)
                .child(row().pb(8.0).child(sprite(sprites::model(&agent))))
                .child(
                    text(name)
                        .font(k.ty.ui_medium)
                        .color(c.text_base)
                        .truncate(),
                )
                .child(text(stat).font(numbers).color(c.text_muted)),
        );
    }
    strip
}

/// One tile per role with the agent winning it, so the week's verdict reads before the cards.
fn leaders(board: &Board, k: &Kit) -> El {
    let c = &k.c;
    let numbers = k.ty.small.features("'tnum' 1");
    let mut strip = row().key("leaders").w_full().gap(10.0).flex_wrap();
    for table in &board.tables {
        let lead = table.leader();
        let label = text(table.role.to_uppercase())
            .font(k.ty.small_medium.tracking(0.5))
            .color(c.text_muted)
            .truncate();
        let body = match &lead {
            Some(lead) => {
                let figure = if table.fanout {
                    format!("{}/{} won", lead.row.wins, lead.row.fanouts)
                } else {
                    fmt(lead.row.mean_score(), 2)
                };
                row()
                    .gap(10.0)
                    .center_y()
                    .child(sprite(sprites::model(&lead.row.agent)))
                    .child(
                        col()
                            .gap(2.0)
                            .shrink()
                            .min_w(0.0)
                            .child(
                                text(lead.row.name.clone())
                                    .font(k.ty.ui_medium)
                                    .color(c.text_base)
                                    .truncate(),
                            )
                            .child(
                                row()
                                    .gap(6.0)
                                    .child(text(figure).font(numbers).color(c.text_accent))
                                    .child(
                                        text(table.margin_short(&lead.margin))
                                            .font(numbers)
                                            .color(c.text_muted)
                                            .truncate()
                                            .shrink()
                                            .min_w(0.0),
                                    ),
                            ),
                    )
            }
            None => col()
                .h(36.0)
                .gap(2.0)
                .justify_center()
                .child(
                    text("No leader yet")
                        .font(k.ty.ui_medium)
                        .color(c.text_muted)
                        .truncate(),
                )
                .child(
                    text(format!(
                        "{} to rank",
                        count(scoreboard::TOO_FEW, "run", "runs")
                    ))
                    .font(numbers)
                    .color(c.text_faint)
                    .truncate(),
                ),
        };
        let (fill, border) = if lead.is_some() {
            (k.alpha(c.text_accent, 0.07), k.alpha(c.text_accent, 0.35))
        } else {
            (c.surface_raised, c.border_weak)
        };
        strip = strip.child(
            col()
                .key(format!("lead-{}", table.role))
                .w(150.0)
                .grow(1.0)
                .gap(10.0)
                .px(12.0)
                .py(12.0)
                .fill(fill)
                .border(1.0, border)
                .radius(k.radius.md)
                .child(label)
                .child(body),
        );
    }
    strip
}

/// A strip of tiles under a small caps label.
fn section(label: &str, body: El, k: &Kit) -> El {
    col()
        .w_full()
        .gap(10.0)
        .child(
            text(label.to_owned())
                .font(k.ty.small_medium.tracking(0.5))
                .color(k.c.text_muted),
        )
        .child(body)
}

fn fmt(v: Option<f64>, digits: usize) -> String {
    v.map_or("—".to_owned(), |v| format!("{v:.digits$}"))
}

/// Past this the page stops widening, so a table's figures stay near their names.
const PAGE_MAX_W: f32 = 1040.0;
/// From a table row's top to a figure's line, centring it on the 24-point sprite.
const FIGURE_DROP: f32 = 2.0;
const RANK_COL_W: f32 = 14.0;
/// The contender's sprite and name, before the figure columns start to give way.
const NAME_MIN_W: f32 = 100.0;
const TABLE_GAP: f32 = 12.0;

/// One figure column of a class table: its header, what it means on hover, and its width,
/// which every row shrinks alike towards `min` when the card is narrow, so columns stay lined up.
struct Column {
    label: &'static str,
    help: &'static str,
    width: f32,
    min: f32,
}

impl Column {
    /// The column the class ranks by, wide enough for "tied, cheaper" under the leader's figure.
    fn ranking(label: &'static str, help: &'static str) -> Self {
        Column {
            label,
            help,
            width: 80.0,
            min: 56.0,
        }
    }

    fn figure(label: &'static str, help: &'static str) -> Self {
        Column {
            label,
            help,
            width: 60.0,
            min: 46.0,
        }
    }

    fn cell(&self) -> El {
        col().w(self.width).shrink().min_w(self.min).items_end()
    }
}

/// The figure columns, the ranking one first: mean score, or fan-outs won.
fn columns(fanout: bool) -> Vec<Column> {
    let mut out = if fanout {
        vec![
            Column::ranking("WON", "Fan-outs won, which this class ranks by"),
            Column::figure("SCORE", "Mean score, 1 to 5"),
        ]
    } else {
        vec![Column::ranking(
            "SCORE",
            "Mean score, 1 to 5, which this class ranks by",
        )]
    };
    out.extend([
        Column::figure("RUNS", "Runs dealt in this class"),
        Column::figure("NAILED", "Rated runs scored 5 · nailed it"),
        Column::figure("COST", "Mean cost per run, at list price"),
        Column {
            width: 68.0,
            min: 60.0,
            ..Column::figure(
                "YOUR TIME",
                "Median minutes you spent per run, idle gaps capped at 10",
            )
        },
    ]);
    out
}

/// A figure and the small line under it, if any.
struct Cell {
    figure: String,
    note: Option<String>,
    /// The leader's ranking figure, drawn in the accent.
    lead: bool,
}

fn minutes(v: f64) -> String {
    if v < 0.05 {
        "<0.1 min".to_owned()
    } else {
        format!("{v:.1} min")
    }
}

/// One contender's cells, in the order of `columns`.
fn cells(table: &scoreboard::Table, r: &scoreboard::Row, lead: Option<&str>) -> Vec<Cell> {
    let plain = |figure: String| Cell {
        figure,
        note: None,
        lead: false,
    };
    let ranking = |figure: String| Cell {
        figure,
        note: lead.map(str::to_owned),
        lead: lead.is_some(),
    };
    let score = fmt(r.mean_score(), 2);
    let mut out = Vec::new();
    if table.fanout {
        let won = if r.fanouts > 0 {
            format!("{}/{}", r.wins, r.fanouts)
        } else {
            "—".to_owned()
        };
        out.push(ranking(won));
        out.push(plain(score));
    } else {
        out.push(ranking(score));
    }
    out.push(Cell {
        figure: r.runs.to_string(),
        note: (r.unrated > 0).then(|| format!("{} unrated", r.unrated)),
        lead: false,
    });
    out.push(plain(
        r.nailed_pct()
            .map_or("—".to_owned(), |pct| format!("{pct:.0}%")),
    ));
    out.push(plain(
        r.cost_per_run()
            .map_or("—".to_owned(), |cost| format!("${cost:.3}")),
    ));
    out.push(plain(r.median_your_min().map_or("—".to_owned(), minutes)));
    out
}

/// A table row's frame: the rank, the contender (or its header) growing, then the figures.
fn table_row(rank: El, name: El, figures: El) -> El {
    row()
        .w_full()
        .gap(TABLE_GAP)
        .items_start()
        .px(16.0)
        .child(row().w(RANK_COL_W).child(rank))
        .child(name)
        .child(figures)
}

/// The figure columns' span in a row: their widths and gaps, shrinking alike in every row
/// towards their minimums, so a blocked row's placeholder covers exactly the columns.
fn figure_span(fanout: bool) -> El {
    let columns = columns(fanout);
    let gaps = TABLE_GAP * columns.len().saturating_sub(1) as f32;
    let width = columns.iter().map(|column| column.width).sum::<f32>() + gaps;
    let min = columns.iter().map(|column| column.min).sum::<f32>() + gaps;
    row()
        .w(width)
        .shrink()
        .min_w(min)
        .gap(TABLE_GAP)
        .items_start()
}

fn figures(fanout: bool, cells: impl IntoIterator<Item = El>) -> El {
    let mut span = figure_span(fanout).key("figures");
    for cell in cells {
        span = span.child(cell);
    }
    span
}

/// A blocked row's height: a model sprite's.
const BLOCKED_H: f32 = 24.0;
/// Wider than any table's figure columns, whose span clips the hatch.
const HATCH_SPAN: f32 = 480.0;

/// Diagonal stripes of dither cells, one in four, rising to the right.
fn hatch(color: Color) -> Shape {
    let rows = (BLOCKED_H / DITHER_PT) as usize;
    let cols = (HATCH_SPAN / DITHER_PT) as usize;
    let mut shape = Shape::new();
    for row in 0..rows {
        for col in (0..cols).filter(|col| (col + row) % 4 == 0) {
            let (x, y) = (col as f32 * DITHER_PT, row as f32 * DITHER_PT);
            shape = shape.fill(
                [
                    [x, y],
                    [x + DITHER_PT, y],
                    [x + DITHER_PT, y + DITHER_PT],
                    [x, y + DITHER_PT],
                ],
                color,
            );
        }
    }
    shape
}

/// A contender short of runs: its figures held back behind a hatch until it ranks, with a
/// block per run it needs, filled for those it has.
fn blocked(fanout: bool, runs: usize, k: &Kit) -> El {
    let c = &k.c;
    let needed = scoreboard::TOO_FEW.saturating_sub(runs);
    let mut progress = row().gap(2.0);
    for run in 0..scoreboard::TOO_FEW {
        let done = run < runs;
        progress = progress.child(el().size(2.0 * DITHER_PT, 2.0 * DITHER_PT).fill(if done {
            c.text_muted
        } else {
            c.border_base
        }));
    }
    figure_span(fanout)
        .key("blocked")
        .h(BLOCKED_H)
        .center()
        .radius(k.radius.sm)
        .clip_rounded(k.radius.sm)
        .child(
            el().pin(Some(0.0), Some(0.0), None, None)
                .size(HATCH_SPAN, BLOCKED_H)
                .shape(hatch(c.border_base)),
        )
        .child(
            row()
                .gap(8.0)
                .center_y()
                .px(8.0)
                .h(20.0)
                .shrink()
                .fill(c.surface_raised)
                .radius(k.radius.sm)
                .child(progress)
                .child(
                    text(format!(
                        "Blocked · {} to rank",
                        count(needed, "more run", "more runs")
                    ))
                    .font(k.ty.small)
                    .color(c.text_muted)
                    .truncate()
                    .shrink()
                    .min_w(0.0),
                ),
        )
}

fn table_head(fanout: bool, k: &Kit) -> El {
    let c = &k.c;
    let label = |s: &str| {
        text(s.to_owned())
            .font(k.ty.small_medium.tracking(0.5))
            .color(c.text_muted)
    };
    let cells = columns(fanout)
        .into_iter()
        .map(|column| column.cell().child(label(column.label).help(column.help)));
    table_row(
        el(),
        row()
            .w(0.0)
            .grow(1.0)
            .min_w(NAME_MIN_W)
            .child(label("CONTENDER")),
        figures(fanout, cells),
    )
    .key("head")
    .py(8.0)
    .border_sides([1.0, 0.0, 0.0, 0.0], c.border_weak)
}

fn contender_row(
    table: &scoreboard::Table,
    r: &scoreboard::Row,
    rank: Option<usize>,
    lead: Option<&scoreboard::Margin>,
    k: &Kit,
) -> El {
    let c = &k.c;
    let leads = lead.is_some();
    let (ink, soft) = if rank.is_some() {
        (c.text_base, c.text_muted)
    } else {
        (c.text_faint, c.text_faint)
    };
    let figure_font = k.ty.ui.features("'tnum' 1");
    let note_font = k.ty.small.features("'tnum' 1");
    let margin = lead.map(|margin| table.margin_short(margin));
    let row_cells = columns(table.fanout)
        .into_iter()
        .zip(cells(table, r, margin.as_deref()))
        .map(|(column, cell)| {
            let (font, color) = if cell.lead {
                (k.ty.ui_medium.features("'tnum' 1"), c.text_accent)
            } else {
                (figure_font, ink)
            };
            let mut stack = column
                .cell()
                .pt(FIGURE_DROP)
                .child(text(cell.figure).font(font).color(color));
            if let Some(note) = cell.note {
                stack = stack.child(text(note).font(note_font).color(soft).truncate());
            }
            stack
        });
    let span = if rank.is_some() {
        figures(table.fanout, row_cells)
    } else {
        blocked(table.fanout, r.runs, k)
    };
    let name = row()
        .w(0.0)
        .grow(1.0)
        .min_w(NAME_MIN_W)
        .gap(10.0)
        .center_y()
        .child(
            sprite(sprites::model_small(&r.agent)).opacity(if rank.is_some() { 1.0 } else { 0.45 }),
        )
        .child(
            text(r.name.clone())
                .font(k.ty.ui_medium)
                .color(ink)
                .truncate()
                .help_when_truncated(r.name.clone())
                .shrink()
                .min_w(0.0),
        );
    let position = row().pt(FIGURE_DROP).child(
        text(rank.map_or("–".to_owned(), |rank| rank.to_string()))
            .font(k.ty.ui_medium.features("'tnum' 1"))
            .color(if leads { c.text_accent } else { soft }),
    );
    let mut line = table_row(position, name, span)
        .key(format!("row-{}", r.agent))
        .py(8.0)
        .border_sides([1.0, 0.0, 0.0, 0.0], c.border_weak);
    if leads {
        line = line.fill(k.alpha(c.text_accent, 0.07));
    }
    line
}

const DEALT_BAR_W: f32 = 96.0;
/// One dither cell, the size of a model sprite's pixel.
const DITHER_PT: f32 = 3.0;
/// Two dither rows.
const DEALT_BAR_H: f32 = 2.0 * DITHER_PT;
const DEALT_BAR_GAP: f32 = 2.0;
/// Past the page's own fade, so the decider bar sweeps in where it can be seen.
pub(super) const RISE_AFTER: Duration = Duration::from_millis(150);

/// A decider's stretch of the bar in one colour: Jev solid, then sparser dithers for the
/// picker, `/role` and the fallback, so each reads apart from its neighbour.
fn dealt_segment(decider: usize, width: f32, color: Color) -> Shape {
    let cols = (width / DITHER_PT).ceil() as usize;
    let mut shape = Shape::new();
    for row in 0..2 {
        for col in 0..cols {
            let on = match decider {
                0 => true,
                1 => (row + col) % 2 == 0,
                2 => row == 0 && col % 2 == 0,
                _ => row == 1 && col % 2 == 1,
            };
            if !on {
                continue;
            }
            let x = col as f32 * DITHER_PT;
            let right = (x + DITHER_PT).min(width);
            let top = row as f32 * DITHER_PT;
            shape = shape.fill(
                [
                    [x, top],
                    [right, top],
                    [right, top + DITHER_PT],
                    [x, top + DITHER_PT],
                ],
                color,
            );
        }
    }
    shape
}

/// Who decided this class's deals: a dithered bar, one stretch per decider, held left of its
/// track until `risen` and then sweeping in, beside each share that isn't zero with its
/// decider's icon. The shares wrap under the bar when the card is narrow.
fn dealt_line(decisions: &scoreboard::Decisions, risen: bool, k: &Kit) -> Option<El> {
    let c = &k.c;
    let shares = decisions.shares()?;
    let shown: Vec<_> = shares
        .iter()
        .enumerate()
        .filter(|(_, (.., share))| *share > 0.0)
        .collect();
    let room = DEALT_BAR_W - DEALT_BAR_GAP * shown.len().saturating_sub(1) as f32;
    let mut segments = row()
        .key("fill")
        .gap(DEALT_BAR_GAP)
        .offset(if risen { 0.0 } else { -DEALT_BAR_W }, 0.0)
        .offset_motion(Spring::new(0.45, 0.15));
    for (decider, (source, _, share)) in &shown {
        let width = (room * (*share / 100.0) as f32).max(2.0 * DITHER_PT);
        segments = segments.child(
            el().key(*source)
                .size(width, DEALT_BAR_H)
                .shape(dealt_segment(*decider, width, c.text_muted)),
        );
    }
    let font = k.ty.small.features("'tnum' 1");
    let mut line = row()
        .key("dealt")
        .gaps(4.0, 10.0)
        .flex_wrap()
        .center_y()
        .child(
            el().w(DEALT_BAR_W)
                .h(DEALT_BAR_H)
                .clip_rounded(0.0)
                .child(segments),
        )
        .child(text("Decided by").font(font).color(c.text_muted));
    for (_, (source, label, share)) in &shown {
        line = line.child(
            row()
                .key(format!("decider-{source}"))
                .gap(4.0)
                .center_y()
                .child(sprite(sprites::decider(source)))
                .child(
                    text(format!("{label} {share}%"))
                        .font(font)
                        .color(c.text_muted),
                ),
        );
    }
    Some(line)
}

fn sheet_header(table: &scoreboard::Table, risen: bool, k: &Kit) -> El {
    let c = &k.c;
    let mut title = row().gap(8.0).center_y().child(
        text(table.role.to_uppercase())
            .font(k.ty.body_medium.tracking(0.4))
            .color(c.text_base),
    );
    if table.fanout {
        title = title.child(text("fan-out").font(k.ty.small).color(c.text_faint));
    }
    let mut heading = col().grow(1.0).min_w(0.0).gap(4.0).child(title);
    if !table.criteria.is_empty() {
        heading = heading.child(
            text(table.criteria.clone())
                .font(k.ty.ui)
                .color(c.text_muted)
                .wrap()
                .shrink()
                .min_w(0.0),
        );
    }
    if let Some(dealt) = dealt_line(&table.decisions, risen, k) {
        heading = heading.child(dealt.pt(4.0));
    }
    row()
        .w_full()
        .gap(16.0)
        .items_center()
        .padding(16.0)
        .child(sprite(sprites::role(&table.role)))
        .child(heading)
}

/// One class: who decided its deals, then every contender as a table row, ranked ones first
/// and the leader tinted, with those still short of runs faded below.
fn role_sheet(table: &scoreboard::Table, risen: bool, k: &Kit) -> El {
    let c = &k.c;
    let mut card = col()
        .key(format!("role-{}", table.role))
        .w_full()
        .fill(c.surface_raised)
        .border(1.0, c.border_weak)
        .radius(k.radius.lg)
        .clip_rounded(k.radius.lg)
        .child(sheet_header(table, risen, k));
    if table.rows.is_empty() {
        return card.child(
            col()
                .key("no-runs")
                .w_full()
                .padding(16.0)
                .border_sides([1.0, 0.0, 0.0, 0.0], c.border_weak)
                .child(text("No runs yet.").font(k.ty.small).color(c.text_muted)),
        );
    }
    card = card.child(table_head(table.fanout, k));
    let lead = table.leader();
    let mut rank = 0;
    for r in &table.rows {
        let place = if r.too_few() {
            None
        } else {
            rank += 1;
            Some(rank)
        };
        let margin = lead
            .as_ref()
            .filter(|lead| std::ptr::eq(lead.row, r))
            .map(|lead| &lead.margin);
        card = card.child(contender_row(table, r, place, margin, k));
    }
    card
}

pub(super) fn page(board: &Board, risen: bool, k: &Kit) -> El {
    if board.runs == 0 {
        return scoreboard_empty(k);
    }
    // One column of class tables, so each figure column lines up down the whole page.
    let mut sheets = col().w_full().gap(16.0);
    for table in &board.tables {
        sheets = sheets.child(role_sheet(table, risen, k));
    }
    let page = col()
        .w_full()
        .max_w(PAGE_MAX_W)
        .self_center()
        .gap(28.0)
        .px(28.0)
        .py(22.0)
        .child(section("LEADING EACH ROLE", leaders(board, k), k))
        .child(section("CONTENDERS", roster(board, k), k))
        .child(sheets);
    col()
        .key("scoreboard-page")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .child(page)
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune::{Env, Runtime};

    #[test]
    fn a_class_lists_every_contender_and_blocks_figures_until_it_ranks() {
        for fanout in [false, true] {
            for runs in 0..=scoreboard::TOO_FEW {
                let table = scoreboard::Table {
                    role: "builder".into(),
                    criteria: String::new(),
                    fanout,
                    rows: vec![
                        scoreboard::Row {
                            agent: "a/agent".into(),
                            name: "Agent".into(),
                            runs,
                            rated: runs,
                            score_sum: 5.0 * runs as f64,
                            fanouts: if fanout { runs } else { 0 },
                            wins: if fanout { runs } else { 0 },
                            ..Default::default()
                        },
                        scoreboard::Row {
                            agent: "b/other".into(),
                            name: "Other agent".into(),
                            runs: 2,
                            rated: 2,
                            score_sum: 8.0,
                            fanouts: if fanout { 2 } else { 0 },
                            wins: 0,
                            ..Default::default()
                        },
                    ],
                    decisions: Default::default(),
                };
                let mut rt = Runtime::new(
                    table,
                    |table, _| role_sheet(table, true, rune_theme_opencode::kit(true)),
                    (720.0, 1000.0),
                    Env::new(true, 1.0),
                );
                rt.frame(0.0);
                let labels: Vec<_> = rt.elements().into_iter().filter_map(|e| e.text).collect();
                let has = |label: &str| labels.iter().any(|text| text == label);
                let ranked = runs >= scoreboard::TOO_FEW;
                let case = format!("{runs} runs, fanout={fanout}");
                assert!(has("BUILDER"));
                assert!(has("Agent") && has("Other agent"), "{case}");
                assert_eq!(has("WON"), fanout, "{case}");
                assert_eq!(has("unopposed"), ranked, "{case}: the leader's margin");
                let needed = scoreboard::TOO_FEW - runs.min(scoreboard::TOO_FEW);
                assert_eq!(
                    has(&format!(
                        "Blocked · {} to rank",
                        count(needed, "more run", "more runs")
                    )),
                    !ranked,
                    "{case}"
                );
                assert_eq!(has("5.00"), ranked, "{case}: a blocked score stays hidden");
                assert!(
                    has("Blocked · 1 more run to rank") && !has("4.00"),
                    "{case}: the other agent is a run short, its score hidden"
                );
            }
        }
    }

    #[test]
    fn a_class_tables_figures_stay_under_their_headers_as_the_card_narrows() {
        for table in crate::fixtures::board().tables {
            let rows: Vec<_> = table
                .rows
                .iter()
                .map(|r| format!("row-{}", r.agent))
                .collect();
            let figures = columns(table.fanout).len();
            for width in [580.0, 640.0, 720.0, 900.0] {
                let mut rt = Runtime::new(
                    table.clone(),
                    |table, _| role_sheet(table, true, rune_theme_opencode::kit(true)),
                    (width, 1000.0),
                    Env::new(true, 1.0),
                );
                rt.frame(0.0);
                let elements = rt.elements();
                let rect = |suffix: String| {
                    elements
                        .iter()
                        .find(|e| e.path.ends_with(&suffix))
                        .map(|e| e.rect)
                };
                let span = rect("/head/figures".into()).unwrap();
                for row in &rows {
                    let drifts = format!("{} at {width}pt: {row}", table.role);
                    // A blocked row's hatch covers the header's figure columns, edge to edge.
                    if let Some(blocked) = rect(format!("/{row}/blocked")) {
                        assert!((blocked[0] - span[0]).abs() < 0.5, "{drifts}'s left edge");
                        assert!((blocked[2] - span[2]).abs() < 0.5, "{drifts}'s width");
                        continue;
                    }
                    for column in 0..figures {
                        let right = |row: &str| {
                            let r = rect(format!("/{row}/figures/#{column}")).unwrap();
                            r[0] + r[2]
                        };
                        assert!(
                            (right("head") - right(row)).abs() < 0.5,
                            "{drifts}'s column {column} drifts from its header"
                        );
                    }
                }
            }
        }
    }
}
