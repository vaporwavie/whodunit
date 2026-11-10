//! One upward dither sweep after pi confirms that the selected run is sealed.

use std::time::Instant;

use rune::{Color, El, effect};

use super::dither::{self, Motion};

pub const MARK: u32 = 0x5ea1;
const SECONDS: f32 = 0.65;

/// A one-shot clock for a sweep: it runs only while the window is focused, and never under
/// Reduce Motion or on another page.
pub struct Sweep {
    seconds: f32,
    active: Option<(Instant, f32)>,
}

impl Default for Sweep {
    fn default() -> Sweep {
        Sweep::lasting(SECONDS)
    }
}

impl Sweep {
    pub fn lasting(seconds: f32) -> Sweep {
        Sweep {
            seconds,
            active: None,
        }
    }

    pub fn start(&mut self, now: Instant) {
        self.active = Some((now, 0.0));
    }

    pub fn clear(&mut self) {
        self.active = None;
    }

    pub fn progress(&self) -> Option<f32> {
        self.active.map(|(_, elapsed)| elapsed / self.seconds)
    }

    pub fn tick(&mut self, now: Instant, motion: Motion, visible: bool) {
        let Some((last, elapsed)) = self.active.as_mut() else {
            return;
        };
        if motion == Motion::Still || !visible {
            self.clear();
            return;
        }
        if motion == Motion::Live {
            *elapsed += now.saturating_duration_since(*last).as_secs_f32();
        }
        *last = now;
        if *elapsed >= self.seconds {
            self.clear();
        }
    }
}

pub fn slot(progress: f32, ink: Color) -> El {
    effect(MARK)
        .key("seal-sweep")
        .pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
        .effect_data([progress, ink.r, ink.g, ink.b])
        .transition(rune::Transition::none())
}

const SHADER: &str = r#"
struct P {
    rect: vec4f,
    // pass origin, scale, sRGB blending
    origin: vec4f,
    // linear RGB ink, progress
    ink: vec4f,
}
var<immediate> p: P;

@vertex fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4f {
    return vec4f(vec2f(f32((i << 1u) & 2u), f32(i & 2u)) * 2.0 - 1.0, 0.0, 1.0);
}

fn bayer8(c: vec2u) -> f32 {
    let x = c.x & 7u;
    let z = x ^ (c.y & 7u);
    let v = ((z & 1u) << 5u) | ((x & 1u) << 4u) | ((z & 2u) << 2u) | ((x & 2u) << 1u) | ((z & 4u) >> 1u) | ((x & 4u) >> 2u);
    return (f32(v) + 0.5) / 64.0;
}

fn to_srgb(c: vec3f) -> vec3f {
    return select(1.055 * pow(c, vec3f(1.0 / 2.4)) - 0.055, c * 12.92, c <= vec3f(0.0031308));
}

@fragment fn fs(@builtin(position) frag: vec4f) -> @location(0) vec4f {
    let local = (frag.xy + p.origin.xy - p.rect.xy) / p.origin.z;
    let cell = vec2u(max(local, vec2f(0.0)) / 3.0);
    let inner = local - vec2f(cell) * 3.0;
    if (inner.x >= 2.0 || inner.y >= 2.0) { discard; }
    let height = p.rect.w / p.origin.z;
    let reach = 150.0;
    let front = mix(height + reach, -reach, p.ink.w);
    let ripple = 14.0 * sin(local.x / 95.0) + 7.0 * sin(local.x / 37.0);
    let distance = (local.y - front - ripple) / reach;
    let density = max(0.0, 1.0 - abs(distance));
    if (density * density * 0.85 <= bayer8(cell)) { discard; }
    let alpha = 0.8 * (1.0 - p.ink.w);
    var color = p.ink.rgb;
    if (p.origin.w > 0.5) { color = to_srgb(color); }
    return vec4f(color * alpha, alpha);
}
"#;

pub fn pipeline(device: &wgpu::Device, format: wgpu::TextureFormat) -> wgpu::RenderPipeline {
    dither::pipeline_for(device, format, "seal", SHADER, 48)
}

pub fn immediates(site: &rune::render::EffectSite<'_>, srgb: bool) -> Vec<u8> {
    let [progress, r, g, b] = site.data;
    let [x, y, w, h] = site.instance.rect;
    let [ox, oy] = site.origin;
    [
        x,
        y,
        w,
        h,
        ox,
        oy,
        site.scale,
        f32::from(u8::from(srgb)),
        r,
        g,
        b,
        progress,
    ]
    .iter()
    .flat_map(|v| v.to_ne_bytes())
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn sweeps_once_and_stops() {
        let now = Instant::now();
        let mut sweep = Sweep::default();
        assert_eq!(sweep.progress(), None);
        sweep.start(now);
        sweep.tick(now + Duration::from_millis(325), Motion::Live, true);
        assert!((sweep.progress().unwrap() - 0.5).abs() < 0.001);
        sweep.tick(now + Duration::from_secs(1), Motion::Live, true);
        assert_eq!(sweep.progress(), None);
    }

    #[test]
    fn pauses_unfocused_and_never_animates_reduced_motion_or_hidden_pages() {
        let now = Instant::now();
        let mut sweep = Sweep::default();
        sweep.start(now);
        sweep.tick(now + Duration::from_secs(10), Motion::Paused, true);
        assert_eq!(sweep.progress(), Some(0.0));
        sweep.tick(now + Duration::from_millis(10_325), Motion::Live, true);
        assert!((sweep.progress().unwrap() - 0.5).abs() < 0.001);
        sweep.tick(now + Duration::from_secs(11), Motion::Still, true);
        assert_eq!(sweep.progress(), None);
        sweep.start(now);
        sweep.tick(now, Motion::Live, false);
        assert_eq!(sweep.progress(), None);
    }
}
