//! View pieces of the chat window. Every handler reaches the shared core through `HasCore`.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use rune::{Color, El, GrowthSpec, Icon, ScrollSpec, SurfaceFit, Tx, col, el, row, spacer, text};
use rune_kit::diff::{self, DiffState, DiffView, FileDiff};
use rune_kit::{
    Badge, BadgeVariant, Button, ButtonVariant, ControlSize, IconButton, IconButtonVariant, Kit,
    Markdown, MarkdownStyle, Spinner,
};
use serde_json::json;

use crate::config;
use crate::core::{Core, DialogKind, Shared, lock, score_key};
use crate::transcript::{Block, FanAnswer, Item, Shot};
use crate::ui::dither::Motion;
use crate::ui::stage::{self, Fanout, Stage};
use crate::ui::design_system;

pub trait HasCore: 'static {
    fn core(&self) -> &Shared;
    /// The diff an `edit` or `write` tool call made, by call id, once the window has built it.
    fn diff(&self, id: &str) -> Option<&DiffState>;
    fn diff_mut(&mut self, id: &str) -> &mut DiffState;
    fn copied_code(&self) -> &HashMap<String, Instant>;
    fn mark_code_copied(&mut self, id: String);
    fn preview_sources(&self) -> &HashSet<String>;
    fn toggle_preview_source(&mut self, id: String);
    /// A prompt image, decoding or decoded; `None` where nothing decodes (headless tests).
    fn image(&self, _shot: &Shot) -> Option<rune::Image> {
        None
    }
    /// Opens an image full size, from a click on its tile.
    fn view_shot(&mut self, _shot: Shot) {}
    /// How the working chat's dot art moves.
    fn stage_motion(&self) -> Motion {
        Motion::Live
    }
    /// How far the scanner has resolved into the live thought; one is settled.
    fn thinking_merge(&self) -> f32 {
        1.0
    }
    /// Seconds since a fan-out lane last changed state.
    fn lane_age(&self, _label: &str) -> f32 {
        stage::SETTLED
    }
}

pub fn with_core<S: HasCore>(s: &S, f: impl FnOnce(&mut Core)) {
    f(&mut lock(s.core()));
}

pub fn chip(label: impl Into<String>, k: &Kit) -> El {
    Badge::new(label).variant(BadgeVariant::Subtle).build(k)
}

/// The surface under the transcript: the composer or a dialog, one at a time.
pub fn panel(k: &Kit, focused: bool) -> El {
    let c = &k.c;
    let border = if focused {
        c.border_strong
    } else {
        c.border_base
    };
    k.elevate(
        col()
            .w_full()
            .radius(k.radius.lg)
            .fill(c.surface_raised)
            .border(1.0, border),
        &k.elevation.raised,
    )
}

pub fn busy_line(core: &Core) -> Option<String> {
    if let Some(text) = core.statuses.get("compaction") {
        return Some(text.clone());
    }
    if let Some(text) = core.statuses.get("retry") {
        if core.reveal().is_none() {
            return Some("Provider error, retrying. Details after /done.".to_owned());
        }
        return Some(text.clone());
    }
    if let Some(line) = core.widgets.values().next().and_then(|lines| lines.first()) {
        let Some(fanout) = Fanout::parse(line) else {
            return Some(line.clone());
        };
        let mut status = format!(
            "Fan-out · {} of {} answered",
            fanout.answered(),
            fanout.lanes.len()
        );
        if let Some(elapsed) = fanout.elapsed {
            status = format!("{status} · {elapsed}");
        }
        return Some(status);
    }
    if core.pending_prompt.is_some() && core.t.week.deal.is_none() {
        return Some("Classifying and dealing an agent…".to_owned());
    }
    None
}

/// "8s", "1m 05s": the working timer, in tabular figures so it doesn't jitter.
fn elapsed(d: Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 {
        return format!("{secs}s");
    }
    format!("{}m {:02}s", secs / 60, secs % 60)
}

/// Sits on the composer while the agent works: what it's doing and for how long.
pub fn working(core: &Core, k: &Kit) -> Option<El> {
    let line = busy_line(core)?;
    let c = &k.c;
    let mut status = row()
        .key("working")
        .w_full()
        .gap(8.0)
        .px(14.0)
        .center_y()
        .child(Spinner::new().size(12.0).build(k))
        .child(
            text(line)
                .font(k.ty.small)
                .color(c.text_muted)
                .truncate()
                .shrink(),
        );
    if let Some(started) = core.turn_started {
        status = status.child(
            text(elapsed(started.elapsed()))
                .font(k.ty.small.features("'tnum' 1"))
                .color(c.text_faint),
        );
    }
    Some(status)
}

const TOOL_TAIL: usize = 8;
const SHELL_TAIL: usize = 20;
/// Shaping is the cost, so a tool line is cut to what a wide card can show before it's laid out.
const TOOL_LINE_MAX: usize = 400;
/// A diff past this scrolls inside its card: the header and sixteen lines.
const DIFF_MAX_H: f32 = 44.0 + 16.0 * 20.0;
/// A thought past twelve lines scrolls inside its block.
const THINKING_MAX_H: f32 = 12.0 * 20.0;

