//! Pixel sprites for the scoreboard: one per role, model and role decider, keyed by role name,
//! agent id and deal source, the first two with a `fallback`. The art lives in `assets/sprites/*.json`.

use std::collections::HashMap;
use std::sync::LazyLock;

use rune::Pixels;
use serde::Deserialize;

#[derive(Deserialize)]
struct Sprite {
    palette: HashMap<char, Option<String>>,
    rows: Vec<String>,
}

pub struct Art {
    pub pixels: Pixels,
    pub width: usize,
    pub height: usize,
    /// Points per art pixel; the image is upscaled to twice that, so it stays exact at 2×.
    pub pt: f32,
}

fn rgba(hex: &str) -> [u8; 4] {
    let channel = |at: usize| u8::from_str_radix(&hex[at..at + 2], 16).unwrap_or(0);
    [channel(1), channel(3), channel(5), 255]
}

fn rasterize(sprite: &Sprite, pt: usize) -> Art {
    let upscale = pt * 2;
    let height = sprite.rows.len();
    let width = sprite.rows.first().map_or(0, |row| row.chars().count());
    let (w, h) = (width * upscale, height * upscale);
    let mut out = vec![0u8; w * h * 4];
    for (y, row) in sprite.rows.iter().enumerate() {
        for (x, key) in row.chars().enumerate() {
            let Some(Some(hex)) = sprite.palette.get(&key) else {
                continue;
            };
            let color = rgba(hex);
            for dy in 0..upscale {
                let line = (y * upscale + dy) * w;
                for dx in 0..upscale {
                    let at = (line + x * upscale + dx) * 4;
                    out[at..at + 4].copy_from_slice(&color);
                }
            }
        }
    }
    let pixels = Pixels::new();
    pixels.set(w as u32, h as u32, out);
    Art {
        pixels,
        width,
        height,
        pt: pt as f32,
    }
}

fn load(json: &str, pt: usize) -> HashMap<String, Art> {
    let sprites: HashMap<String, Sprite> =
        serde_json::from_str(json).expect("sprite JSON is checked in and valid");
    sprites
        .iter()
        .map(|(key, sprite)| (key.clone(), rasterize(sprite, pt)))
        .collect()
}

static ROLES: LazyLock<HashMap<String, Art>> =
    LazyLock::new(|| load(include_str!("../assets/sprites/roles.json"), 4));
static MODELS: LazyLock<HashMap<String, Art>> =
    LazyLock::new(|| load(include_str!("../assets/sprites/models.json"), 3));
static DECIDERS: LazyLock<HashMap<String, Art>> =
    LazyLock::new(|| load(include_str!("../assets/sprites/deciders.json"), 3));

fn pick<'a>(set: &'a HashMap<String, Art>, key: &str) -> &'a Art {
    set.get(key).unwrap_or_else(|| &set["fallback"])
}

pub fn role(name: &str) -> &'static Art {
    pick(&ROLES, name)
}

pub fn model(agent: &str) -> &'static Art {
    pick(&MODELS, agent)
}

/// The deal source's icon (`jev`, `picker`, `forced`, `fallback`); unknown sources count as
/// `fallback`, as the scoreboard tallies them.
pub fn decider(source: &str) -> &'static Art {
    DECIDERS
        .get(source)
        .unwrap_or_else(|| &DECIDERS["fallback"])
}
