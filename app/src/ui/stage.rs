//! Dot art for a chat at work, on the shore's dot grid and drawn by the same compositor
//! (`dither::Waves`). A deck of cards riffles and deals one off while the agent is dealt. The rest
//! stays small and quiet, grey static with one neon accent: a scanner sweeps the transcript's tail
//! while the agent works, and each fan-out contender gets a slim lane whose data bus streams
//! until it answers.

use std::collections::HashMap;

use rune::{El, col, effect, row, spacer, text};
use rune_kit::Kit;

use crate::core::Core;
use crate::transcript::{Block, Item};
use crate::ui::dither::{self, Motion};

pub const MARK: u32 = 0x57a6;
/// The moment Reduce Motion holds still on: the deck split, the scanner mid-sweep.
pub const REST: f32 = 0.9;
/// An age past every settle, for lanes Reduce Motion shows at rest.
pub const SETTLED: f32 = 60.0;

const DEAL_H: f32 = 72.0;
const SCAN_H: f32 = 15.0;
const BUS_H: f32 = 6.0;
/// One pass of the deck (the shader's `DEAL_CYCLE`): split, riffle, deal a card off. A quick
/// deal still plays it through, unless the answer starts first.
pub const DEAL_HOLD: std::time::Duration = std::time::Duration::from_millis(2600);