/// The one frame for monospace output: tool cards, `!cmd` output and fenced code.
fn mono_frame(k: &Kit) -> El {
    col()
        .w_full()
        .gap(6.0)
        .px(12.0)
        .py(10.0)
        .radius(k.radius.md)
        .fill(k.c.layer_01)
}

pub(super) const COPIED_FOR: Duration = Duration::from_millis(1500);

static COPY: Icon = Icon {
    name: "aw-copy",
    view: 16.0,
    body: r#"<rect x="5.5" y="5.5" width="7" height="8" rx="1.5" stroke="currentColor" stroke-width="1.2" fill="none"/><path d="M9.5 5.5V4A1.5 1.5 0 0 0 8 2.5H4A1.5 1.5 0 0 0 2.5 4V9A1.5 1.5 0 0 0 4 10.5H5.5" stroke="currentColor" stroke-width="1.2" stroke-linejoin="round" fill="none"/>"#,
};
static COPIED: Icon = Icon {
    name: "aw-copied",
    view: 16.0,
    body: r#"<path d="M3.5 8L6.5 11L12.5 5" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round" fill="none"/>"#,
};
static SOURCE: Icon = Icon {
    name: "aw-source",
    view: 16.0,
    body: r#"<path d="M5 4.5L1.5 8L5 11.5M11 4.5L14.5 8L11 11.5M9 3L7 13" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" stroke-linejoin="round" fill="none"/>"#,
};
static PREVIEW: Icon = Icon {
    name: "aw-preview",
    view: 16.0,
    body: r#"<path d="M1.5 8S4 3.5 8 3.5S14.5 8 14.5 8S12 12.5 8 12.5S1.5 8 1.5 8Z" stroke="currentColor" stroke-width="1.2" stroke-linejoin="round" fill="none"/><circle cx="8" cy="8" r="2" stroke="currentColor" stroke-width="1.2" fill="none"/>"#,
};

fn markdown<S: HasCore>(s: &S, body: String, key: String, k: &Kit) -> Markdown {
    let copied = s.copied_code().clone();
    let sources = s.preview_sources().clone();
    let diagram_copied = copied.clone();
    let diagram_sources = sources.clone();
    let diagram_key = key.clone();
    let diagram_kit = k.clone();
    let style = MarkdownStyle::from_kit(k)
        .code_block(k.c.layer_01, Color::TRANSPARENT)
        .code_block_font(k.ty.mono)
        .code_block_padding(12.0, 10.0);
    let k = k.clone();
    Markdown::new(body)
        .style(style)
        .selectable(true)
        .on_link_tx(|s: &mut S, url, tx| {
            let cwd = lock(s.core()).cwd.clone();
            match link_path(url, &cwd) {
                Some(path) => open_file(s, &path),
                None => {
                    if let Err(e) = tx.open_url(url) {
                        with_core(s, |core| core.toast(format!("{e}: {url}"), "warning"));
                    }
                }
            }
        })
        .diagram_frame(move |card, diagram| {
            let id = format!("{diagram_key}-code{}", diagram.index);
            let shown = diagram_sources.contains(&id);
            let body = if shown {
                mono_frame(&diagram_kit).child(
                    text(diagram.source.to_owned())
                        .font(diagram_kit.ty.mono)
                        .color(diagram_kit.c.text_base)
                        .wrap()
                        .selectable(true),
                )
            } else {
                card
            };
            fenced::<S>(
                body,
                Some("mermaid"),
                diagram.source,
                id.clone(),
                diagram_copied.contains_key(&id),
                Some(shown),
                &diagram_kit,
            )
        })
        .code_frame(move |card, code| {
            let id = format!("{key}-code{}", code.index);
            let copied = copied.contains_key(&id);
            let mut source_shown = None;
            let body = if code.language == Some("design-system") && !code.open {
                match design_system::render(code.source, &k) {
                    Ok(preview) => {
                        let shown = sources.contains(&id);
                        source_shown = Some(shown);
                        if shown { card } else { preview }
                    }
                    Err(error) => col()
                        .w_full()
                        .gap(6.0)
                        .child(
                            text(format!("Cannot preview design system: {error}"))
                                .font(k.ty.small)
                                .color(k.c.danger.fg)
                                .wrap()
                                .px(12.0),
                        )
                        .child(card),
                }
            } else {
                card
            };
            fenced::<S>(
                body,
                code.language,
                code.source,
                id,
                copied,
                source_shown,
                &k,
            )
        })
}

