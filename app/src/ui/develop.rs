//! When the agent's turn ends, a scanner makes one pass down its answer and develops it: inside
//! the band the rendered text redraws as ordered dither on the dot grid, inked in the accent at
//! the leading edge and cooling back to crisp text behind it. The slot reads its backdrop, so
//! the dots are the answer's own glyphs.

use std::time::Instant;

use rune::{Color, El, effect};
use rune_kit::Kit;

use super::dither::{self, Motion};
use super::seal::Sweep;

pub const MARK: u32 = 0xde7e;
const SECONDS: f32 = 0.6;

/// The pass over the answer whose transcript key is `key`.
pub struct Develop {
    sweep: Sweep,
    key: String,
}

impl Default for Develop {
    fn default() -> Develop {
        Develop {
            sweep: Sweep::lasting(SECONDS),
            key: String::new(),
        }
    }
}

impl Develop {
    pub fn start(&mut self, now: Instant, key: String) {
        self.key = key;
        self.sweep.start(now);
    }

    pub fn clear(&mut self) {
        self.sweep.clear();
    }

    pub fn progress(&self) -> Option<f32> {
        self.sweep.progress()
    }

    /// How far the pass is over the answer keyed `key`, if it's that answer's turn.
    pub fn at(&self, key: &str) -> Option<f32> {
        self.progress().filter(|_| self.key == key)
    }

    pub fn tick(&mut self, now: Instant, motion: Motion, visible: bool) {
        self.sweep.tick(now, motion, visible);
    }
}

/// `0xrrggbb` in sRGB, exact in an `f32`.
fn packed(color: Color) -> f32 {
    let [r, g, b, _] = color.premul_srgb(1.0);
    let byte = |c: f32| (c.clamp(0.0, 1.0) * 255.0).round() as u32;
    ((byte(r) << 16) | (byte(g) << 8) | byte(b)) as f32
}

/// Lies over the answer, so its parent must be the answer's column.
pub fn slot(progress: f32, k: &Kit) -> El {
    let c = &k.c;
    effect(MARK)
        .key("develop")
        .pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
        .effect_backdrop(true)
        .effect_data([
            progress,
            packed(c.text_accent),
            packed(c.bg_base),
            packed(c.text_base),
        ])
        .transition(rune::Transition::none())
}

const SHADER: &str = r#"
struct P {
    rect: vec4f,
    // pass origin, scale, sRGB blending
    origin: vec4f,
    // progress, then accent, background and text as packed sRGB
    data: vec4f,
}
var<immediate> p: P;
@group(0) @binding(0) var backdrop: texture_2d<f32>;

const PITCH: f32 = 3.0;
const DOT: f32 = 2.0;
// The band behind the leading edge, and how much of a long answer the pass covers: its tail,
// where the transcript sits.
const REACH: f32 = 110.0;
const SPAN: f32 = 760.0;

@vertex fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4f {
    return vec4f(vec2f(f32((i << 1u) & 2u), f32(i & 2u)) * 2.0 - 1.0, 0.0, 1.0);
}

fn bayer8(c: vec2u) -> f32 {
    let x = c.x & 7u;
    let z = x ^ (c.y & 7u);
    let v = ((z & 1u) << 5u) | ((x & 1u) << 4u) | ((z & 2u) << 2u) | ((x & 2u) << 1u) | ((z & 4u) >> 1u) | ((x & 4u) >> 2u);
    return (f32(v) + 0.5) / 64.0;
}

fn to_linear(c: vec3f) -> vec3f {
    return select(pow((c + 0.055) / 1.055, vec3f(2.4)), c / 12.92, c <= vec3f(0.04045));
}

// A packed sRGB colour, in the space the pass blends in.
fn unpack(v: f32) -> vec3f {
    let n = u32(v);
    let c = vec3f(f32((n >> 16u) & 255u), f32((n >> 8u) & 255u), f32(n & 255u)) / 255.0;
    if (p.origin.w > 0.5) { return c; }
    return to_linear(c);
}

fn luma(c: vec3f) -> f32 { return dot(c, vec3f(0.2126, 0.7152, 0.0722)); }

// What was drawn at a point of the slot, over the page's ground. The backdrop is window-sized
// even inside an isolated layer, which it holds at the layer's origin.
fn under(pt: vec2f, ground: vec3f) -> vec3f {
    let px = vec2i(p.rect.xy + pt * p.origin.z);
    let size = vec2i(textureDimensions(backdrop)) - 1;
    let c = textureLoad(backdrop, clamp(px, vec2i(0), size), 0);
    return c.rgb + ground * (1.0 - c.a);
}

