//! An ordered-dither shoreline behind the empty chat, drawn on the GPU at an effect slot: ripples
//! run along the band and a swash surges in and drains back, with foam on its leading edge. The
//! same water rises along the bottom of a working chat as a tide.

use std::collections::HashMap;
use std::time::Duration;

use rune::compose::{Compositor, DeviceKey, Frame};
use rune::{El, Spring, SpringPair, Transition, effect};

use crate::ui::window::Window;
use crate::ui::{develop, seal, stage};

const MARK: u32 = 0xd17e;
/// Dot art reads as motion at this rate, at half the cost of the display's.
pub const FRAME: Duration = Duration::from_millis(33);
/// The moment Reduce Motion holds still on: the foam is mid-surge.
const REST: f32 = 1.2;

/// How the shore moves: the clock runs only while `Live`; `Paused` holds where it stopped.
#[derive(Clone, Copy, PartialEq)]
pub enum Motion {
    Live,
    Paused,
    Still,
}

impl Motion {
    const ALL: [Motion; 3] = [Motion::Live, Motion::Paused, Motion::Still];

    pub(crate) fn of(data: f32) -> Motion {
        let index = (data as usize).min(Motion::ALL.len() - 1);
        Motion::ALL[index]
    }
}

/// How far up from the chat's bottom edge the tide's slot reaches; the water sits lower.
pub const TIDE_H: f32 = 340.0;
/// The tide comes in on a stiff spring, quick with a small overshoot, and goes out slow, falling
/// away from rest as the shore's swash does: out of sight in `TIDE_OUT` seconds.
const RISE: f32 = 17.0;
const RISE_DAMPING: f32 = 0.68;
const TIDE_OUT: f32 = 2.2;
/// How long the tide takes to change colour.
const SHADE: f32 = 0.45;
/// Steps of a shade in the payload: six bits each.
const SHADE_STEPS: f32 = 63.0;
/// A thought fires the tide's net once this many bytes of it have come in, at most this often.
const FIRE_BYTES: usize = 12;
const FIRE_GAP: f64 = 0.08;
/// A firing's spike dies fast; the net's activity lingers a while longer.
const SPIKE_DECAY: f32 = 0.14;
const ACTIVITY_DECAY: f32 = 0.9;
/// A thought heats up over this many bytes, or this many seconds when it streams no text.
const HEAT_BYTES: f32 = 3000.0;
const HEAT_SECONDS: f32 = 45.0;
const HEAT_EASE: f32 = 0.6;
/// Distinct firing patterns; ten bits of the payload.
const SEEDS: u32 = 1024;

/// What the tide says about the run: ink while it works, haze while it thinks, red when a
/// provider error or a failure troubles it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Mood {
    #[default]
    Working,
    Thinking,
    Trouble,
}

impl Mood {
    /// How far toward haze and toward red.
    fn shade(self) -> Shade {
        match self {
            Mood::Working => Shade::default(),
            Mood::Thinking => Shade {
                think: 1.0,
                ..Shade::default()
            },
            Mood::Trouble => Shade {
                trouble: 1.0,
                ..Shade::default()
            },
        }
    }
}

/// The tide's colour between ink, haze (`think`) and red (`trouble`), each 0 to 1. `heat` is how
/// far the current thought has gone, and deepens the haze.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Shade {
    pub think: f32,
    pub trouble: f32,
    pub heat: f32,
}

/// The live thought the tide reacts to: `key` tells one thinking block from the next.
#[derive(Clone, Copy)]
pub struct Thought<'a> {
    pub key: usize,
    pub text: &'a str,
}

/// The tide's net firing: `spike` from the latest burst of reasoning, `activity` from the bursts
/// lately, both 0 to 1, and `seed`, from the burst's text, says which neurons fire.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Pulse {
    pub spike: f32,
    pub activity: f32,
    pub seed: u32,
}

fn step(v: f32, steps: f32) -> u32 {
    (v.clamp(0.0, 1.0) * steps).round() as u32
}

/// The first payload float: dark, then motion, then whether it's the tide, then its shade.
fn flags(dark: bool, motion: Motion, tide: bool, shade: Shade) -> f32 {
    let bits = u32::from(dark)
        | (motion as u32) << 1
        | u32::from(tide) << 3
        | step(shade.think, SHADE_STEPS) << 4
        | step(shade.trouble, SHADE_STEPS) << 10
        | step(shade.heat, SHADE_STEPS) << 16;
    bits as f32
}

fn unflag(data: f32) -> (bool, Motion, bool, Shade) {
    let bits = data as u32;
    let unstep = |v: u32| (v & 63) as f32 / SHADE_STEPS;
    (
        bits & 1 == 1,
        Motion::of(((bits >> 1) & 3) as f32),
        bits & 8 == 8,
        Shade {
            think: unstep(bits >> 4),
            trouble: unstep(bits >> 10),
            heat: unstep(bits >> 16),
        },
    )
}

/// The pulse in one payload float: ten bits of seed, seven of spike, six of activity.
fn pack_pulse(pulse: Pulse) -> f32 {
    ((pulse.seed % SEEDS) | (step(pulse.spike, 127.0) << 10) | (step(pulse.activity, 63.0) << 17))
        as f32
}

fn unpack_pulse(data: f32) -> Pulse {
    let bits = data as u32;
    Pulse {
        seed: bits % SEEDS,
        spike: ((bits >> 10) & 127) as f32 / 127.0,
        activity: ((bits >> 17) & 63) as f32 / 63.0,
    }
}

/// The composer's column in one payload float, in whole points: its left inset over its top.
/// Negative before the first layout.
fn pack_dock(dock: Option<[f32; 2]>) -> f32 {
    let point = |v: f32| v.round().clamp(0.0, 4095.0) as u32;
    dock.map_or(-1.0, |[inset, top]| {
        (point(inset) << 12 | point(top)) as f32
    })
}

