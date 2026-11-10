//! The chat page: the transcript, then the composer, a dialog in its place, or the ended panel.

use rune::{El, ScrollSpec, col, row, text};
use rune_kit::{Button, ButtonVariant, ControlSize, Kit};

use super::composer::{self, attach, is_shot_file};
use super::{Window, request_end};
use crate::core::{Core, Leave, cmd};
use crate::transcript::Shot;
use crate::ui::{dither, parts, seal};

/// One reading column for chat and anonymous fan-out answers.
pub(super) const COLUMN: f32 = 820.0;

/// Sits on the composer, inset to its text, so reading it leads straight into typing.
fn empty_intro(k: &Kit) -> El {
    let c = &k.c;
    col()
        .key("empty-intro")
        .w_full()
        .gap(4.0)
        .px(14.0)
        .pb(6.0)
        .child(
            text("One task per chat. As many chats as you need.")
                .font(k.ty.body_medium)
                .color(c.text_base),
        )
        .child(
            text(format!("Start another task with {} while this one works. End each run with {} to score it and reveal the role and agent.", cmd("N"), cmd("⏎")))
                .font(k.ty.ui)
                .color(c.text_muted)
                .wrap(),
        )
}

/// A sealed session takes no more input, so the composer gives way to the next step.
fn ended(k: &Kit) -> El {
    let c = &k.c;
    parts::panel(k, false).key("ended").padding(16.0).child(
        row()
            .gap(16.0)
            .center_y()
            .child(
                col()
                    .gap(4.0)
                    .grow(1.0)
                    .shrink()
                    .min_w(0.0)
                    .child(
                        text("Done & dusted.")
                            .font(k.ty.body_medium)
                            .color(c.text_base),
                    )
                    .child(
                        text("This chat is scored and sealed. Start another task or return to an open chat.")
                            .font(k.ty.small)
                            .color(c.text_muted)
                            .wrap(),
                    ),
            )
            .child(
                Button::new("New task")
                    .variant(ButtonVariant::Contrast)
                    .shortcut(cmd("N"))
                    .show_shortcut()
                    .on_press_tx(|s: &mut Window, tx| {
                        s.leave(Leave::New, tx);
                        s.draft.composer.focused = true;
                    })
                    .build(k),
            ),
    )
}

/// No message yet: the intro and the shoreline show.
pub(super) fn blank(core: &Core) -> bool {
    core.t.items.is_empty() && core.pending_prompt.is_none()
}

pub(super) fn build(s: &Window, core: &Core, k: &'static Kit, dark: bool) -> El {
    let width = COLUMN;
    let empty = blank(core);
    let choice = parts::fanout_choice(core);
    let body: El = if let Some(choice) = &choice {
        col().w_full().items_center().px(24.0).py(20.0).child(
            col()
                .w_full()
                .max_w(width)
                .child(parts::fanout_reading(s, choice, k)),
        )
    } else if empty {
        col()
    } else {
        parts::transcript::<Window>(core, k, width)
    };
    // A fresh scroll area starts each contender at the top, even after a long answer.
    let scroll_key = choice.as_ref().map_or_else(
        || "transcript".to_owned(),
        |choice| {
            format!(
                "fanout-reading-{}-{}-{}",
                core.task_id(),
                choice.generation,
                choice.index
            )
        },
    );
    let scroll = col()
        .key(scroll_key)
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .child(body)
        .scroll(ScrollSpec {
            stick_bottom: choice.is_none(),
        });
    let mut bottom = col()
        .key("dock")
        .w_full()
        .max_w(width)
        .gap(10.0)
        .on_layout(|s: &mut Window, rect| s.dock_rect = Some(rect));
    if empty {
        bottom = bottom.child(empty_intro(k));
    }
    if let Some(toasts) = parts::toasts(core, k) {
        bottom = bottom.child(toasts);
    }
    // A dialog takes the composer's place, so keys have one owner.
    if let Some(dialog) =
        parts::dialog::<Window>(core, k, Some(&s.draft.dialog_edit), |s: &mut Window| {
            &mut s.draft.dialog_edit
        })
    {
        bottom = bottom.child(dialog);
    } else if core.t.week.done.is_some() {
        bottom = bottom.child(ended(k));
    } else {
        bottom = bottom.child(composer::build(s, core, k));
        if core.can_end_run() {
            bottom = bottom.child(
                row()
                    .key("finish-run")
                    .w_full()
                    .center_y()
                    .gap(12.0)
                    .pl(14.0)
                    .child(
                        text("Call it done?")
                            .font(k.ty.ui)
                            .color(k.c.text_muted)
                            .grow(1.0)
                            .shrink(),
                    )
                    .child(
                        Button::new("I am done")
                            .variant(ButtonVariant::Contrast)
                            .control_size(ControlSize::Large)
                            .shortcut(cmd("⏎"))
                            .show_shortcut()
                            .on_press_tx(request_end)
                            .build(k)
                            .key("end-run"),
                    ),
            );
        }
    }
    let mut stage = col().w_full().h_full().min_h(0.0).clip_rounded(0.0);
    // Images dropped anywhere on an open chat go to its composer.
    if core.dialog.is_none() && core.t.week.done.is_none() {
        stage = stage
            .on_drop_files(|s: &mut Window, files, _| {
                s.dropping = false;
                attach(s, files.iter().map(|p| Shot::from_file(p)));
            })
            .accepts_files(is_shot_file)
            .on_drop_hover(|s: &mut Window, over, _| s.dropping = over);
    }
    if empty {
        stage = stage.child(dither::slot(dark, s.shore_motion()));
    } else if choice.is_none() && s.tide.level() > 0.0 {
        let dock = s
            .tide_rect
            .zip(s.dock_rect)
            .map(|([x, y, ..], [dx, dy, ..])| [dx - x, dy - y]);
        stage = stage.child(
            dither::tide(dark, s.shore_motion(), s.tide.level(), dock)
                .on_layout(|s: &mut Window, rect| s.tide_rect = Some(rect)),
        );
    }
    stage = stage.child(scroll).child(
        col()
            .w_full()
            .items_center()
            .px(24.0)
            .pb(18.0)
            .pt(8.0)
            .child(bottom),
    );
    if let Some(progress) = s.seal.progress()
        && s.shore_motion() != dither::Motion::Still
    {
        stage = stage.child(seal::slot(progress, k.c.text_accent));
    }
    stage
}