fn fenced<S: HasCore>(
    body: El,
    language: Option<&str>,
    source: &str,
    id: String,
    copied: bool,
    source_shown: Option<bool>,
    k: &Kit,
) -> El {
    let mut header = row().w_full().gap(4.0).px(12.0).pt(6.0).center_y();
    if let Some(language) = language {
        header = header.child(
            text(language.to_owned())
                .font(k.ty.small)
                .color(k.c.text_muted)
                .truncate()
                .shrink(),
        );
    }
    header = header.child(spacer());
    if let Some(shown) = source_shown {
        let toggle_id = id.clone();
        header = header.child(
            IconButton::new(if shown { &PREVIEW } else { &SOURCE })
                .variant(IconButtonVariant::GhostMuted)
                .label(if shown { "Show preview" } else { "Show source" })
                .help(if shown { "Show preview" } else { "Show source" })
                .on_press(move |s: &mut S| s.toggle_preview_source(toggle_id.clone()))
                .build(k)
                .key("preview-source"),
        );
    }
    let source = source.to_owned();
    let copy_id = id.clone();
    header = header.child(
        IconButton::new(if copied { &COPIED } else { &COPY })
            .variant(IconButtonVariant::GhostMuted)
            .label(if copied { "Copied" } else { "Copy code" })
            .help(if copied { "Copied" } else { "Copy code" })
            .on_press_tx(move |s: &mut S, tx| {
                tx.copy(source.clone());
                s.mark_code_copied(copy_id.clone());
                tx.wake_after(COPIED_FOR);
            })
            .build(k)
            .key("copy-code"),
    );
    col()
        .key(id)
        .w_full()
        .radius(k.radius.md)
        .fill(k.c.layer_01)
        .child(header)
        .child(body)
}