/// What the tail of a working chat shows.
#[derive(Clone, Debug, PartialEq)]
pub enum Stage {
    /// Classifying the prompt and dealing a hidden agent; the deck starts its pass at `since`.
    Dealing {
        since: Option<std::time::Instant>,
    },
    Working,
    /// Working, with the live message thinking: its own thinking block moves, so the tail
    /// stays empty rather than stack a second animation under it.
    Thinking,
    Fanout(Fanout),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Fanout {
    pub lanes: Vec<Lane>,
    /// "93s", as the extension counts it.
    pub elapsed: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Lane {
    pub label: String,
    pub state: LaneState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaneState {
    Running,
    Done,
    Failed,
}

impl Fanout {
    /// The extension's widget line, `fan-out · A running · B done · C failed · 93s`.
    pub fn parse(line: &str) -> Option<Fanout> {
        let mut parts = line.split(" · ");
        if parts.next()? != "fan-out" {
            return None;
        }
        let mut fanout = Fanout {
            lanes: Vec::new(),
            elapsed: None,
        };
        for part in parts {
            let state = match part.split_once(' ') {
                Some((_, "running")) => LaneState::Running,
                Some((_, "done")) => LaneState::Done,
                Some((_, "failed")) => LaneState::Failed,
                _ => {
                    fanout.elapsed = Some(part.to_owned());
                    continue;
                }
            };
            let label = part.split_once(' ').map_or(part, |(label, _)| label);
            fanout.lanes.push(Lane {
                label: label.to_owned(),
                state,
            });
        }
        (!fanout.lanes.is_empty()).then_some(fanout)
    }

    pub fn answered(&self) -> usize {
        self.lanes
            .iter()
            .filter(|lane| lane.state != LaneState::Running)
            .count()
    }
}

impl Stage {
    pub fn of(core: &Core) -> Option<Stage> {
        if core.t.week.done.is_some() {
            return None;
        }
        let widget = core.widgets.values().next().and_then(|lines| lines.first());
        if let Some(fanout) = widget.and_then(|line| Fanout::parse(line)) {
            return Some(Stage::Fanout(fanout));
        }
        let dealing = core.pending_prompt.is_some() && core.t.week.deal.is_none();
        let held = core
            .dealing_since
            .is_some_and(|since| since.elapsed() < DEAL_HOLD)
            && (core.streaming || core.pending_prompt.is_some())
            && !matches!(
                core.t.items.last(),
                Some(Item::Assistant { blocks, .. }) if blocks.iter().flatten().next().is_some()
            );
        if dealing || held {
            return Some(Stage::Dealing {
                since: core.dealing_since,
            });
        }
        if !core.streaming {
            return None;
        }
        let thinking = matches!(
            core.t.items.last(),
            Some(Item::Assistant { blocks, streaming: true, .. })
                if matches!(blocks.iter().rev().flatten().next(), None | Some(Block::Thinking(_)))
        );
        Some(if thinking {
            Stage::Thinking
        } else {
            Stage::Working
        })
    }

    pub fn wide(&self) -> bool {
        matches!(self, Stage::Fanout(_))
    }

    /// Whether the transcript gives it a row of its own.
    pub fn shows(&self) -> bool {
        *self != Stage::Thinking
    }
}

/// When each lane last changed state, so a finished one settles from where it was.
#[derive(Default)]
pub struct LaneClock(HashMap<String, (LaneState, std::time::Instant)>);

impl LaneClock {
    pub fn track(&mut self, stage: Option<&Stage>) {
        let Some(Stage::Fanout(fanout)) = stage else {
            self.0.clear();
            return;
        };
        let now = std::time::Instant::now();
        // A lane already finished when first seen (a chat opened mid fan-out) shows at rest.
        let long_ago = now
            .checked_sub(std::time::Duration::from_secs_f32(SETTLED))
            .unwrap_or(now);
        self.0
            .retain(|label, _| fanout.lanes.iter().any(|l| &l.label == label));
        for lane in &fanout.lanes {
            let entry = self
                .0
                .entry(lane.label.clone())
                .or_insert((lane.state, long_ago));
            if entry.0 != lane.state {
                *entry = (lane.state, now);
            }
        }
    }

    pub fn age(&self, label: &str) -> f32 {
        self.0
            .get(label)
            .map_or(SETTLED, |(_, since)| since.elapsed().as_secs_f32())
    }
}

const DEAL: u8 = 0;
const SCAN: u8 = 1;
const RUN: u8 = 2;
const DONE: u8 = 3;
const FAIL: u8 = 4;
const THINK: u8 = 5;
const SEALED: u8 = 6;
const RAIL: u8 = 7;

/// Three lines of redacted text, written as the agent thinks.
const THINK_H: f32 = 39.0;
/// Two still lines: a thought the run hasn't revealed. Its size never follows the trace's.
const SEALED_H: f32 = 24.0;
pub const RAIL_W: f32 = 3.0;

/// A thought being written: scrambled lines, a cursor, scrolling up as they fill.
pub fn thinking(motion: Motion, k: &Kit) -> El {
    slot(k, motion, THINK, 0, 0.0)
        .w_full()
        .h(THINK_H)
        .label("Thinking")
}

/// A finished thought before the reveal, redacted. `seed` varies the lines between blocks.
pub fn sealed(seed: usize, k: &Kit) -> El {
    slot(k, Motion::Still, SEALED, seed % 64, 0.0)
        .w_full()
        .h(SEALED_H)
        .label("A thought, shown after /done")
        .help("Thinking shows after /done")
}

/// The dotted rail down a revealed thought's left edge; pin it to the block's height.
pub fn rail(k: &Kit) -> El {
    slot(k, Motion::Still, RAIL, 0, 0.0).w(RAIL_W)
}

fn slot(k: &Kit, motion: Motion, kind: u8, seed: usize, age: f32) -> El {
    let flags = f32::from(u8::from(k.dark)) + 2.0 * f32::from(motion as u8);
    effect(MARK).effect_data([flags, f32::from(kind) + 8.0 * seed as f32, age, 0.0])
}

/// The tail of a working chat. `age` is how long ago a lane last changed state.
pub fn view(stage: &Stage, motion: Motion, age: impl Fn(&str) -> f32, k: &Kit) -> El {
    match stage {
        // The pass starts from the split; without a start (or held still) it follows the clock.
        Stage::Dealing { since } => slot(
            k,
            motion,
            DEAL,
            0,
            since
                .filter(|_| motion != Motion::Still)
                .map_or(-1.0, |since| since.elapsed().as_secs_f32()),
        )
        .key("stage-deal")
        .w_full()
        .h(DEAL_H),
        Stage::Working => slot(k, motion, SCAN, 0, 0.0)
            .key("stage-scan")
            .w_full()
            .h(SCAN_H)
            .label("Working"),
        Stage::Thinking => col(),
        Stage::Fanout(fanout) => {
            let lanes = fanout.lanes.iter().enumerate().map(|(index, lane)| {
                let age = if motion == Motion::Still {
                    SETTLED
                } else {
                    age(&lane.label)
                };
                lane_view(lane, index, age, motion, k)
            });
            row().key("stage-fanout").w_full().gap(12.0).children(lanes)
        }
    }
}

/// Sits where the contender's answer card will, the same width and corners.
fn lane_view(lane: &Lane, index: usize, age: f32, motion: Motion, k: &Kit) -> El {
    let c = &k.c;
    let (kind, status, ink) = match lane.state {
        LaneState::Running => (RUN, "answering", c.text_faint),
        LaneState::Done => (DONE, "answered", c.text_muted),
        LaneState::Failed => (FAIL, "failed", c.danger.fg),
    };
    col()
        .key(format!("lane-{}", lane.label))
        .grow(1.0)
        .min_w(0.0)
        .gap(10.0)
        .padding(12.0)
        .radius(k.radius.lg)
        .fill(c.layer_01)
        .child(
            row()
                .w_full()
                .gap(8.0)
                .center_y()
                .child(
                    text(lane.label.clone())
                        .font(k.ty.small_medium)
                        .color(c.text_base),
                )
                .child(spacer())
                .child(text(status).font(k.ty.small).color(ink)),
        )
        .child(slot(k, motion, kind, index, age).w_full().h(BUS_H))
}

struct Palette {
    /// The static: most dots are this.
    base: u32,
    /// The one neon colour: the scanner's head, a locked lane, a glitch.
    accent: u32,
    alarm: u32,
    /// The deck: card edges, and the lattice on the backs, top card to bottom.
    card_edge: u32,
    card_top: u32,
    card_bottom: u32,
    strength: f32,
}

const LIGHT: Palette = Palette {
    base: 0x8794a0,
    accent: 0x0b9fae,
    alarm: 0xd92a6a,
    card_edge: 0x2f5f9f,
    card_top: 0x5cb8f2,
    card_bottom: 0x2f7fdc,
    strength: 0.9,
};
const DARK: Palette = Palette {
    base: 0x77838e,
    accent: 0x45f0d8,
    alarm: 0xff4f8b,
    card_edge: 0xf7efe0,
    card_top: 0x5cb8f2,
    card_bottom: 0x2f7fdc,
    strength: 1.0,
};

pub const SHADER: &str = r#"
struct P {
    rect: vec4f,
    // scale, time, sRGB blending, kind
    misc: vec4f,
    // pass origin x and y, seed, age
    origin: vec4f,
    // strength
    extra: vec4f,
    base: vec4f,
    accent: vec4f,
    alarm: vec4f,
    card_edge: vec4f,
    card_top: vec4f,
    card_bottom: vec4f,
}
var<immediate> p: P;

const PI: f32 = 3.14159265;

const PITCH: f32 = 3.0;
const DOT: f32 = 2.0;

@vertex fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4f {
    return vec4f(vec2f(f32((i << 1u) & 2u), f32(i & 2u)) * 2.0 - 1.0, 0.0, 1.0);
}

fn pcg(v: u32) -> u32 {
    let s = v * 747796405u + 2891336453u;
    let w = ((s >> ((s >> 28u) + 4u)) ^ s) * 277803737u;
    return (w >> 22u) ^ w;
}
fn u01(n: u32) -> f32 { return f32(pcg(n) >> 8u) * (1.0 / 16777216.0); }
fn cell_hash(c: vec2u) -> u32 { return pcg(c.x + pcg(c.y)); }

fn bayer8(c: vec2u) -> f32 {
    let x = c.x & 7u;
    let z = x ^ (c.y & 7u);
    let v = ((z & 1u) << 5u) | ((x & 1u) << 4u) | ((z & 2u) << 2u) | ((x & 2u) << 1u) | ((z & 4u) >> 1u) | ((x & 4u) >> 2u);
    return (f32(v) + 0.5) / 64.0;
}

fn to_linear(c: vec3f) -> vec3f {
    return select(pow((c + 0.055) / 1.055, vec3f(2.4)), c / 12.92, c <= vec3f(0.04045));
}

// A cell's coin, flipped `rate` times a second.
fn flick(c: vec2u, t: f32, rate: f32, salt: u32) -> f32 {
    return u01(cell_hash(c) ^ pcg(u32(floor(t * rate)) + salt));
}

// Turns q by -a: into the frame of something turned by a.
fn unturn(q: vec2f, a: f32) -> vec2f {
    let c = cos(a);
    let s = sin(a);
    return vec2f(c * q.x + s * q.y, -s * q.x + c * q.y);
}

fn sd_box(q: vec2f, half: vec2f, r: f32) -> f32 {
    let d = abs(q) - half + r;
    return length(max(d, vec2f(0.0))) + min(max(d.x, d.y), 0.0) - r;
}

// The deck: it splits, riffles back together card by card, and deals the top card off.
const CARD: vec2f = vec2f(15.0, 21.0);
const CARDS: i32 = 6;
const DEAL_CYCLE: f32 = 2.6;

// A card back: an edge, a gap, then a diamond lattice.
fn card_face(q: vec2f, d: f32, depth: f32) -> vec4f {
    if (d > -2.0) { return vec4f(p.card_edge.rgb, 1.0); }
    if (d > -4.5) { return vec4f(0.0); }
    let line = min(abs(fract((q.x + q.y) / 9.0) - 0.5), abs(fract((q.x - q.y) / 9.0) - 0.5));
    let shade = select(0.3, 0.95, line < 0.12) * mix(1.0, 0.6, depth);
    return vec4f(mix(p.card_top.rgb, p.card_bottom.rgb, depth), shade);
}

fn deal(pt: vec2f, size: vec2f, t: f32) -> vec4f {
    let ph = fract(t / DEAL_CYCLE);
    let centre = size * 0.5;
    // The dealt card leaves the top of the deck for the right, thinning out as it goes.
    let gone = smoothstep(0.72, 0.98, ph);
    if (ph > 0.72) {
        let c = centre + vec2f(gone * 140.0, -f32(CARDS) * 1.5 + 2.0 - sin(PI * gone) * 6.0);
        let q = unturn(pt - c, gone * 0.35);
        let d = sd_box(q, CARD, 3.0);
        if (d < 0.0) {
            let face = card_face(q, d, 0.0);
            return vec4f(face.rgb, face.a * (1.0 - gone));
        }
    }
    let split = smoothstep(0.02, 0.26, ph);
    for (var j = 0; j < CARDS; j++) {
        let i = CARDS - 1 - j;
        let fi = f32(i);
        let side = select(-1.0, 1.0, (i & 1) == 0);
        let back = smoothstep(0.38 + fi * 0.045, 0.5 + fi * 0.045, ph);
        let out = split * (1.0 - back);
        let lift = sin(PI * back) * 5.0 * split;
        let c = centre + vec2f(side * 27.0 * out, -fi * 1.5 + 2.0 - lift);
        let q = unturn(pt - c, side * 0.22 * out);
        let d = sd_box(q, CARD, 3.0);
        if (d < 0.0) { return card_face(q, d, 1.0 - fi / f32(CARDS - 1)); }
    }
    return vec4f(0.0);
}

// A scanner sweeps the strip; its trail breaks into dots behind it, over faint static and the
// odd glitch.
fn scan(pt: vec2f, size: vec2f, cell: vec2u, rows: u32, t: f32) -> vec4f {
    let span = size.x + 200.0;
    let head = fract(t / 3.0) * span - 100.0;
    let behind = head - pt.x;
    var e = exp(behind / 2.5);
    if (behind >= 0.0) { e = exp(-behind / 120.0); }
    var color = mix(p.base.rgb, p.accent.rgb, smoothstep(0.45, 0.95, e));
    var lit = 0.12 + e * 0.88;
    if (lit <= bayer8(cell) && flick(cell, t, 9.0, 1u) < 0.035) {
        color = p.base.rgb;
        lit = 1.0;
    }
    let beat = pcg(u32(floor(t * 4.0)) * 31u + 7u);
    if (u01(beat) < 0.22) {
        let start = u01(beat + 1u) * size.x;
        let len = 4.0 + 36.0 * u01(beat + 2u);
        if (cell.y == pcg(beat) % rows && pt.x > start && pt.x < start + len) {
            color = p.accent.rgb;
            lit = 1.0;
        }
    }
    return vec4f(color, lit);
}

// A contender's data bus: packets stream while it answers, the bar locks in the accent from the
// left when it's done, and breaks into static when it fails.
fn bus(pt: vec2f, size: vec2f, cell: vec2u, t: f32, kind: u32) -> vec4f {
    let seed = p.origin.z;
    let age = p.origin.w;
    let r = cell.y;
    let speed = 26.0 + 12.0 * f32(r % 3u) + 4.0 * seed;
    let pos = pt.x - t * speed - seed * 53.0 - f32(r) * 17.0;
    let seg = floor(pos / 10.0);
    let id = pcg(u32(i32(seg) + 100000) * 7u + r * 131u + u32(seed) * 977u);
    let within = fract(pos / 10.0);
    var color = p.base.rgb;
    var lit = select(0.0, mix(0.45, 1.0, within), u01(id) < 0.34);
    if (u01(id + 1u) < 0.15 && within > 0.7) { color = p.accent.rgb; }
    if (kind == 3u) {
        let front = smoothstep(0.0, 0.6, age) * (size.x + 12.0);
        if (pt.x < front - 6.0) {
            color = p.accent.rgb;
            lit = mix(1.0, 0.38, smoothstep(0.6, 1.4, age));
        } else if (pt.x < front) {
            color = p.accent.rgb;
            lit = 1.0;
        }
    } else if (kind == 4u) {
        let settle = smoothstep(0.0, 1.2, age);
        let noisy = flick(cell, t, mix(18.0, 5.0, settle), 9u) < mix(0.6, 0.18, settle);
        color = p.alarm.rgb;
        lit = select(0.0, 1.0, noisy);
    }
    return vec4f(color, lit);
}

// Redacted text: words of scrambled glyph cells, laid on lines LINE tall every PITCH_LINE.
const LINE: f32 = 9.0;
const PITCH_LINE: f32 = 15.0;

fn glyphs(x: f32, line: i32, cell: vec2u, scramble: f32) -> bool {
    let at = x + f32(line) * 97.0;
    let word = floor(at / 22.0);
    let id = pcg(u32(i32(word) + 100000) * 13u + u32(line + 100000) * 7u);
    if (at - word * 22.0 > 8.0 + 11.0 * u01(id)) { return false; }
    return u01(cell_hash(cell) ^ pcg(u32(scramble) + 11u)) < 0.84;
}

// A line's length, as a share of the room: paragraphs end on a short one.
fn line_len(line: i32, salt: u32) -> f32 {
    let id = pcg(u32(line + 100000) * 31u + salt);
    if (pcg(id) % 4u == 0u) { return 0.25 + 0.3 * u01(id); }
    return 0.7 + 0.3 * u01(id);
}

fn think(pt: vec2f, size: vec2f, cell: vec2u, t: f32) -> vec4f {
    let room = min(size.x, 440.0);
    if (pt.x > room) { return vec4f(0.0); }
    // Start with two redacted lines; a new one writes in as the oldest scrolls away.
    let progress = t / 1.7;
    let n = floor(progress) + 2.0;
    let f = fract(progress);
    let ease = smoothstep(0.0, 0.14, f);
    let top = size.y - PITCH_LINE * (n + 1.0) + PITCH_LINE * (1.0 - ease) + (PITCH_LINE - LINE);
    let q = pt.y - top;
    let m = floor(q / PITCH_LINE);
    if (q - m * PITCH_LINE >= LINE || m > n) { return vec4f(0.0); }
    let line = i32(m);
    let len = room * line_len(line, 0u);
    var head = len;
    if (m == n) { head = len * smoothstep(0.16, 0.9, f); }
    if (pt.x > head) { return vec4f(0.0); }
    // The newest writing still glows; lines fade as they scroll off the top.
    let behind = head - pt.x;
    let fade = mix(0.4, 1.0, smoothstep(0.0, PITCH_LINE, pt.y));
    if (m == n && behind < 3.5 && f < 0.92) { return vec4f(p.accent.rgb, fade); }
    if (!glyphs(pt.x, line, cell, floor(t * 3.0))) { return vec4f(0.0); }
    var glow = 0.0;
    if (m == n) { glow = 1.0 - smoothstep(0.0, 48.0, behind); }
    return vec4f(mix(p.base.rgb, p.accent.rgb, glow), (0.95 + 0.05 * glow) * fade);
}

fn sealed(pt: vec2f, size: vec2f, cell: vec2u) -> vec4f {
    let m = floor(pt.y / PITCH_LINE);
    if (pt.y - m * PITCH_LINE >= LINE) { return vec4f(0.0); }
    let line = i32(m);
    let salt = u32(p.origin.z) * 7919u;
    let len = min(size.x, 260.0) * line_len(line + i32(salt % 997u), salt) * select(1.0, 0.6, line == 1);
    if (pt.x > len || !glyphs(pt.x, line + i32(salt % 997u), cell, 0.0)) { return vec4f(0.0); }
    return vec4f(p.base.rgb, 0.7);
}

fn rail(pt: vec2f, cell: vec2u) -> vec4f {
    // Dashes down the edge: two cells on, one off.
    return vec4f(p.base.rgb, select(0.0, 0.9, cell.y % 3u != 2u));
}

@fragment fn fs(@builtin(position) frag: vec4f) -> @location(0) vec4f {
    let local = frag.xy + p.origin.xy - p.rect.xy;
    if (local.x < 0.0 || local.y < 0.0) { discard; }
    let cell = vec2u(local / PITCH);
    let inner = local - vec2f(cell) * PITCH;
    if (inner.x >= DOT || inner.y >= DOT) { discard; }
    let scale = p.misc.x;
    let t = p.misc.y;
    let kind = u32(p.misc.w + 0.5);
    let pt = (vec2f(cell) + 0.5) * PITCH / scale;
    let size = p.rect.zw / scale;
    // The pass's scissor is the slot's clip, not its rect.
    if (pt.x > size.x || pt.y > size.y) { discard; }
    let rows = max(u32(p.rect.w / PITCH), 1u);
    var ink = vec4f(0.0);
    switch kind {
        case 0u: { ink = deal(pt, size, select(t, p.origin.w, p.origin.w >= 0.0)); }
        case 1u: { ink = scan(pt, size, cell, rows, t); }
        case 5u: { ink = think(pt, size, cell, t); }
        case 6u: { ink = sealed(pt, size, cell); }
        case 7u: { ink = rail(pt, cell); }
        default: { ink = bus(pt, size, cell, t, kind); }
    }
    // Scanlines: every other row runs dimmer, except on the cards.
    var lit = ink.a * p.extra.x;
    if (kind != 0u && kind != 7u && (cell.y & 1u) == 1u) {
        lit *= select(0.6, 0.85, kind == 5u || kind == 6u);
    }
    if (lit <= bayer8(cell)) { discard; }
    var color = ink.rgb;
    if (p.misc.z < 0.5) { color = to_linear(color); }
    return vec4f(color, 1.0);
}
"#;

pub const IMMEDIATES: usize = 4 * 4 * 10;

fn rgb(hex: u32) -> [f32; 3] {
    [(hex >> 16) & 0xff, (hex >> 8) & 0xff, hex & 0xff].map(|c| c as f32 / 255.0)
}

/// How a stage slot moves, from its payload.
pub fn motion(site: &rune::render::EffectSite<'_>) -> Motion {
    Motion::of((site.data[0] / 2.0).floor())
}

pub fn immediates(site: &rune::render::EffectSite<'_>, t: f32, srgb: bool) -> Vec<u8> {
    let [flags, kind_seed, age, _] = site.data;
    let palette = if flags % 2.0 > 0.5 { &DARK } else { &LIGHT };
    let kind = kind_seed % 8.0;
    let seed = (kind_seed / 8.0).floor();
    let [x, y, w, h] = site.instance.rect;
    let [ox, oy] = site.origin;
    let mut values = vec![
        x,
        y,
        w,
        h,
        site.scale,
        t,
        f32::from(u8::from(srgb)),
        kind,
        ox,
        oy,
        seed,
        age,
        palette.strength,
        0.0,
        0.0,
        0.0,
    ];
    for hex in [
        palette.base,
        palette.accent,
        palette.alarm,
        palette.card_edge,
        palette.card_top,
        palette.card_bottom,
    ] {
        let [r, g, b] = rgb(hex);
        values.extend([r, g, b, 0.0]);
    }
    values.iter().flat_map(|v| v.to_ne_bytes()).collect()
}

/// A second for the shore's pipeline builder, with this shader.
pub fn pipeline(device: &wgpu::Device, format: wgpu::TextureFormat) -> wgpu::RenderPipeline {
    dither::pipeline_for(device, format, "stage", SHADER, IMMEDIATES)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_fanout_widget() {
        let fanout = Fanout::parse("fan-out · A running · B done · C failed · 93s").unwrap();
        let states: Vec<_> = fanout
            .lanes
            .iter()
            .map(|l| (l.label.as_str(), l.state))
            .collect();
        assert_eq!(
            states,
            [
                ("A", LaneState::Running),
                ("B", LaneState::Done),
                ("C", LaneState::Failed)
            ]
        );
        assert_eq!(fanout.elapsed.as_deref(), Some("93s"));
        assert_eq!(fanout.answered(), 2);
        assert_eq!(Fanout::parse("Compacting…"), None);
        assert_eq!(Fanout::parse("fan-out · 3s"), None);
    }
}
