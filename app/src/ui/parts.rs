//! View pieces of the chat window. Every handler reaches the shared core through `HasCore`.

use std::borrow::Cow;
use std::hash::{Hash, Hasher};

use rune::{Color, El, GrowthSpec, Tx, col, el, row, spacer, text};
use rune_kit::{
    Badge, BadgeVariant, Button, ButtonVariant, ControlSize, Kit, Markdown, MarkdownStyle, Spinner,
};
use serde_json::json;

use crate::core::{Core, DialogKind, Shared, lock, score_key};
use crate::transcript::{Block, FanAnswer, Item};

pub trait HasCore: 'static {
    fn core(&self) -> &Shared;
}

pub fn with_core<S: HasCore>(s: &S, f: impl FnOnce(&mut Core)) {
    f(&mut lock(s.core()));
}

pub fn chip(label: impl Into<String>, k: &Kit) -> El {
    Badge::new(label).variant(BadgeVariant::Subtle).build(k)
}

/// The surface under the transcript: the composer, a dialog or the unrated guard, one at a time.
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

/// Readable measure for prose; code, tables and fan-out cards keep the full column.
const PROSE_WIDTH: f32 = 480.0;

/// Nothing about the deal shows until the run has ended, then "builder · M · it was Opus 5.5".
pub fn role_line(core: &Core) -> Option<String> {
    let Some(deal) = &core.t.week.deal else {
        return core.pending_prompt.as_ref().map(|_| "dealing…".to_owned());
    };
    let name = core.reveal()?;
    Some(format!("{} · {} · it was {name}", deal.role, deal.size))
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
    if let Some(lines) = core.widgets.values().next() {
        return lines.first().cloned();
    }
    if core.pending_prompt.is_some() && core.t.week.deal.is_none() {
        return Some("Classifying and dealing an agent…".to_owned());
    }
    if !core.streaming {
        return None;
    }
    let tool_running = core.t.items.iter().any(|item| {
        let Item::Assistant { blocks, .. } = item else {
            return false;
        };
        blocks.iter().flatten().any(|block| match block {
            Block::Tool { id, .. } => core.t.tools.get(id).is_none_or(|run| run.running),
            _ => false,
        })
    });
    if tool_running {
        return None;
    }
    Some("Working…".to_owned())
}

const TOOL_TAIL: usize = 8;

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

fn markdown(body: String, k: &Kit) -> Markdown {
    let style = MarkdownStyle::from_kit(k)
        .code_block(k.c.layer_01, Color::TRANSPARENT)
        .code_block_font(k.ty.mono)
        .code_block_padding(12.0, 10.0)
        .prose_width(PROSE_WIDTH);
    Markdown::new(body).style(style).selectable(true)
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

fn thinking_row(k: &Kit) -> El {
    row()
        .gap(8.0)
        .center_y()
        .child(Spinner::new().size(14.0).build(k))
        .child(text("Thinking…").font(k.ty.small).color(k.c.text_muted))
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
                .shrink(),
        );
    let mut card = mono_frame(k).child(head);
    if let Some(run) = run.filter(|r| !r.output.trim().is_empty()) {
        let output = dedent(&run.output);
        let lines: Vec<&str> = output.lines().collect();
        let hidden = lines.len().saturating_sub(TOOL_TAIL);
        if hidden > 0 {
            card = card.child(
                text(format!(
                    "… {hidden} earlier {}",
                    if hidden == 1 { "line" } else { "lines" }
                ))
                .font(k.ty.small)
                .color(c.text_muted),
            );
        }
        let shown = &lines[hidden..];
        let (body, last) = match shown.split_last() {
            Some((last, body)) if run.is_error => (body, Some(*last)),
            _ => (shown, None),
        };
        let mut output = col().w_full();
        if !body.is_empty() {
            output = output.child(
                text(body.join("\n"))
                    .font(k.ty.mono)
                    .color(c.text_muted)
                    .wrap()
                    .selectable(true),
            );
        }
        if let Some(last) = last {
            output = output.child(
                text(last.to_owned())
                    .font(k.ty.mono)
                    .color(c.danger.fg)
                    .wrap()
                    .selectable(true),
            );
        }
        card = card.child(output);
    }
    card
}

fn user_bubble(prompt: &str, k: &Kit) -> El {
    row().w_full().justify_end().child(
        col()
            .max_w(560.0)
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
    )
}