fn unpack_dock(data: f32) -> [f32; 2] {
    if data < 0.0 {
        return [-1.0, -1.0];
    }
    let bits = data as u32;
    [(bits >> 12) as f32, (bits & 4095) as f32]
}

/// Fills its parent, and fades out when the chat gets its first message.
pub fn slot(dark: bool, motion: Motion) -> El {
    effect(MARK)
        .key("dither")
        .pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
        .effect_data([flags(dark, motion, false, Shade::default()), 0.0, -1.0, 0.0])
        .transition(
            Transition::asymmetric(Transition::none(), Transition::opacity())
                .springs(SpringPair::new(Spring::NONE, Spring::new(0.15, 0.0))),
        )
}

/// The shore's water along the bottom of a working chat. `tide` says how far it has come in, its
/// colour and how its net fires. `dock` is the composer's column as `[left inset, top]` in points
/// from the slot's top-left, kept clear, with the slot's own edges; `None` before the first layout.
pub fn tide(dark: bool, motion: Motion, tide: &Tide, dock: Option<[f32; 2]>) -> El {
    let shade = tide.shade();
    let label = if shade.trouble > 0.5 {
        "Trouble"
    } else if shade.think > 0.5 {
        "Thinking"
    } else {
        "Working"
    };
    effect(MARK)
        .key("tide")
        .pin(Some(0.0), None, Some(0.0), Some(0.0))
        .h(TIDE_H)
        .effect_data([
            flags(dark, motion, true, shade),
            tide.level(),
            pack_dock(dock),
            pack_pulse(tide.pulse()),
        ])
        .label(label)
        .transition(Transition::none())
}

/// How far the tide has come in, from 0 (out of sight below the chat) to 1, its colour and its
/// net. It moves toward whether the chat works: in while it does, back out when the turn ends. Its
/// shade follows the mood while it's in, and holds while it drains, so a failed turn goes out red.
/// While the agent thinks, each burst of the thought fires the net, and the thought's length heats
/// the haze.
#[derive(Default)]
pub struct Tide {
    level: f32,
    /// Level per second.
    speed: f32,
    shade: Shade,
    target: Shade,
    pulse: Pulse,
    /// The thinking block being read, how many of its bytes are read, and how many of those
    /// haven't fired yet, folded into `hash`.
    thought: Option<usize>,
    read: usize,
    unfired: usize,
    hash: u32,
    fired: f64,
    began: f64,
    last: Option<f64>,
}

impl Tide {
    /// Lands on `working` in `mood` with no motion: another chat, or a chat reloaded.
    pub fn snap(&mut self, working: bool, mood: Mood) {
        self.thought = None;
        self.target.heat = 0.0;
        self.land(working, mood);
    }

    /// Lands where the water and its colour are going, without a spike in it.
    fn land(&mut self, working: bool, mood: Mood) {
        self.level = f32::from(u8::from(working));
        self.speed = 0.0;
        self.target = Shade {
            heat: self.target.heat,
            ..mood.shade()
        };
        self.shade = self.target;
        self.pulse = Pulse::default();
        self.last = None;
    }

    /// Holds at `level` in `mood`, for snapshots.
    pub fn hold(&mut self, level: f32, mood: Mood) {
        self.snap(false, mood);
        self.level = level.clamp(0.0, 1.0);
    }

    /// Holds a thought `heat` of the way along, its net firing as `pulse`, for snapshots.
    pub fn hold_thought(&mut self, heat: f32, pulse: Pulse) {
        self.shade.heat = heat;
        self.target.heat = heat;
        self.pulse = pulse;
    }

    /// `now` is the UI clock in seconds; `thought` is the live message's thinking, if it thinks.
    pub fn tick(
        &mut self,
        now: f64,
        working: bool,
        mood: Mood,
        thought: Option<Thought>,
        motion: Motion,
    ) {
        let elapsed = self.last.map_or(0.0, |last| (now - last).max(0.0) as f32);
        self.last = Some(now);
        self.read(now, thought);
        match motion {
            // Held water would say a finished chat still works, so nothing moves unseen.
            // Unfocused, the thought's colour and its last firing hold where they were.
            Motion::Paused => {
                let (heat, pulse) = (self.shade.heat, self.pulse);
                self.land(working, mood);
                self.shade.heat = heat;
                self.pulse = pulse;
                return;
            }
            Motion::Still => {
                self.land(working, mood);
                self.shade.heat = self.target.heat;
                return;
            }
            Motion::Live => self.flow(working, elapsed),
        }
        if working || self.level == 0.0 {
            self.target = Shade {
                heat: self.target.heat,
                ..mood.shade()
            };
        }
        if self.level == 0.0 {
            self.shade = self.target;
        }
        let step = elapsed / SHADE;
        let toward = |from: f32, to: f32| from + (to - from).clamp(-step, step);
        let ease = 1.0 - (-elapsed / HEAT_EASE).exp();
        self.shade = Shade {
            think: toward(self.shade.think, self.target.think),
            trouble: toward(self.shade.trouble, self.target.trouble),
            heat: self.shade.heat + (self.target.heat - self.shade.heat) * ease,
        };
        if (self.shade.heat - self.target.heat).abs() < 0.01 {
            self.shade.heat = self.target.heat;
        }
        self.pulse.spike *= (-elapsed / SPIKE_DECAY).exp();
        self.pulse.activity *= (-elapsed / ACTIVITY_DECAY).exp();
        if self.pulse.activity < 0.01 {
            self.pulse.spike = 0.0;
            self.pulse.activity = 0.0;
        }
        if now - self.fired >= FIRE_GAP && self.unfired >= FIRE_BYTES {
            self.fire(now);
        }
    }

