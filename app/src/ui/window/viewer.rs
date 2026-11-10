//! An image opened full size from its tile, over the whole window.

use rune::{El, col, el, row, text};
use rune_kit::{Button, ButtonVariant, ControlSize, Kit};

use super::Window;
use crate::core::lock;
use crate::ui::parts::HasCore;

pub(super) fn close(s: &mut Window) {
    s.viewing = None;
    s.draft.composer.focused = true;
}

/// An image opened from its tile: as large as the window allows, never past its own size, with
/// its format and size and a way to open it in the system's viewer to zoom. Esc, a click on
/// it or a click beside it closes it.
pub(super) fn build(s: &Window, k: &Kit, layer: &mut Vec<El>) {
    let Some(shot) = &s.viewing else {
        return;
    };
    let c = &k.c;
    let image = s.image(shot).filter(|image| image.error().is_none());
    let size = shot.dimensions();
    let picture = match &image {
        Some(image) => {
            let mut picture = el()
                .key("viewer-image")
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .pixels_fit(image.pixels(), rune::SurfaceFit::Fit)
                .label(format!("{} image", shot.kind()))
                .on_press(close);
            // Never blown up past one image pixel per point.
            if let Some((w, h)) = size {
                picture = picture.max_w(w as f32).max_h(h as f32);
            }
            picture
        }
        None => col().key("viewer-image").padding(24.0).child(
            text(format!("This {} can’t be shown here.", shot.kind()))
                .font(k.ty.ui)
                .color(c.text_muted),
        ),
    };
    let about = match size {
        Some((w, h)) => format!("{} · {w} × {h}", shot.kind()),
        None => shot.kind().to_owned(),
    };
    let open_label = if cfg!(target_os = "macos") {
        "Open in Preview"
    } else {
        "Open"
    };
    let bar = row()
        .key("viewer-bar")
        .gap(12.0)
        .center_y()
        .pl(12.0)
        .pr(4.0)
        .py(4.0)
        .radius(k.radius.lg)
        .fill(c.surface_raised)
        .border(1.0, c.border_weak)
        .child(
            text(about)
                .font(k.ty.small.features("'tnum' 1"))
                .color(c.text_muted),
        )
        .child(
            Button::new(open_label)
                .variant(ButtonVariant::Ghost)
                .control_size(ControlSize::Small)
                .on_press(|s: &mut Window| {
                    let Some(shot) = s.viewing.clone() else {
                        return;
                    };
                    if let Err(error) = shot.open_externally() {
                        lock(&s.core).toast(error, "error");
                    }
                })
                .build(k),
        )
        .child(
            Button::new("Close")
                .variant(ButtonVariant::Ghost)
                .control_size(ControlSize::Small)
                .shortcut("esc")
                .show_shortcut()
                .on_press(close)
                .build(k),
        );
    let panel = col()
        .key("viewer")
        .role("dialog")
        .label("Image")
        .w_full()
        .h_full()
        .gap(12.0)
        .items_center()
        .justify_center()
        .child(picture)
        .child(bar)
        .on_press(close)
        .token_motion(k.motion.overlay)
        .modal(true, close);
    layer.push(
        el().key("viewer-scrim")
            .absolute(0.0, 0.0)
            .w_full()
            .h_full()
            .fill(c.scrim)
            .token_motion(k.motion.overlay)
            .a11y_hidden(),
    );
    layer.push(
        el().key("viewer-layer")
            .absolute(0.0, 0.0)
            .w_full()
            .h_full()
            .padding(40.0)
            .child(panel),
    );
}
