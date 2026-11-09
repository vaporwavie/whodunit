//! Pixel sprites for the scoreboard, in `assets/sprites/*.json`. Model art has stable keys;
//! `model-aliases.json` maps agent ids to those keys without changing scoreboard identity.

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
/// The same role art at one point per pixel, 16 points square: the sidebar's ended-run icon.
static ROLE_ICONS: LazyLock<HashMap<String, Art>> =
    LazyLock::new(|| load(include_str!("../assets/sprites/roles.json"), 1));
static MODELS: LazyLock<HashMap<String, Art>> =
    LazyLock::new(|| load(include_str!("../assets/sprites/models.json"), 3));
/// The same model art at one point per pixel, 12 points square: the sidebar's role tooltip.
static MODEL_ICONS: LazyLock<HashMap<String, Art>> =
    LazyLock::new(|| load(include_str!("../assets/sprites/models.json"), 1));
/// The same model art at two points per pixel, 24 points square: a scoreboard table row.
static MODEL_SMALL: LazyLock<HashMap<String, Art>> =
    LazyLock::new(|| load(include_str!("../assets/sprites/models.json"), 2));
static MODEL_ALIASES: LazyLock<HashMap<String, String>> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../assets/sprites/model-aliases.json"))
        .expect("model aliases are checked in and valid")
});
/// Decider art at two points per pixel, 24 points square: beside a class card's shares.
static DECIDERS: LazyLock<HashMap<String, Art>> =
    LazyLock::new(|| load(include_str!("../assets/sprites/deciders.json"), 2));

fn pick<'a>(set: &'a HashMap<String, Art>, key: &str) -> &'a Art {
    set.get(key).unwrap_or_else(|| &set["fallback"])
}

pub fn role(name: &str) -> &'static Art {
    pick(&ROLES, name)
}

pub fn role_icon(name: &str) -> &'static Art {
    pick(&ROLE_ICONS, name)
}

fn model_key(agent: &str) -> &'static str {
    MODEL_ALIASES.get(agent).map_or("fallback", String::as_str)
}

pub fn model(agent: &str) -> &'static Art {
    &MODELS[model_key(agent)]
}

pub fn model_icon(agent: &str) -> &'static Art {
    &MODEL_ICONS[model_key(agent)]
}

pub fn model_small(agent: &str) -> &'static Art {
    &MODEL_SMALL[model_key(agent)]
}

/// The deal source's icon (`jev`, `picker`, `forced`, `fallback`); unknown sources count as
/// `fallback`, as the scoreboard tallies them.
pub fn decider(source: &str) -> &'static Art {
    DECIDERS
        .get(source)
        .unwrap_or_else(|| &DECIDERS["fallback"])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ptr;

    #[test]
    fn model_aliases_share_the_expected_art() {
        for (key, agents) in [
            ("opus", vec!["anthropic/claude-opus-5-5"]),
            (
                "sol",
                vec!["openai/gpt-6.1-sol", "openai-codex/gpt-6.1-sol"],
            ),
            (
                "astra",
                vec!["openai/gpt-6-astra", "openai-codex/gpt-6-astra"],
            ),
            (
                "gemini",
                vec!["google/gemini-4-argon", "radius/gemini-4-argon"],
            ),
            (
                "kimi",
                vec![
                    "fireworks/accounts/fireworks/models/kimi-k3p1-pro",
                    "radius/kimi-k3.1-pro",
                ],
            ),
        ] {
            for agent in agents {
                assert!(ptr::eq(model(agent), &MODELS[key]), "{agent} -> {key}");
            }
        }
    }

    #[test]
    fn every_model_alias_names_existing_art() {
        for (agent, key) in MODEL_ALIASES.iter() {
            assert_ne!(key, "fallback", "{agent} explicitly names fallback art");
            assert!(MODELS.contains_key(key), "{agent} names missing art {key}");
        }
    }

    #[test]
    fn example_contenders_have_sprites_and_current_fixture_ids() {
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../../extension/roles.example.json")).unwrap();
        let board = crate::fixtures::board();
        for role in config["roles"].as_object().unwrap().values() {
            for agent in role["contenders"].as_array().unwrap() {
                let agent = agent.as_str().unwrap();
                assert!(
                    !ptr::eq(model(agent), &MODELS["fallback"]),
                    "{agent} has no sprite alias"
                );
                assert!(
                    board
                        .tables
                        .iter()
                        .flat_map(|t| &t.rows)
                        .any(|r| r.agent == agent),
                    "the board fixture does not exercise {agent}"
                );
            }
        }
    }

    #[test]
    fn unlisted_models_use_fallback_without_guessing() {
        for agent in [
            "",
            "unknown/model",
            "other/gpt-6.1-sol",
            "openai/gpt-6.1-sol-preview",
            "fireworks/kimi-k3.1-pro",
            "GPT-6.1 Sol",
            "sol",
        ] {
            assert!(ptr::eq(model(agent), &MODELS["fallback"]), "{agent}");
        }
    }
}