fn assistant_view(
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
                let md = markdown(body.clone(), k)
                    .streaming(streaming)
                    .build(k)
                    .key(format!("{key}-md{index}"));
                column = column.child(md);
            }
            Some(Block::Thinking(body)) if !body.is_empty() => {
                // The trace's style names the lab, so before the reveal it only shows while it streams.
                if core.reveal().is_some() {
                    let first = body.lines().next().unwrap_or("").to_owned();
                    column =
                        column.child(text(first).font(k.ty.small).color(c.text_muted).truncate());
                } else if streaming && latest == Some(index) {
                    column = column.child(thinking_row(k));
                }
            }
            Some(Block::Tool { id, name, args }) => {
                column = column.child(tool_row(core, id, name, args, k))
            }
            _ => {}
        }
    }
    if streaming && latest.is_none() {
        column = column.child(thinking_row(k));
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
    core: &Core,
    answer: &FanAnswer,
    picked: &Option<String>,
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
        _ => markdown(answer.text.clone(), k).build(k),
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

fn fanout_view<S: HasCore>(
    core: &Core,
    prompt: &Option<String>,
    answers: &[FanAnswer],
    picked: &Option<String>,
    k: &Kit,
) -> El {
    let mut column = col().w_full().gap(12.0);
    if let Some(prompt) = prompt {
        column = column.child(user_bubble(prompt, k));
    }
    let note = match picked {
        Some(label) => {
            format!("Fan-out: you picked {label}. Follow-ups go to the same agent.")
        }
        None => "Fan-out: every contender answered. Pick the best before you see who wrote it."
            .to_owned(),
    };
    column = column.child(text(note).font(k.ty.small).color(k.c.text_muted));
    let cards = answers.iter().map(|a| fanout_card::<S>(core, a, picked, k));
    column.child(row().w_full().gap(12.0).items_start().children(cards))
}

fn item_view<S: HasCore>(core: &Core, item: &Item, key: &str, k: &Kit) -> El {
    let c = &k.c;
    let view = match item {
        Item::User(prompt) => user_bubble(prompt, k),
        Item::Assistant {
            blocks,
            streaming,
            error,
        } => assistant_view(core, blocks, *streaming, error, k, key),
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
                .child(
                    text(dedent(output))
                        .font(k.ty.mono)
                        .color(c.text_muted)
                        .wrap()
                        .selectable(true),
                )
        }
        Item::Fanout {
            prompt,
            answers,
            picked,
        } => fanout_view::<S>(core, prompt, answers, picked, k),
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
    let count = core.t.items.len() + usize::from(core.pending_prompt.is_some());
    let session = session_tag(core);
    col()
        .key("items")
        .w_full()
        .growth(GrowthSpec::default())
        .virtual_rows_version(session)
        .virtual_rows(
            count,
            120.0,
            |s: &S, i| Cow::Owned(format!("item-{:x}-{i}", session_tag(&lock(s.core())))),
            move |s: &S, i| {
                let core = lock(s.core());
                let last = i + 1 == core.t.items.len() + usize::from(core.pending_prompt.is_some());
                let body = match core.t.items.get(i) {
                    Some(item) => item_view::<S>(&core, item, &format!("item-{i}"), k),
                    None => core.pending_prompt.as_deref().map_or_else(
                        || col(),
                        |prompt| user_bubble(prompt, k).key("pending-prompt"),
                    ),
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
        text(dialog.title.clone())
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
            card = card.child(list);
            if ends_run {
                card = card.child(
                    row()
                        .w_full()
                        .gap(8.0)
                        .center_y()
                        .child(
                            text("Press 5–1, then ⏎ seals the session and reveals the agent · esc keeps working")
                                .font(k.ty.small)
                                .color(c.text_muted)
                                .wrap()
                                .shrink(),
                        )
                        .child(spacer())
                        .child(
                            Button::new("End run")
                                .variant(ButtonVariant::Contrast)
                                .shortcut("⏎")
                                .show_shortcut()
                                .disabled(dialog.highlight.is_none())
                                .on_press(|s: &mut S| with_core(s, |core| core.commit()))
                                .build(k),
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

pub fn leave_guard<S: HasCore>(core: &Core, k: &Kit) -> Option<El> {
    core.leave.as_ref()?;
    let c = &k.c;
    Some(
        panel(k, false)
            .key("leave-guard")
            .gap(12.0)
            .padding(16.0)
            .child(text("This run hasn’t ended").font(k.ty.body_medium).color(c.text_base))
            .child(text("House rule 4: end every run with ⌘D. A run left open shows on the scoreboard as unrated.").font(k.ty.small).color(c.text_muted).wrap())
            .child(
                row()
                    .gap(8.0)
                    .justify_end()
                    .child(Button::new("Stay").variant(ButtonVariant::Ghost).shortcut("esc").show_shortcut().on_press(|s: &mut S| with_core(s, |core| core.answer_leave(None))).build(k))
                    .child(Button::new("Leave unrated").variant(ButtonVariant::Outline).on_press(|s: &mut S| with_core(s, |core| core.answer_leave(Some(false)))).build(k))
                    .child(Button::new("End it now").variant(ButtonVariant::Contrast).shortcut("⏎").show_shortcut().disabled(core.streaming).on_press(|s: &mut S| with_core(s, |core| core.answer_leave(Some(true)))).build(k).default_button()),
            ),
    )
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