    /// Reads what the thought has added since the last tick, and how far along it is.
    fn read(&mut self, now: f64, thought: Option<Thought>) {
        let Some(Thought { key, text }) = thought else {
            self.thought = None;
            self.unfired = 0;
            self.target.heat = 0.0;
            return;
        };
        if self.thought != Some(key) || text.len() < self.read {
            // A thought already well under way when first seen doesn't fire for what came before.
            let fresh = text.len() <= 4 * FIRE_BYTES;
            self.thought = Some(key);
            self.read = if fresh { 0 } else { text.len() };
            self.unfired = 0;
            self.began = now;
        }
        let new = &text.as_bytes()[self.read.min(text.len())..];
        self.hash = new.iter().fold(self.hash, |hash, &byte| {
            (hash ^ u32::from(byte)).wrapping_mul(0x0100_0193)
        });
        self.unfired += new.len();
        self.read = text.len();
        let along = text.len() as f32 / HEAT_BYTES + (now - self.began) as f32 / HEAT_SECONDS;
        self.target.heat = 1.0 - (-along).exp();
    }

    /// A burst of reasoning fires the net: harder the more of it there was.
    fn fire(&mut self, now: f64) {
        let burst = self.unfired as f32;
        self.pulse = Pulse {
            spike: self.pulse.spike.max(0.55 + burst / 120.0).min(1.0),
            activity: (self.pulse.activity + burst / 240.0).min(1.0),
            seed: (self.hash ^ self.hash >> 16) % SEEDS,
        };
        self.unfired = 0;
        self.fired = now;
    }

    /// In on a spring toward 1, overshooting a little; out falling from rest, slow and then fast.
    fn flow(&mut self, working: bool, elapsed: f32) {
        let fall = 2.0 / (TIDE_OUT * TIDE_OUT);
        let mut left = elapsed;
        while left > 0.0 {
            let dt = left.min(0.004);
            left -= dt;
            let accel = if working {
                RISE * RISE * (1.0 - self.level) - 2.0 * RISE_DAMPING * RISE * self.speed
            } else {
                -fall
            };
            self.speed += accel * dt;
            self.level = (self.level + self.speed * dt).max(0.0);
            if self.level == 0.0 {
                self.speed = 0.0;
                if !working {
                    break;
                }
            }
        }
        if working && (self.level - 1.0).abs() < 0.001 && self.speed.abs() < 0.01 {
            self.level = 1.0;
            self.speed = 0.0;
        }
    }

    /// Eased by its spring or its fall; past 1 while it overshoots.
    pub fn level(&self) -> f32 {
        self.level
    }

    pub fn shade(&self) -> Shade {
        self.shade
    }

    pub fn pulse(&self) -> Pulse {
        self.pulse
    }