/// The file a link names, if it names one rather than a web page: an absolute or `~/` path, a
/// `file://` URL, or a path relative to the chat's folder. A `#L12` or `:12:3` suffix is
/// dropped when only the path without it exists.
fn link_path(url: &str, cwd: &Path) -> Option<PathBuf> {
    let url = url.trim();
    if let Some(rest) = url
        .get(..7)
        .filter(|p| p.eq_ignore_ascii_case("file://"))
        .map(|_| &url[7..])
    {
        let rest = rest.strip_prefix("localhost").unwrap_or(rest);
        return Some(PathBuf::from(percent_decode(
            rest.split(['?', '#']).next().unwrap_or(rest),
        )));
    }
    // A scheme has no dot, so `main.rs:12` stays a path.
    let scheme = url.split_once(':').is_some_and(|(s, _)| {
        s.len() > 1
            && s.starts_with(|c: char| c.is_ascii_alphabetic())
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-'))
    });
    if url.is_empty() || url.starts_with('#') || scheme {
        return None;
    }
    let full = |p: &str| match p.strip_prefix("~/") {
        Some(rest) => config::home().join(rest),
        None => cwd.join(p),
    };
    let path = full(url);
    if path.exists() {
        return Some(path);
    }
    let mut bare = url.split_once('#').map_or(url, |(p, _)| p);
    for _ in 0..2 {
        match bare.rsplit_once(':') {
            Some((p, n)) if !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) => bare = p,
            _ => break,
        }
    }
    let bare = full(bare);
    Some(if bare.exists() { bare } else { path })
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = b
            .get(i + 1..i + 3)
            .filter(|h| h.iter().all(u8::is_ascii_hexdigit));
        match hex {
            Some(h) if b[i] == b'%' => {
                out.push(u8::from_str_radix(std::str::from_utf8(h).unwrap(), 16).unwrap());
                i += 3;
            }
            _ => {
                out.push(b[i]);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// An image opens in the viewer; any other file or folder is shown in Finder (the file
/// manager on Linux) rather than opened, so a link can't launch an app or run a script.
fn open_file<S: HasCore>(s: &mut S, path: &Path) {
    if !path.exists() {
        with_core(s, |core| {
            core.toast(format!("No such file: {}", path.display()), "warning")
        });
        return;
    }
    let image = path.is_file()
        && path.extension().is_some_and(|e| {
            ["png", "jpg", "jpeg", "gif", "webp"]
                .iter()
                .any(|x| e.eq_ignore_ascii_case(x))
        });
    if image && let Ok(shot) = Shot::from_file(path) {
        s.view_shot(shot);
        return;
    }
    let opened = if cfg!(target_os = "macos") {
        std::process::Command::new("open")
            .arg("-R")
            .arg(path)
            .spawn()
    } else {
        let folder = if path.is_dir() {
            path
        } else {
            path.parent().unwrap_or(path)
        };
        std::process::Command::new("xdg-open").arg(folder).spawn()
    };
    if let Err(e) = opened {
        with_core(s, |core| {
            core.toast(format!("Couldn’t show {}: {e}", path.display()), "error")
        });
    }
}

/// The first `chars` characters of `line`.
fn first_chars(line: &str, chars: usize) -> &str {
    match line.char_indices().nth(chars) {
        Some((end, _)) => &line[..end],
        None => line,
    }
}

/// Drops the indent every line shares, as `git diff --stat` adds one.
fn dedent(output: &str) -> String {
    let lines: Vec<&str> = output.trim_end().lines().collect();
    let indent = lines
        .iter()
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.len() - line.trim_start_matches(' ').len())
        .min()
        .unwrap_or(0);
    lines
        .iter()
        .map(|line| line.get(indent..).unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The last `keep` lines of monospace output, one clipped row each, after a count of the rest.
/// With `error`, the last line shows in red and wraps to three lines.
fn output_tail(output: &str, keep: usize, error: bool, k: &Kit) -> El {
    let c = &k.c;
    let all: Vec<&str> = output.trim_end().lines().collect();
    let hidden = all.len().saturating_sub(keep);
    let shown = dedent(&all[hidden..].join("\n"));
    let lines: Vec<&str> = shown.lines().collect();
    let mut tail = col().w_full().gap(6.0);
    if hidden > 0 {
        tail = tail.child(
            text(format!(
                "… {hidden} earlier {}",
                if hidden == 1 { "line" } else { "lines" }
            ))
            .font(k.ty.small)
            .color(c.text_muted),
        );
    }
    let (body, last) = match lines.split_last() {
        Some((last, body)) if error => (body, Some(*last)),
        _ => (&lines[..], None),
    };
    let mut rows = col().w_full();
    for line in body {
        // One row per line, as a terminal tail: minified output is a few lines thousands wide.
        let line = if line.is_empty() {
            " "
        } else {
            first_chars(line, TOOL_LINE_MAX)
        };
        rows = rows.child(
            text(line.to_owned())
                .font(k.ty.mono)
                .color(c.text_muted)
                .w_full()
                .truncate()
                .selectable(true),
        );
    }
    if let Some(last) = last {
        rows = rows.child(
            text(first_chars(last, TOOL_LINE_MAX * 3).to_owned())
                .font(k.ty.mono)
                .color(c.danger.fg)
                .max_lines(3)
                .wrap_anywhere()
                .selectable(true),
        );
    }
    tail.child(rows)
}

/// A finished thought after the reveal: the whole trace, capped so it never walls the answer,
/// down a dotted rail.
fn thinking_block(body: &str, k: &Kit, key: String) -> El {
    let c = &k.c;
    let trace = col()
        .w_full()
        .max_h(THINKING_MAX_H)
        .pl(12.0 + stage::RAIL_W)
        .child(
            text(body.trim().to_owned())
                .font(k.ty.ui)
                .color(c.text_muted)
                .wrap()
                .selectable(true),
        )
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
        .scroll_edge_fade(16.0);
    col()
        .key(key)
        .w_full()
        .child(stage::rail(k).pin(Some(0.0), Some(0.0), None, Some(0.0)))
        .child(trace)
}

/// Paths in the diff header read from the session's folder.
fn relative(path: &str, cwd: &Path) -> String {
    Path::new(path)
        .strip_prefix(cwd)
        .map_or_else(|_| path.to_owned(), |p| p.to_string_lossy().into_owned())
}

/// The files a finished `edit` or `write` changed, for the diffs the window doesn't hold yet.
pub fn tool_diffs(core: &Core, known: impl Fn(&str) -> bool) -> Vec<(String, Vec<FileDiff>)> {
    let mut found = Vec::new();
    for item in &core.t.items {
        let Item::Assistant { blocks, .. } = item else {
            continue;
        };
        for block in blocks.iter().flatten() {
            let Block::Tool {
                id, args, written, ..
            } = block
            else {
                continue;
            };
            if known(id) {
                continue;
            }
            let Some(run) = core.t.tools.get(id).filter(|r| !r.running && !r.is_error) else {
                continue;
            };
            let files = match (&run.patch, written) {
                (Some(patch), _) => diff::parse(patch)
                    .into_iter()
                    .map(|mut file| {
                        file.old_path = file.old_path.map(|p| relative(&p, &core.cwd));
                        file.new_path = file.new_path.map(|p| relative(&p, &core.cwd));
                        file
                    })
                    .collect(),
                (None, Some(content)) => vec![FileDiff::between(
                    None,
                    Some(relative(args, &core.cwd)),
                    "",
                    content,
                )],
                (None, None) => continue,
            };
            found.push((id.clone(), files));
        }
    }
    found
}

/// Lines, hunk gaps and headers, as the diff view lays them out, up to the cap.
fn diff_height(st: &DiffState) -> f32 {
    let mut height = 0.0;
    for (f, file) in st.files().iter().enumerate() {
        height += 44.0;
        if st.collapsed(f) {
            continue;
        }
        let lines: usize = file.hunks.iter().map(|h| h.lines.len()).sum();
        let gaps = (0..=file.hunks.len())
            .filter(|&g| st.hidden(f, g).is_some_and(|n| n > 0))
            .count();
        height += lines as f32 * 20.0 + gaps as f32 * 32.0;
    }
    height.min(DIFF_MAX_H)
}

fn diff_card<S: HasCore>(s: &S, st: &DiffState, id: &str, k: &Kit) -> El {
    let get = id.to_owned();
    let get_mut = id.to_owned();
    let view = DiffView::new(
        move |s: &S| s.diff(&get).unwrap_or_else(|| empty_diff()),
        move |s: &mut S| s.diff_mut(&get_mut),
    )
    .key(format!("diff-{id}"))
    .build(s, k);
    col()
        .w_full()
        .h(diff_height(st))
        .radius(k.radius.md)
        .clip_rounded(k.radius.md)
        .border(1.0, k.c.border_weak)
        .child(view)
}

/// What a diff view reads once its session is gone, until the row it sat in is rebuilt.
fn empty_diff() -> &'static DiffState {
    thread_local! {
        static EMPTY: &'static DiffState = Box::leak(Box::default());
    }
    EMPTY.with(|empty| *empty)
}

fn tool_row(core: &Core, id: &str, name: &str, args: &str, k: &Kit) -> El {
    let c = &k.c;
    let run = core.t.tools.get(id);
    let running = run.is_none_or(|r| r.running);
    let mark: El = match run {
        _ if running => Spinner::new().size(12.0).build(k),
        Some(r) if r.is_error => text("✕").font(k.ty.small).color(c.danger.fg),
        _ => text("✓").font(k.ty.small).color(c.success.fg),
    };
    let head = row()
        .gap(8.0)
        .center_y()
        .child(mark)
        .child(
            text(name.to_owned())
                .font(k.ty.small_medium)
                .color(c.text_base),
        )
        .child(
            text(args.to_owned())
                .font(k.ty.mono)
                .color(c.text_muted)
                .truncate()
                .shrink()
                .selectable(true),
        );
    let mut card = mono_frame(k).child(head);
    if let Some(run) = run.filter(|r| !r.output.trim().is_empty()) {
        card = card.child(output_tail(&run.output, TOOL_TAIL, run.is_error, k));
    }
    card
}

/// An attached image, cropped to a square so a tile never changes size as it decodes. A
/// format the window can't draw (GIF, WebP) shows its name instead. A click opens it whole.
pub fn shot_tile<S: HasCore>(s: &S, shot: &Shot, side: f32, k: &Kit) -> El {
    let c = &k.c;
    let opened = shot.clone();
    let tile = col()
        .size(side, side)
        .center()
        .radius(k.radius.md)
        .fill(c.layer_01)
        .border(1.0, c.border_weak)
        .role("button")
        .label(format!("Open attached {} image", shot.kind()))
        .cursor(rune::Cursor::Pointer)
        .on_press(move |s: &mut S| s.view_shot(opened.clone()));
    match s.image(shot).filter(|image| image.error().is_none()) {
        Some(image) => tile.pixels_fit(image.pixels(), SurfaceFit::Fill),
        None => tile.child(text(shot.kind()).font(k.ty.small).color(c.text_muted)),
    }
}

const BUBBLE_SHOT: f32 = 96.0;

fn user_bubble<S: HasCore>(s: &S, prompt: &str, shots: &[Shot], k: &Kit) -> El {
    let mut column = col().items_end().gap(8.0).max_w(560.0);
    if !shots.is_empty() {
        column = column.child(row().gap(8.0).flex_wrap().justify_end().children(
            shots.iter().enumerate().map(|(index, shot)| {
                shot_tile(s, shot, BUBBLE_SHOT, k).key(format!("shot-{index}"))
            }),
        ));
    }
    if !prompt.trim().is_empty() {
        column = column.child(
            col()
                .px(14.0)
                .py(10.0)
                .radius(k.radius.lg)
                .fill(k.c.layer_02)
                .child(
                    text(prompt.to_owned())
                        .font(k.ty.body)
                        .color(k.c.text_base)
                        .wrap()
                        .selectable(true),
                ),
        );
    }
    row().w_full().justify_end().child(column)
}

fn assistant_view<S: HasCore>(
    s: &S,
    core: &Core,
    blocks: &[Option<Block>],
    streaming: bool,
    error: &Option<String>,
    k: &Kit,
    key: &str,
) -> El {
    let c = &k.c;
    let mut column = col().w_full().gap(10.0);
    let latest = blocks.iter().rposition(Option::is_some);
    for (index, block) in blocks.iter().enumerate() {
        match block {
            Some(Block::Text(body)) if !body.is_empty() => {
                let md = markdown(s, body.clone(), format!("{key}-md{index}"), k)
                    .streaming(streaming)
                    .build(k)
                    .selection_group(false)
                    .key(format!("{key}-md{index}"));
                column = column.child(md);
            }
            Some(Block::Thinking(body)) => {
                // The trace's style names the lab, so before the reveal it shows only as dot
                // art: written while it streams, then sealed at a size that never follows it.
                let think_key = format!("{key}-think{index}");
                if core.reveal().is_some() {
                    if !body.is_empty() {
                        column = column.child(thinking_block(body, k, think_key));
                    }
                } else if streaming && latest == Some(index) {
                    let thought =
                        stage::thinking(s.stage_motion(), s.thinking_merge(), k).key(think_key);
                    let follows_content =
                        blocks[..index].iter().flatten().any(|block| match block {
                            Block::Text(body) | Block::Thinking(body) => !body.is_empty(),
                            Block::Tool { .. } => true,
                        });
                    // Match the tail scanner's 18pt gap, rather than the column's 10pt gap.
                    column = column.child(if follows_content {
                        col().pt(8.0).child(thought)
                    } else {
                        thought
                    });
                } else if !body.is_empty() {
                    column = column.child(stage::sealed(index, k).key(think_key));
                }
            }
            Some(Block::Tool { id, name, args, .. }) => {
                column = column.child(match s.diff(id) {
                    Some(st) => diff_card(s, st, id, k),
                    None => tool_row(core, id, name, args, k),
                })
            }
            _ => {}
        }
    }
    let dealing = matches!(Stage::of(core), Some(Stage::Dealing { .. }));
    if streaming && latest.is_none() && !dealing {
        column = column.child(
            stage::thinking(s.stage_motion(), s.thinking_merge(), k).key(format!("{key}-think0")),
        );
    }
    if let Some(error) = error {
        let shown = if core.reveal().is_some() {
            format!("Error: {error}")
        } else {
            "Provider error. The message can name the lab, so it shows after /done.".to_owned()
        };
        column = column.child(text(shown).font(k.ty.small).color(c.danger.fg).wrap());
    }
    column
}

fn fanout_card<S: HasCore>(
    s: &S,
    core: &Core,
    answer: &FanAnswer,
    picked: &Option<String>,
    key: &str,
    k: &Kit,
) -> El {
    let c = &k.c;
    let choosing = core.dialog.as_ref().and_then(|d| match &d.kind {
        DialogKind::Select(options) => options.iter().position(|o| o == &answer.label),
        _ => None,
    });
    let won = picked.as_deref() == Some(answer.label.as_str());
    let mut head = row().w_full().gap(8.0).center_y().child(
        text(answer.label.clone())
            .font(k.ty.body_medium)
            .color(c.text_base),
    );
    if won {
        head = head.child(chip("picked", k));
    }
    if !answer.ok {
        head = head.child(chip("failed", k));
    }
    head = head.child(spacer());
    if let Some(index) = choosing {
        head = head.child(
            Button::new(format!("Pick {}", answer.label))
                .variant(ButtonVariant::Neutral)
                .control_size(ControlSize::Small)
                .shortcut(format!("{}", index + 1))
                .show_shortcut()
                .on_press(move |s: &mut S| with_core(s, |core| core.pick(index)))
                .build(k),
        );
    }
    let body: El = match (&answer.error, answer.text.is_empty()) {
        (Some(error), true) => text(error.clone())
            .font(k.ty.small)
            .color(c.danger.fg)
            .wrap(),
        _ => markdown(
            s,
            answer.text.clone(),
            format!("{key}-fan-{}", answer.label),
            k,
        )
        .build(k),
    };
    col()
        .key(format!("fan-{}", answer.label))
        .grow(1.0)
        .min_w(0.0)
        .gap(10.0)
        .padding(14.0)
        .radius(k.radius.lg)
        .fill(c.layer_01)
        .border(
            1.0,
            if won {
                c.border_strong
            } else {
                Color::TRANSPARENT
            },
        )
        .child(head)
        .child(body)
}

/// `prompt` is the request's bubble, once the transcript has it.
fn fanout_view<S: HasCore>(
    s: &S,
    core: &Core,
    prompt: Option<El>,
    answers: &[FanAnswer],
    picked: &Option<String>,
    key: &str,
    k: &Kit,
) -> El {
    let mut column = col().w_full().gap(12.0);
    if let Some(prompt) = prompt {
        column = column.child(prompt);
    }
    let note = match picked {
        Some(label) => {
            format!("Fan-out: you picked {label}. Follow-ups go to the same agent.")
        }
        None => "Fan-out: every contender answered. Pick the best before you see who wrote it."
            .to_owned(),
    };
    column = column.child(text(note).font(k.ty.small).color(k.c.text_muted));
    let cards = answers
        .iter()
        .map(|a| fanout_card(s, core, a, picked, key, k));
    column.child(row().w_full().gap(12.0).items_start().children(cards))
}

fn item_view<S: HasCore>(s: &S, core: &Core, item: &Item, key: &str, k: &Kit) -> El {
    let c = &k.c;
    let view = match item {
        Item::User { text, shots } => user_bubble(s, text, shots, k),
        Item::Assistant {
            blocks,
            streaming,
            error,
        } => assistant_view(s, core, blocks, *streaming, error, k, key),
        Item::Bash {
            command,
            output,
            exit_code,
            running,
        } => {
            let status = match (running, exit_code) {
                (true, _) => "running".to_owned(),
                (false, Some(code)) => format!("exit {code}"),
                (false, None) => "done".to_owned(),
            };
            mono_frame(k)
                .child(
                    row()
                        .gap(8.0)
                        .center_y()
                        .child(
                            text(format!("$ {command}"))
                                .font(k.ty.mono)
                                .color(c.text_base)
                                .truncate()
                                .shrink(),
                        )
                        .child(spacer())
                        .child(text(status).font(k.ty.small).color(c.text_muted)),
                )
                .child(output_tail(output, SHELL_TAIL, false, k))
        }
        Item::Fanout {
            prompt,
            shots,
            answers,
            picked,
        } => {
            let bubble = prompt.as_ref().map(|p| user_bubble(s, p, shots, k));
            fanout_view::<S>(s, core, bubble, answers, picked, key, k)
        }
        Item::Notice(note) => row()
            .w_full()
            .gap(12.0)
            .center_y()
            .child(el().h(1.0).grow(1.0).fill(c.border_base))
            .child(text(note.clone()).font(k.ty.small).color(c.text_muted))
            .child(el().h(1.0).grow(1.0).fill(c.border_base)),
    };
    view.key(key.to_owned())
}

/// The transcript as rows of a growth stack: only the rows on screen are built.
pub fn transcript<S: HasCore>(core: &Core, k: &'static Kit, max_w: f32) -> El {
    let count = core.t.items.len()
        + usize::from(core.pending_prompt.is_some())
        + usize::from(Stage::of(core).is_some_and(|stage| stage.shows()));
    let session = session_tag(core);
    // One selection over the whole chat, as on a web page: a drag runs from the prompt through
    // the answer and its tool output, and Cmd+A takes every row, built or not.
    col()
        .key("items")
        .w_full()
        .growth(GrowthSpec::default())
        .selection_group(true)
        .virtual_rows_version(session)
        .virtual_rows(
            count,
            120.0,
            |s: &S, i| Cow::Owned(format!("item-{:x}-{i}", session_tag(&lock(s.core())))),
            move |s: &S, i| {
                let core = lock(s.core());
                let messages = core.t.items.len() + usize::from(core.pending_prompt.is_some());
                let stage = Stage::of(&core).filter(Stage::shows);
                let last = i + 1 == messages + usize::from(stage.is_some());
                let body = match (core.t.items.get(i), &core.pending_prompt) {
                    (Some(item), _) => item_view::<S>(s, &core, item, &format!("item-{i}"), k),
                    (None, Some(prompt)) if i < messages => {
                        user_bubble(s, prompt, &core.pending_shots, k).key("pending-prompt")
                    }
                    _ => stage.map_or_else(col, |stage| {
                        stage::view(&stage, s.stage_motion(), |label| s.lane_age(label), k)
                    }),
                };
                col()
                    .w_full()
                    .items_center()
                    .px(24.0)
                    .pt(if i == 0 { 20.0 } else { 0.0 })
                    .pb(if last { 20.0 } else { 18.0 })
                    .child(col().w_full().max_w(max_w).child(body))
            },
        )
}

fn session_tag(core: &Core) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    core.session_file.hash(&mut hasher);
    hasher.finish()
}

pub fn dialog<S: HasCore>(
    core: &Core,
    k: &Kit,
    edit: Option<&rune::TextEdit>,
    edit_access: fn(&mut S) -> &mut rune::TextEdit,
) -> Option<El> {
    let dialog = core.dialog.as_ref()?;
    let c = &k.c;
    if let DialogKind::Select(options) = &dialog.kind
        && picks_on_cards(core, options)
    {
        let last = options.len().min(9);
        return Some(
            text(format!(
                "Press 1–{last} or a card’s Pick button · esc to cancel"
            ))
            .key("dialog")
            .font(k.ty.small)
            .color(c.text_muted),
        );
    }
    let mut card = panel(k, false).key("dialog").gap(12.0).padding(16.0);
    card = card.child(
        text(if dialog.ends_run() {
            "Call it done?".to_owned()
        } else {
            dialog.title.clone()
        })
        .font(k.ty.body_medium)
        .color(c.text_base)
        .wrap(),
    );
    match &dialog.kind {
        DialogKind::Select(options) => {
            let keycaps = options.iter().all(|o| o.chars().count() <= 2);
            let ends_run = dialog.ends_run();
            let mut list = if keycaps {
                row().gap(8.0).flex_wrap()
            } else {
                col().gap(2.0).mx(-8.0)
            };
            let digit = k.ty.small.features("'tnum' 1");
            for (index, option) in options.iter().enumerate() {
                let on = Some(index) == dialog.highlight;
                let (hint, label) = match score_key(option) {
                    Some((key, label)) if ends_run => (key.to_owned(), label.to_owned()),
                    _ if index < 9 => (format!("{}", index + 1), option.clone()),
                    _ => (String::new(), option.clone()),
                };
                let ink = if on { c.text_base } else { c.text_muted };
                let mut item = row()
                    .key(format!("option-{index}"))
                    .gap(10.0)
                    .px(8.0)
                    .py(7.0)
                    .center_y()
                    .radius(k.radius.md);
                if on {
                    item = item.fill(c.layer_02);
                }
                if !hint.is_empty() {
                    item = item.child(text(hint).font(digit).color(ink).w(10.0));
                }
                let item = k
                    .interactive(
                        item.child(text(label).font(k.ty.ui_medium).color(ink).wrap().shrink()),
                    )
                    .on_press(move |s: &mut S| {
                        with_core(s, |core| {
                            if ends_run {
                                core.choose(index);
                            } else {
                                core.pick(index);
                            }
                        })
                    });
                list = list.child(if keycaps { item } else { item.w_full() });
            }
            if ends_run {
                card = card.child(
                    text("Score the work before the reveal. Ending seals this chat; you can’t send more prompts.")
                        .font(k.ty.ui)
                        .color(c.text_muted)
                        .wrap(),
                );
            }
            card = card.child(list);
            if ends_run {
                card = card
                    .child(
                        text("5–1 to score · ⏎ to confirm")
                            .font(k.ty.small)
                            .color(c.text_muted),
                    )
                    .child(
                        row()
                            .w_full()
                            .gap(8.0)
                            .justify_end()
                            .child(
                                Button::new("Keep working")
                                    .variant(ButtonVariant::Ghost)
                                    .shortcut("esc")
                                    .show_shortcut()
                                    .on_press(|s: &mut S| {
                                        with_core(s, |core| core.answer_dialog(None))
                                    })
                                    .build(k)
                                    .key("keep-working"),
                            )
                            .child(
                                Button::new("I am done")
                                    .variant(ButtonVariant::Contrast)
                                    .shortcut("⏎")
                                    .show_shortcut()
                                    .disabled(dialog.highlight.is_none())
                                    .on_press(|s: &mut S| with_core(s, |core| core.commit()))
                                    .build(k)
                                    .key("confirm-end"),
                            ),
                    );
            } else {
                card = card.child(
                    text("Number keys or ↑↓ ⏎ to choose · esc to cancel")
                        .font(k.ty.small)
                        .color(c.text_muted),
                );
            }
        }
        DialogKind::Confirm(message) => {
            card = card
                .child(
                    text(message.clone())
                        .font(k.ty.body)
                        .color(c.text_muted)
                        .wrap(),
                )
                .child(
                    row()
                        .gap(8.0)
                        .justify_end()
                        .child(
                            Button::new("No")
                                .variant(ButtonVariant::Ghost)
                                .shortcut("N")
                                .show_shortcut()
                                .on_press(|s: &mut S| {
                                    with_core(s, |core| core.answer_dialog(Some(json!(false))))
                                })
                                .build(k),
                        )
                        .child(
                            Button::new("Yes")
                                .variant(ButtonVariant::Contrast)
                                .shortcut("⏎")
                                .show_shortcut()
                                .on_press(|s: &mut S| {
                                    with_core(s, |core| core.answer_dialog(Some(json!(true))))
                                })
                                .build(k),
                        ),
                );
        }
        DialogKind::Input {
            placeholder,
            multiline,
            ..
        } => {
            if let Some(edit) = edit {
                let field = rune_kit::TextArea::new(edit)
                    .plain()
                    .placeholder(placeholder.clone())
                    .width(None)
                    .auto_grow(if *multiline { 4 } else { 1 }, 12)
                    .on_edit(edit_access)
                    .on_submit(move |s: &mut S, _tx: &mut Tx| {
                        let value = edit_access(s).text.clone();
                        with_core(s, |core| core.answer_dialog(Some(json!(value))));
                        edit_access(s).clear();
                    })
                    .build(k);
                let framed = col()
                    .w_full()
                    .px(4.0)
                    .radius(k.radius.md)
                    .fill(c.bg_base)
                    .border(1.0, c.border_strong)
                    .child(field);
                card = card.child(framed).child(
                    text("⏎ to submit · esc to cancel")
                        .font(k.ty.small)
                        .color(c.text_muted),
                );
            }
        }
    }
    Some(card)
}

/// The open select picks among the cards of the fan-out still waiting for a pick.
fn picks_on_cards(core: &Core, options: &[String]) -> bool {
    core.t.items.iter().rev().find_map(|item| match item {
        Item::Fanout {
            answers,
            picked: None,
            ..
        } => Some(
            answers.len() == options.len()
                && answers.iter().zip(options).all(|(a, o)| &a.label == o),
        ),
        _ => None,
    }) == Some(true)
}

pub fn toasts(core: &Core, k: &Kit) -> Option<El> {
    if core.toasts.is_empty() && core.exited.is_none() {
        return None;
    }
    let mut column = col().key("toasts").gap(6.0).w_full();
    if let Some(exit) = &core.exited {
        let detail = if core.reveal().is_some() || core.t.week.deal.is_none() {
            exit.as_str()
        } else {
            "Details after /done."
        };
        column = column.child(note_row(&format!("pi stopped. {detail}"), "error", k));
    }
    for toast in core.toasts.iter().rev().take(3) {
        column = column.child(note_row(&toast.text, &toast.level, k));
    }
    Some(column)
}

fn note_row(message: &str, level: &str, k: &Kit) -> El {
    let c = &k.c;
    let (fill, ink) = match level {
        "error" => (c.danger.bg, c.danger.fg),
        "warning" => (c.warning.bg, c.warning.fg),
        _ => (c.layer_02, c.text_base),
    };
    row()
        .w_full()
        .px(12.0)
        .py(8.0)
        .radius(k.radius.md)
        .fill(fill)
        .child(text(message.to_owned()).font(k.ty.small).color(ink).wrap())
}

pub fn queue_line(core: &Core, k: &Kit) -> Option<El> {
    let (steering, follow) = &core.queue;
    if steering.is_empty() && follow.is_empty() {
        return None;
    }
    let mut parts = Vec::new();
    if !steering.is_empty() {
        parts.push(format!("{} steering", steering.len()));
    }
    if !follow.is_empty() {
        parts.push(format!("{} follow-up", follow.len()));
    }
    Some(
        text(format!("Queued: {}", parts.join(", ")))
            .font(k.ty.small)
            .color(k.c.text_muted),
    )
}