@fragment fn fs(@builtin(position) frag: vec4f) -> @location(0) vec4f {
    let scale = p.origin.z;
    let local = (frag.xy + p.origin.xy - p.rect.xy) / scale;
    let size = p.rect.zw / scale;
    if (any(local < vec2f(0.0)) || any(local >= size)) { discard; }
    let cell = vec2u(max(local, vec2f(0.0)) / PITCH);
    let corner = vec2f(cell) * PITCH;
    let inner = local - corner;
    let h = size.y;

    // Out fast, settling as it reaches the end.
    let progress = clamp(p.data.x, 0.0, 1.0);
    let eased = 1.0 - (1.0 - progress) * (1.0 - progress);
    let front = mix(max(0.0, h - SPAN), h + REACH, eased);
    let behind = (front - corner.y - PITCH * 0.5) / REACH;
    if (behind < 0.0 || behind > 1.0) { discard; }
    let w = (1.0 - behind) * smoothstep(0.0, 0.12, progress);
    let edge = behind * REACH < PITCH * 1.5;
    if (!edge && w <= bayer8(cell)) { discard; }

    let accent = unpack(p.data.y);
    let ground = unpack(p.data.z);
    let text = unpack(p.data.w);
    let contrast = max(abs(luma(text) - luma(ground)), 0.04);

    // The cell's darkest and lightest ink, as coverage of the text colour.
    var ink = ground;
    var paper = ground;
    var most = 0.0;
    var least = 2.0;
    for (var i = 0u; i < 9u; i++) {
        let at = corner + (vec2f(f32(i % 3u), f32(i / 3u)) + 0.5) * (PITCH / 3.0);
        let c = under(at, ground);
        let cover = clamp(abs(luma(c) - luma(ground)) / contrast, 0.0, 1.0);
        if (cover > most) { most = cover; ink = c; }
        if (cover < least) { least = cover; paper = c; }
    }

    var color = paper;
    if (inner.x < DOT && inner.y < DOT) {
        if (edge && bayer8(cell + vec2u(3u, 5u)) < 0.6 * w) {
            color = accent;
        } else if (most > 0.15 && most > bayer8(cell + vec2u(5u, 3u)) * 0.85) {
            color = mix(ink, accent, smoothstep(0.2, 0.9, w));
        }
    }
    return vec4f(color, 1.0);
}
"#;

pub struct Gpu {
    group: wgpu::BindGroupLayout,
    pipeline: wgpu::RenderPipeline,
}

pub fn gpu(device: &wgpu::Device, format: wgpu::TextureFormat) -> Gpu {
    let group = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("develop"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        }],
    });
    let pipeline = dither::pipeline_with(device, format, "develop", SHADER, 48, &[Some(&group)]);
    Gpu { group, pipeline }
}

/// Draws the pass, or nothing on a target that can't hand over its backdrop.
pub fn draw(
    gpu: &Gpu,
    device: &wgpu::Device,
    pass: &mut wgpu::RenderPass<'_>,
    site: &rune::render::EffectSite<'_>,
    srgb: bool,
) {
    let Some(backdrop) = site.backdrop else {
        return;
    };
    let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("develop"),
        layout: &gpu.group,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::TextureView(backdrop),
        }],
    });
    let [x, y, w, h] = site.instance.rect;
    let [ox, oy] = site.origin;
    let immediates: Vec<u8> = [x, y, w, h, ox, oy, site.scale, f32::from(u8::from(srgb))]
        .into_iter()
        .chain(site.data)
        .flat_map(f32::to_ne_bytes)
        .collect();
    pass.set_pipeline(&gpu.pipeline);
    pass.set_bind_group(0, &group, &[]);
    pass.set_immediates(0, &immediates);
    pass.draw(0..3, 0..1);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn passes_only_over_the_answer_it_started_on() {
        let now = Instant::now();
        let mut develop = Develop::default();
        develop.start(now, "item-3".into());
        develop.tick(now + Duration::from_millis(300), Motion::Live, true);
        assert!((develop.at("item-3").unwrap() - 0.5).abs() < 0.001);
        assert_eq!(develop.at("item-1"), None);
        develop.tick(now + Duration::from_secs(1), Motion::Live, true);
        assert_eq!(develop.at("item-3"), None);
    }

    #[test]
    fn packs_colours_exactly() {
        let color = rune::color::hex("#12abef").unwrap();
        assert_eq!(packed(color) as u32, 0x12abef);
    }
}