    /// Coming in, going out, changing colour or firing.
    pub fn moving(&self) -> bool {
        self.level > 0.0
            && (self.level != 1.0
                || self.speed != 0.0
                || self.shade != self.target
                || self.pulse.activity > 0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ticks every frame from `from` to `to` seconds.
    fn run(tide: &mut Tide, from: f64, to: f64, working: bool, thought: Option<Thought>) {
        let mood = if thought.is_some() {
            Mood::Thinking
        } else {
            Mood::Working
        };
        let mut now = from;
        while now <= to {
            tide.tick(now, working, mood, thought, Motion::Live);
            now += FRAME.as_secs_f64();
        }
    }

    #[test]
    fn the_tide_snaps_in_overshoots_a_little_and_drains_slowly() {
        let mut tide = Tide::default();
        let mut peak: f32 = 0.0;
        let mut now = 0.0;
        while now < 0.25 {
            tide.tick(now, true, Mood::Working, None, Motion::Live);
            peak = peak.max(tide.level());
            now += FRAME.as_secs_f64();
        }
        assert!(tide.level() > 0.9, "in within a quarter second");
        run(&mut tide, now, 1.0, true, None);
        peak = peak.max(tide.level());
        assert!(peak > 1.0 && peak < 1.1, "a small overshoot: {peak}");
        assert_eq!(tide.level(), 1.0, "and it settles");
        assert!(!tide.moving());
        run(&mut tide, 1.0, 1.0 + f64::from(TIDE_OUT) / 2.0, false, None);
        assert!(
            tide.level() > 0.5,
            "out is slower than in, and slow to start"
        );
        run(&mut tide, 2.2, 1.1 + f64::from(TIDE_OUT), false, None);
        assert_eq!(tide.level(), 0.0);
    }

    #[test]
    fn the_tide_lands_when_not_live() {
        let mut tide = Tide::default();
        tide.tick(0.0, true, Mood::Working, None, Motion::Live);
        tide.tick(0.1, true, Mood::Working, None, Motion::Live);
        tide.tick(10.0, true, Mood::Working, None, Motion::Paused);
        assert_eq!(tide.level(), 1.0, "unfocused, it lands");
        tide.tick(11.0, false, Mood::Working, None, Motion::Still);
        assert_eq!(tide.level(), 0.0);
        tide.tick(30.0, true, Mood::Working, None, Motion::Live);
        assert_eq!(
            tide.level(),
            0.0,
            "a landing doesn't count as time going by"
        );
    }

    #[test]
    fn the_tide_changes_colour_while_in_and_drains_in_the_colour_it_had() {
        let mut tide = Tide::default();
        tide.tick(0.0, true, Mood::Working, None, Motion::Live);
        tide.tick(1.0, true, Mood::Thinking, None, Motion::Live);
        assert_eq!(tide.shade().think, 1.0, "a long step lands on haze");
        tide.tick(1.1, true, Mood::Trouble, None, Motion::Live);
        let shade = tide.shade();
        assert!(shade.think < 1.0 && shade.trouble > 0.0 && shade.trouble < 1.0);
        assert!(tide.moving(), "it keeps drawing while the colour turns");
        tide.tick(2.0, true, Mood::Trouble, None, Motion::Live);
        assert_eq!(tide.shade(), Mood::Trouble.shade());
        tide.tick(2.5, false, Mood::Working, None, Motion::Live);
        assert_eq!(tide.shade(), Mood::Trouble.shade(), "it drains red");
        run(&mut tide, 2.5, 6.0, false, None);
        assert_eq!(tide.level(), 0.0);
        assert_eq!(
            tide.shade(),
            Mood::Working.shade(),
            "out of sight, it resets"
        );
    }

    #[test]
    fn a_thought_fires_the_net_as_it_streams_and_heats_as_it_runs() {
        let mut tide = Tide::default();
        run(&mut tide, 0.0, 1.0, true, None);
        let text = "Which README sections name the store? ".repeat(200);
        let thought = |len: usize| {
            Some(Thought {
                key: 1,
                text: &text[..len],
            })
        };
        run(&mut tide, 1.0, 1.05, true, thought(4));
        assert_eq!(tide.pulse().spike, 0.0, "a few bytes don't fire");
        tide.tick(1.1, true, Mood::Thinking, thought(80), Motion::Live);
        let first = tide.pulse();
        assert!(first.spike > 0.5 && first.activity > 0.0, "{first:?}");
        tide.tick(1.15, true, Mood::Thinking, thought(160), Motion::Live);
        assert_eq!(tide.pulse().seed, first.seed, "bursts are spaced");
        tide.tick(1.2, true, Mood::Thinking, thought(240), Motion::Live);
        assert_ne!(tide.pulse().seed, first.seed, "a new burst fires others");
        assert!(tide.moving(), "it keeps drawing while the net fires");

        run(&mut tide, 1.2, 2.5, true, thought(240));
        assert!(tide.pulse().spike < 0.01, "a spike dies fast without text");
        let warm = tide.shade().heat;
        run(&mut tide, 2.5, 6.0, true, thought(text.len()));
        let hot = tide.shade().heat;
        assert!(warm < hot && hot < 1.0, "{warm} then {hot}");

        run(&mut tide, 6.0, 9.0, true, None);
        assert_eq!(
            tide.shade().heat,
            0.0,
            "the haze cools when the thought ends"
        );
        let quiet = tide.pulse();
        assert!(
            quiet.spike == 0.0 && quiet.activity == 0.0,
            "and the net goes quiet"
        );
    }

    #[test]
    fn a_thought_seen_under_way_does_not_fire_for_what_came_before() {
        let mut tide = Tide::default();
        let text = "a long thought ".repeat(100);
        let thought = Some(Thought {
            key: 7,
            text: &text,
        });
        run(&mut tide, 0.0, 0.5, true, thought);
        assert_eq!(tide.pulse().spike, 0.0);
        assert!(tide.shade().heat > 0.3, "but it's as warm as it is long");
    }

    #[test]
    fn payloads_round_trip() {
        let shades = [
            Shade::default(),
            Mood::Thinking.shade(),
            Mood::Trouble.shade(),
            Shade {
                think: 20.0 / SHADE_STEPS,
                trouble: 41.0 / SHADE_STEPS,
                heat: 1.0,
            },
        ];
        for dark in [false, true] {
            for motion in Motion::ALL {
                for tide in [false, true] {
                    for shade in shades {
                        let (d, m, t, s) = unflag(flags(dark, motion, tide, shade));
                        assert!(d == dark && m == motion && t == tide);
                        assert!((s.think - shade.think).abs() < 1e-6);
                        assert!((s.trouble - shade.trouble).abs() < 1e-6);
                        assert!((s.heat - shade.heat).abs() < 1e-6);
                    }
                }
            }
        }
        let pulse = Pulse {
            spike: 1.0,
            activity: 1.0,
            seed: SEEDS - 1,
        };
        assert_eq!(unpack_pulse(pack_pulse(pulse)), pulse);
        assert_eq!(unpack_pulse(pack_pulse(Pulse::default())), Pulse::default());
        assert_eq!(unpack_dock(pack_dock(Some([4095.0, 4095.0]))), [4095.0; 2]);
        assert_eq!(unpack_dock(pack_dock(Some([180.4, 212.6]))), [180.0, 213.0]);
        assert_eq!(unpack_dock(pack_dock(None)), [-1.0, -1.0]);
    }
}

struct Palette {
    /// Across the band in points (negative is above it), and the sRGB colour there.
    stops: [(f32, u32); 5],
    foam: u32,
    speck_dim: f32,
    strength: f32,
}

const STOPS: [(f32, u32); 5] = [
    (-200.0, 0x1a3f8f),
    (-100.0, 0x2f7fdc),
    (-35.0, 0xc9dbe6),
    (0.0, 0x9fdcff),
    (28.0, 0x5cb8f2),
];
const LIGHT: Palette = Palette {
    stops: STOPS,
    foam: 0x8fa6ba,
    speck_dim: 0.8,
    strength: 0.85,
};
const DARK: Palette = Palette {
    stops: STOPS,
    foam: 0xf7efe0,
    speck_dim: 0.55,
    strength: 1.0,
};

/// The tide is ink rather than water: one near-black (near-white in dark mode), its depth carried
/// by the dither alone, darkest along the crest. Thinking turns it to haze, trouble to red.
/// Across the band, negative is under the crest, inside the water.
const TIDE_LIGHT: Palette = Palette {
    stops: [
        (-200.0, 0x3a3a3a),
        (-60.0, 0x262626),
        (-12.0, 0x111111),
        (0.0, 0x0a0a0a),
        (28.0, 0x4a4a4a),
    ],
    foam: 0x5e5e5e,
    speck_dim: 1.0,
    strength: 0.95,
};
const TIDE_DARK: Palette = Palette {
    stops: [
        (-200.0, 0xbdbab4),
        (-60.0, 0xd6d3cd),
        (-12.0, 0xf1eee8),
        (0.0, 0xfaf8f4),
        (28.0, 0xb3b0aa),
    ],
    foam: 0xc9c6c0,
    speck_dim: 0.8,
    strength: 1.0,
};
const HAZE_LIGHT: Palette = Palette {
    stops: [
        (-200.0, 0xa995f7),
        (-60.0, 0x8e74f2),
        (-12.0, 0x7552ea),
        (0.0, 0x6a45e6),
        (28.0, 0xb3a1f8),
    ],
    foam: 0x9d86f5,
    speck_dim: 1.0,
    strength: 0.95,
};
const HAZE_DARK: Palette = Palette {
    stops: [
        (-200.0, 0x7259d6),
        (-60.0, 0x9177f0),
        (-12.0, 0xb5a0ff),
        (0.0, 0xc8b8ff),
        (28.0, 0x8c76e8),
    ],
    foam: 0xa892fa,
    speck_dim: 0.8,
    strength: 1.0,
};
/// The haze a long thought heats to: deeper and more saturated on light, brighter on dark, its
/// sparks near white.
const HOT_LIGHT: Palette = Palette {
    stops: [
        (-200.0, 0x8a5cf6),
        (-60.0, 0x6a2ff2),
        (-12.0, 0x4f12e8),
        (0.0, 0x3f06d6),
        (28.0, 0x9a78fb),
    ],
    foam: 0x8f1dff,
    speck_dim: 1.0,
    strength: 1.0,
};
const HOT_DARK: Palette = Palette {
    stops: [
        (-200.0, 0x7a3df0),
        (-60.0, 0x9a5cff),
        (-12.0, 0xbb8fff),
        (0.0, 0xd2b8ff),
        (28.0, 0x9c66ff),
    ],
    foam: 0xf4eeff,
    speck_dim: 0.9,
    strength: 1.0,
};
const RED_LIGHT: Palette = Palette {
    stops: [
        (-200.0, 0xf09093),
        (-60.0, 0xeb6c70),
        (-12.0, 0xe2464b),
        (0.0, 0xd93a40),
        (28.0, 0xf2a3a5),
    ],
    foam: 0xee7f82,
    speck_dim: 1.0,
    strength: 0.95,
};
const RED_DARK: Palette = Palette {
    stops: [
        (-200.0, 0xb8373c),
        (-60.0, 0xdc474c),
        (-12.0, 0xff666b),
        (0.0, 0xff8085),
        (28.0, 0xc9484d),
    ],
    foam: 0xf0595e,
    speck_dim: 0.8,
    strength: 1.0,
};

/// The tide's palette at `shade`: ink, toward haze heated by the thought, then toward red.
fn tide_palette(dark: bool, shade: Shade) -> Palette {
    let (ink, haze, hot, red) = if dark {
        (&TIDE_DARK, &HAZE_DARK, &HOT_DARK, &RED_DARK)
    } else {
        (&TIDE_LIGHT, &HAZE_LIGHT, &HOT_LIGHT, &RED_LIGHT)
    };
    let haze = mix(haze, hot, shade.heat);
    mix(&mix(ink, &haze, shade.think), red, shade.trouble)
}

fn mix(a: &Palette, b: &Palette, t: f32) -> Palette {
    let lerp = |x: f32, y: f32| x + (y - x) * t;
    let hex = |x: u32, y: u32| {
        let (from, to) = (rgb(x), rgb(y));
        let channel = |i: usize| (lerp(from[i], to[i]) * 255.0).round() as u32;
        channel(0) << 16 | channel(1) << 8 | channel(2)
    };
    let mut stops = a.stops;
    for (stop, (at, colour)) in stops.iter_mut().zip(b.stops) {
        *stop = (lerp(stop.0, at), hex(stop.1, colour));
    }
    Palette {
        stops,
        foam: hex(a.foam, b.foam),
        speck_dim: lerp(a.speck_dim, b.speck_dim),
        strength: lerp(a.strength, b.strength),
    }
}

fn rgb(hex: u32) -> [f32; 3] {
    [(hex >> 16) & 0xff, (hex >> 8) & 0xff, hex & 0xff].map(|c| c as f32 / 255.0)
}

const SHADER: &str = r#"
struct P {
    rect: vec4f,
    // scale, time, sRGB blending, strength
    misc: vec4f,
    // pass origin x and y, speck dimming, tide
    origin: vec4f,
    // foam colour, and whether the clock runs
    foam: vec4f,
    // colour, and where across the band it sits
    stops: array<vec4f, 5>,
    // the tide's level, the composer column's left inset and top (negative before layout), and
    // how far it has turned to thinking
    tide: vec4f,
    // its net: how far the thought has gone, the latest burst's spike, the bursts lately, and the
    // burst's seed
    net: vec4f,
}
var<immediate> p: P;

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

fn noise(q: vec2f) -> f32 {
    let i = vec2i(floor(q));
    let f = fract(q);
    let u = f * f * (3.0 - 2.0 * f);
    let a = u01(cell_hash(vec2u(i + vec2i(10000))));
    let b = u01(cell_hash(vec2u(i + vec2i(10001, 10000))));
    let c = u01(cell_hash(vec2u(i + vec2i(10000, 10001))));
    let d = u01(cell_hash(vec2u(i + vec2i(10001))));
    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}

fn bayer8(c: vec2u) -> f32 {
    let x = c.x & 7u;
    let z = x ^ (c.y & 7u);
    let v = ((z & 1u) << 5u) | ((x & 1u) << 4u) | ((z & 2u) << 2u) | ((x & 2u) << 1u) | ((z & 4u) >> 1u) | ((x & 4u) >> 2u);
    return (f32(v) + 0.5) / 64.0;
}

fn ramp(d: f32) -> vec3f {
    if (d <= p.stops[0].w) { return p.stops[0].rgb; }
    for (var i = 0u; i < 4u; i++) {
        let a = p.stops[i];
        let b = p.stops[i + 1u];
        if (d <= b.w) { return mix(a.rgb, b.rgb, (d - a.w) / (b.w - a.w)); }
    }
    return p.stops[4].rgb;
}

// The tide's net: rows of neurons along the band, `SPAN` apart, the first on the crest itself
// (layer -1) and the rest deeper under it, each row offset half a span from the one above.
const SPAN: f32 = 54.0;
const LAYER: f32 = 22.0;

// Neuron `j` of layer `k`: how far along the band, how deep under the crest, and how hard it fires.
// A burst of reasoning fires a few, picked by its text; deeper layers answer to the bursts lately
// as much as to the latest. Now and then one fires on its own, so a quiet thought still hums.
fn neuron(k: i32, j: i32, t: f32) -> vec3f {
    let id = pcg(u32(j + 65536) * 4u + u32(k + 1));
    let x = (f32(j) + 0.5 * f32(k + 1) + 0.2 + 0.6 * u01(id)) * SPAN;
    let depth = select(0.0, 18.0 + LAYER * f32(k) + 8.0 * (u01(id + 3u) - 0.5), k >= 0);
    let seed = u32(p.net.w);
    let picked = u01(id ^ pcg(seed + 101u)) < 0.38;
    let burst = select(0.0, 0.5 + 0.5 * u01(id ^ pcg(seed + 7u)), picked);
    var a = burst * mix(p.net.y, p.net.z, f32(k + 1) / 3.0);
    let cycle = t * 1.4 + u01(id + 9u) * 17.0;
    let own = select(0.0, exp(-fract(cycle) * 6.0), u01(id ^ pcg(u32(floor(cycle)))) < 0.1);
    a = max(a, own * 0.55 * p.foam.w * (1.0 - p.net.z));
    return vec3f(x, depth, a);
}

fn to_segment(q: vec2f, a: vec2f, b: vec2f) -> f32 {
    let ab = b - a;
    let s = clamp(dot(q - a, ab) / dot(ab, ab), 0.0, 1.0);
    return length(q - a - ab * s);
}

// The net under the crest at `q` (along the band, depth under the line): how lit, and how much
// of it fires. Each neuron wires to the two nearest in the row below; a wire carries a firing
// down when both its ends fire.
fn net(q: vec2f, t: f32) -> vec2f {
    let k = clamp(i32(floor((q.y - 18.0) / LAYER)), -1, 1);
    let top = i32(floor(q.x / SPAN - 0.5 * f32(k + 1)));
    var lit = 0.0;
    var fire = 0.0;
    for (var dj = -1; dj <= 1; dj++) {
        let a = neuron(k, top + dj, t);
        if (k >= 0) {
            let node = smoothstep(4.8, 2.4, length(q - a.xy));
            lit = max(lit, node * (0.45 + 0.55 * a.z));
            fire = max(fire, node * a.z);
        }
        for (var c = -1; c <= 0; c++) {
            let b = neuron(k + 1, top + dj + c, t);
            let node = smoothstep(4.8, 2.4, length(q - b.xy));
            lit = max(lit, node * (0.45 + 0.55 * b.z));
            fire = max(fire, node * b.z);
            let carried = sqrt(a.z * b.z);
            let wire = smoothstep(1.7, 0.5, to_segment(q, a.xy, b.xy));
            lit = max(lit, wire * (0.12 + 0.88 * carried));
            fire = max(fire, wire * carried);
        }
    }
    return vec2f(lit, fire);
}

fn to_linear(c: vec3f) -> vec3f {
    return select(pow((c + 0.055) / 1.055, vec3f(2.4)), c / 12.92, c <= vec3f(0.04045));
}

@fragment fn fs(@builtin(position) frag: vec4f) -> @location(0) vec4f {
    let local = frag.xy + p.origin.xy - p.rect.xy;
    let cell = vec2u(max(local, vec2f(0.0)) / PITCH);
    let inner = local - vec2f(cell) * PITCH;
    if (inner.x >= DOT || inner.y >= DOT) { discard; }
    let scale = p.misc.x;
    let t = p.misc.y;
    let pt = vec2f(cell) * PITCH / scale;
    let x = pt.x;
    let y = pt.y;
    let h = p.rect.w / scale;

    let tide = p.origin.w > 0.5;
    let w = p.rect.z / scale;
    // The composer's column, kept clear: from its top to the chat's padding at the bottom.
    // Before the first layout, a guess at where it sits.
    var inset = p.tide.y;
    var dock = p.tide.z;
    if (dock < 0.0) {
        inset = max(24.0, (w - 820.0) * 0.5);
        dock = h - 122.0;
    }
    let slope = select(0.42, 0.0, tide);
    let ripple = 16.0 * sin(x / 190.0 - t * 0.5) + 8.0 * sin(x / 71.0 + 1.3 - t * 0.9);
    // The swash breaks along the shore: in fast, out slow. The tide's comes quicker.
    let phase = select(fract((t - x / 700.0) / 7.0), fract((t * 1.6 - x / 900.0) / 7.0), tide);
    let surge = smoothstep(0.0, 0.16, phase) * (1.0 - smoothstep(0.16, 1.0, phase));
    let think = p.tide.w;
    let heat = p.net.x;
    var d: f32;
    // Under the crest at rest, without its spikes: where the net hangs.
    var under = vec2f(0.0);
    var jag = 0.0;
    if (tide) {
        // The sea lies below the line, which rests clear above the composer, higher and rougher
        // while the agent thinks, and higher still the longer the thought runs. Going out, the line
        // sinks below the slot, foam and spray with it; the level comes eased, overshooting a
        // little as it comes in.
        let rest = dock - 46.0 - think * (22.0 + 22.0 * heat);
        let out = (1.0 - p.tide.x) * (h - rest + 110.0);
        let base = rest + out - (0.55 + 0.25 * think) * ripple - (14.0 + 8.0 * think) * surge;
        // While it thinks, the crest's neurons fire as needles off the line, a dip either side.
        if (think > 0.0) {
            let j = i32(floor(x / SPAN));
            for (var dj = -1; dj <= 1; dj++) {
                let n = neuron(-1, j + dj, t);
                let off = abs(x - n.x);
                let r = max(0.0, 1.0 - off / (7.0 + 9.0 * n.z));
                jag += n.z * (r * r - 0.22 * exp(-pow((off - 20.0) / 7.0, 2.0)));
            }
            jag *= think;
            under = vec2f(x, y - base);
        }
        d = base - think * (24.0 + 28.0 * heat) * jag - y;
    } else {
        let line = 36.0 + slope * x + ripple + 36.0 * surge;
        d = (y - line) / sqrt(1.0 + slope * slope);
    }

    var lit: f32;
    if (tide) {
        // A solid crest a few dots deep, the water thinning under it toward the composer, and
        // nothing above but the foam.
        let crest = exp(-pow((d + 4.0) / 5.5, 2.0));
        let body = smoothstep(-110.0 - 30.0 * heat, -8.0, d) * (1.0 - smoothstep(-3.0, 1.0, d));
        // While it thinks the water thins, so the net in it reads.
        lit = max(crest, (0.5 - think * (0.3 - 0.08 * heat)) * body * body);
    } else {
        let above = smoothstep(-260.0, -10.0, d);
        let below = 1.0 - smoothstep(6.0, 64.0, d);
        lit = above * above * below;
    }
    var color = ramp(d);

    let froth = noise(pt / 7.0 + vec2f(t * 1.6, -t * 0.7));
    // The tide's foam rides its crest; the shore's is flung up the sand.
    let crown = select(34.0, 5.0, tide);
    let spread = select(12.0, 6.0, tide);
    let foam = exp(-pow((d - crown) / spread, 2.0)) * surge * smoothstep(0.35, 0.75, froth);
    lit = max(lit, foam);
    color = mix(color, p.foam.rgb, clamp(foam * 1.6, 0.0, 1.0));
    if (tide && think > 0.0) {
        // A firing needle flares at the crest; the net lights under it, wires carrying it down.
        let flare = clamp(jag * 2.0, 0.0, 1.0) * exp(-pow((d + 3.0) / 6.0, 2.0));
        color = mix(color, p.foam.rgb, flare);
        if (under.y > 6.0) {
            let n = net(under, t) * think * smoothstep(6.0, 14.0, under.y);
            lit = max(lit, n.x);
            color = mix(color, mix(ramp(-12.0), p.foam.rgb, n.y), smoothstep(0.0, 0.1, n.x));
        }
    }

    var bottom = 1.0 - smoothstep(h - 90.0, h - 10.0, y);
    if (tide) {
        // Clear of the chat's edges, and of the composer's column with a margin around it.
        let sides = smoothstep(2.0, 44.0, x) * smoothstep(2.0, 44.0, w - x);
        let edge = smoothstep(2.0, 30.0, h - y);
        let centre = vec2f(w * 0.5, (dock + h - 18.0) * 0.5);
        let half = vec2f(w * 0.5 - inset, (h - 18.0 - dock) * 0.5);
        let q = abs(pt - centre) - half;
        let gap = length(max(q, vec2f(0.0))) + min(max(q.x, q.y), 0.0);
        bottom = sides * edge * smoothstep(4.0, 30.0, gap);
    }
    lit *= bottom * p.misc.w;

    var alpha = 1.0;
    if (lit <= bayer8(cell)) {
        // A speck glints for part of its cycle, fading in and out.
        let id = cell_hash(cell);
        let cycle = t * 0.8 + u01(id) * 9.0;
        var specks = 0.03 * (1.0 - smoothstep(220.0, 400.0, y)) * bottom;
        // The tide's spray flies just above the water, never off the slot's top edge.
        // It flies thicker while the net fires.
        if (tide) {
            let spray = 0.012 * (1.0 + think * (1.0 + 4.0 * p.net.z));
            specks = spray * exp(-pow((d - 12.0) / 12.0, 2.0)) * smoothstep(0.0, 30.0, y) * bottom;
        }
        if (u01(id ^ pcg(u32(floor(cycle)))) >= specks) { discard; }
        let q = fract(cycle);
        alpha = smoothstep(0.0, 0.15, q) * (1.0 - smoothstep(0.6, 0.85, q));
        color = ramp(-200.0 + 230.0 * u01(id + 7u)) * p.origin.z;
    }
    if (p.misc.z < 0.5) { color = to_linear(color); }
    return vec4f(color * alpha, alpha);
}
"#;

const IMMEDIATES: usize = 4 * (4 + 7) * 4;

fn pipeline(device: &wgpu::Device, format: wgpu::TextureFormat) -> wgpu::RenderPipeline {
    pipeline_for(device, format, "dither", SHADER, IMMEDIATES)
}

/// A full-slot triangle with `source`'s `vs` and `fs`, blending premultiplied.
pub fn pipeline_for(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    label: &str,
    source: &str,
    immediates: usize,
) -> wgpu::RenderPipeline {
    pipeline_with(device, format, label, source, immediates, &[])
}

/// `pipeline_for` with bind groups, for a slot that reads its backdrop.
pub fn pipeline_with(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    label: &str,
    source: &str,
    immediates: usize,
    groups: &[Option<&wgpu::BindGroupLayout>],
) -> wgpu::RenderPipeline {
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(label),
        bind_group_layouts: groups,
        immediate_size: immediates as u32,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("vs"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some("fs"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: Default::default(),
        depth_stencil: None,
        multisample: Default::default(),
        multiview_mask: None,
        cache: None,
    })
}

fn immediates(site: &rune::render::EffectSite<'_>, t: f32, srgb: bool) -> Vec<u8> {
    let [flags, level, dock, pulse] = site.data;
    let (dark, motion, tide, shade) = unflag(flags);
    let [inset, top] = unpack_dock(dock);
    let pulse = unpack_pulse(pulse);
    let blended;
    let palette = match (tide, dark) {
        (false, false) => &LIGHT,
        (false, true) => &DARK,
        (true, _) => {
            blended = tide_palette(dark, shade);
            &blended
        }
    };
    let [x, y, w, h] = site.instance.rect;
    let [ox, oy] = site.origin;
    let [fr, fg, fb] = rgb(palette.foam);
    let mut values = vec![
        x,
        y,
        w,
        h,
        site.scale,
        t,
        f32::from(u8::from(srgb)),
        palette.strength,
        ox,
        oy,
        palette.speck_dim,
        f32::from(u8::from(tide)),
        fr,
        fg,
        fb,
        f32::from(u8::from(motion == Motion::Live)),
    ];
    for (at, hex) in palette.stops {
        let [r, g, b] = rgb(hex);
        values.extend([r, g, b, at]);
    }
    values.extend([level, inset, top, shade.think]);
    values.extend([shade.heat, pulse.spike, pulse.activity, pulse.seed as f32]);
    values.iter().flat_map(|v| v.to_ne_bytes()).collect()
}

/// Draws frames only when asked: `window::poll` paces the motion at `FRAME`. Paints the shore and
/// the working chat's stage (`stage::MARK`), the seal and the end-of-turn pass (`develop::MARK`).
#[derive(Default)]
pub struct Waves {
    device: Option<DeviceKey>,
    pipelines: HashMap<wgpu::TextureFormat, wgpu::RenderPipeline>,
    stage_pipelines: HashMap<wgpu::TextureFormat, wgpu::RenderPipeline>,
    seal_pipelines: HashMap<wgpu::TextureFormat, wgpu::RenderPipeline>,
    develop: HashMap<wgpu::TextureFormat, develop::Gpu>,
    /// The shore's clock: it advances only while live, so a pause resumes where it stopped.
    shore: f32,
    /// The stage's clock, kept the same way.
    stage: f32,
    last: Option<f32>,
}

impl Compositor<Window> for Waves {
    fn features(&self) -> wgpu::Features {
        wgpu::Features::IMMEDIATES
    }

    fn render(&mut self, f: &mut Frame<'_, Window>) {
        let key = f.device_key();
        if self.device != Some(key) {
            self.pipelines.clear();
            self.stage_pipelines.clear();
            self.seal_pipelines.clear();
            self.develop.clear();
            self.device = Some(key);
        }
        let clear = f.rt.clear_color();
        f.ui.prepare(
            f.device,
            f.queue,
            &mut f.rt.text.atlas,
            &f.rt.instances,
            f.size,
        );
        let now = f.rt.t as f32;
        if self.last.is_none() {
            self.shore = now;
        }
        let last = self.last.unwrap_or(now);
        let shore = self.shore;
        let mut next = shore;
        let stage = self.stage;
        let mut stage_next = stage;
        let srgb = f.srgb_blend();
        let device = f.device;
        let pipelines = &mut self.pipelines;
        let stage_pipelines = &mut self.stage_pipelines;
        let seal_pipelines = &mut self.seal_pipelines;
        let develops = &mut self.develop;
        f.ui.draw_with(f.encoder, f.target, clear, &mut |pass, site| {
            if site.mark == develop::MARK {
                let gpu = develops
                    .entry(site.format)
                    .or_insert_with(|| develop::gpu(device, site.format));
                develop::draw(gpu, device, pass, site, srgb);
                return;
            }
            if site.mark == seal::MARK {
                let pipeline = seal_pipelines
                    .entry(site.format)
                    .or_insert_with(|| seal::pipeline(device, site.format));
                pass.set_pipeline(pipeline);
                pass.set_immediates(0, &seal::immediates(site, srgb));
                pass.draw(0..3, 0..1);
                return;
            }
            if site.mark == stage::MARK {
                let t = match stage::motion(site) {
                    Motion::Live => {
                        stage_next = stage + (now - last);
                        stage_next
                    }
                    Motion::Paused => stage,
                    Motion::Still => stage::REST,
                };
                let pipeline = stage_pipelines
                    .entry(site.format)
                    .or_insert_with(|| stage::pipeline(device, site.format));
                pass.set_pipeline(pipeline);
                pass.set_immediates(0, &stage::immediates(site, t, srgb));
                pass.draw(0..3, 0..1);
                return;
            }
            if site.mark != MARK {
                return;
            }
            let t = match unflag(site.data[0]).1 {
                Motion::Live => {
                    next = shore + (now - last);
                    next
                }
                Motion::Paused => shore,
                Motion::Still => REST,
            };
            let pipeline = pipelines
                .entry(site.format)
                .or_insert_with(|| pipeline(device, site.format));
            pass.set_pipeline(pipeline);
            pass.set_immediates(0, &immediates(site, t, srgb));
            pass.draw(0..3, 0..1);
        });
        self.shore = next;
        self.stage = stage_next;
        self.last = Some(now);
    }
}
