//! Declarative design-token specimens. No HTML, scripts, fonts or network resources are loaded.

use std::collections::BTreeMap;

use rune::{Color, El, TextStyle, col, el, row, text};
use rune_kit::Kit;
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct System {
    #[serde(default)]
    name: String,
    #[serde(default)]
    colors: BTreeMap<String, String>,
    #[serde(default)]
    typography: BTreeMap<String, TypeSample>,
    #[serde(default)]
    spacing: BTreeMap<String, f32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TypeSample {
    size: f32,
    #[serde(default = "normal_weight")]
    weight: f32,
    #[serde(default)]
    sample: String,
}

fn normal_weight() -> f32 {
    400.0
}

fn color(value: &str) -> Option<Color> {
    let hex = value.strip_prefix('#')?;
    if !matches!(hex.len(), 3 | 6 | 8) || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    rune::color::hex(value)
}

fn name_ok(name: &str) -> bool {
    !name.trim().is_empty() && name.chars().count() <= 80
}

fn parse(source: &str) -> Result<System, String> {
    if source.len() > 32 * 1024 {
        return Err("Keep a design system under 32 KiB.".into());
    }
    let system: System = serde_json::from_str(source).map_err(|e| e.to_string())?;
    if system.colors.is_empty() && system.typography.is_empty() && system.spacing.is_empty() {
        return Err("Add colors, typography or spacing to preview.".into());
    }
    if system.name.chars().count() > 80 {
        return Err("Use at most 80 characters for the system name.".into());
    }
    if [
        system.colors.len(),
        system.typography.len(),
        system.spacing.len(),
    ]
    .into_iter()
    .any(|n| n > 32)
    {
        return Err("Use at most 32 tokens per section.".into());
    }
    if system
        .colors
        .keys()
        .chain(system.typography.keys())
        .chain(system.spacing.keys())
        .any(|name| !name_ok(name))
    {
        return Err("Token names must be 1–80 characters.".into());
    }
    for (name, value) in &system.colors {
        if color(value).is_none() {
            return Err(format!("{name}: use #RGB, #RRGGBB or #RRGGBBAA."));
        }
    }
    for (name, ty) in &system.typography {
        if !(8.0..=72.0).contains(&ty.size)
            || !(100.0..=900.0).contains(&ty.weight)
            || ty.sample.chars().count() > 160
        {
            return Err(format!(
                "{name}: use size 8–72, weight 100–900 and at most 160 sample characters."
            ));
        }
    }
    for (name, value) in &system.spacing {
        if !(0.0..=128.0).contains(value) {
            return Err(format!("{name}: spacing must be 0–128 points."));
        }
    }
    Ok(system)
}

fn heading(title: &str, k: &Kit) -> El {
    text(title.to_owned())
        .font(k.ty.small_medium)
        .color(k.c.text_muted)
}

pub fn render(source: &str, k: &Kit) -> Result<El, String> {
    let system = parse(source)?;
    let mut preview = col()
        .key("design-system-preview")
        .w_full()
        .gap(16.0)
        .padding(12.0)
        .role("group")
        .label("Design system preview");
    if !system.name.is_empty() {
        preview = preview.child(
            text(system.name)
                .font(k.ty.body_medium)
                .color(k.c.text_base)
                .wrap(),
        );
    }
    if !system.colors.is_empty() {
        let mut swatches = row().w_full().gap(12.0).flex_wrap();
        for (name, value) in &system.colors {
            swatches = swatches.child(
                col()
                    .w(132.0)
                    .gap(6.0)
                    .child(
                        el().w_full()
                            .h(48.0)
                            .radius(k.radius.sm)
                            .fill(color(value).expect("validated color"))
                            .border(1.0, k.c.border_base)
                            .role("img")
                            .label(format!("{name}: {value}")),
                    )
                    .child(
                        text(name.clone())
                            .font(k.ty.small_medium)
                            .color(k.c.text_base)
                            .wrap(),
                    )
                    .child(
                        text(value.clone())
                            .font(k.ty.mono)
                            .color(k.c.text_muted)
                            .selectable(true),
                    ),
            );
        }
        preview = preview.child(
            col()
                .w_full()
                .gap(8.0)
                .child(heading("Colors", k))
                .child(swatches),
        );
    }
    if !system.typography.is_empty() {
        let mut samples = col()
            .w_full()
            .gap(12.0)
            .child(heading("Typography · app typeface", k));
        for (name, ty) in &system.typography {
            let specimen = if ty.sample.is_empty() {
                "The quick brown fox"
            } else {
                &ty.sample
            };
            samples = samples.child(
                col()
                    .w_full()
                    .gap(4.0)
                    .child(
                        text(format!("{name} · {} pt / {}", ty.size, ty.weight))
                            .font(k.ty.small)
                            .color(k.c.text_muted)
                            .wrap(),
                    )
                    .child(
                        text(specimen.to_owned())
                            .font(TextStyle {
                                size: ty.size,
                                weight: ty.weight,
                                line_height: 0.0,
                                ..k.ty.body
                            })
                            .color(k.c.text_base)
                            .wrap()
                            .selectable(true),
                    ),
            );
        }
        preview = preview.child(samples);
    }
    if !system.spacing.is_empty() {
        let mut spaces = col().w_full().gap(8.0).child(heading("Spacing", k));
        let mut tokens: Vec<_> = system.spacing.iter().collect();
        tokens.sort_by(|a, b| a.1.total_cmp(b.1).then_with(|| a.0.cmp(b.0)));
        for (name, value) in tokens {
            spaces = spaces.child(
                col()
                    .w_full()
                    .gap(4.0)
                    .child(
                        text(format!("{name} · {value} pt"))
                            .font(k.ty.small)
                            .color(k.c.text_muted)
                            .wrap(),
                    )
                    .child(el().w(*value).h(8.0).fill(k.c.text_muted)),
            );
        }
        preview = preview.child(spaces);
    }
    Ok(preview)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_tokens_and_type_defaults() {
        let system = parse(r##"{"colors":{"ink":"#123","accent":"#12345678"},"typography":{"body":{"size":14}},"spacing":{"zero":0,"large":128}}"##).unwrap();
        assert_eq!(system.typography["body"].weight, 400.0);
        assert_eq!(system.colors.len(), 2);
    }

    #[test]
    fn rejects_invalid_or_unbounded_specimens() {
        for source in [
            "{}",
            "null",
            "{",
            r#"{"html":"<script/>"}"#,
            r##"{"colors":{"bad":"red"}}"##,
            r##"{"colors":{"bad":"#12"}}"##,
            r##"{"colors":{"bad":"#é12"}}"##,
            r#"{"typography":{"body":{"size":0}}}"#,
            r#"{"typography":{"body":{"size":14,"weight":1000}}}"#,
            r#"{"typography":{"body":{"size":14,"font":"https://example.com"}}}"#,
            r#"{"spacing":{"negative":-1}}"#,
            r#"{"spacing":{"huge":129}}"#,
            r#"{"spacing":{"infinite":1e999}}"#,
            r#"{"spacing":{" ":1}}"#,
        ] {
            assert!(parse(source).is_err(), "{source}");
        }
        assert!(parse(&" ".repeat(32 * 1024 + 1)).is_err());
        let tokens: BTreeMap<_, _> = (0..33).map(|i| (format!("token{i}"), 4)).collect();
        assert!(parse(&serde_json::json!({"spacing": tokens}).to_string()).is_err());
    }
}
