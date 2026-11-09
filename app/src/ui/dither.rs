//! An ordered-dither shoreline behind the empty chat, drawn on the GPU at an effect slot: ripples
//! run along the band and a swash surges in and drains back, with foam on its leading edge.

use std::collections::HashMap;
use std::time::Duration;

use rune::compose::{Compositor, DeviceKey, Frame};
use rune::{El, Spring, SpringPair, Transition, effect};

use crate::ui::window::Window;
use crate::ui::{seal, stage};

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

/// Fills its parent, and fades out when the chat gets its first message.
pub fn slot(dark: bool, motion: Motion) -> El {
    effect(MARK)
        .key("dither")
        .pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
        .effect_data([f32::from(u8::from(dark)), motion as u8 as f32, 0.0, 0.0])
        .transition(
            Transition::asymmetric(Transition::none(), Transition::opacity())
                .springs(SpringPair::new(Spring::NONE, Spring::new(0.15, 0.0))),
        )
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

fn rgb(hex: u32) -> [f32; 3] {
    [(hex >> 16) & 0xff, (hex >> 8) & 0xff, hex & 0xff].map(|c| c as f32 / 255.0)
}

const SHADER: &str = r#"
struct P {
    rect: vec4f,
    // scale, time, sRGB blending, strength
    misc: vec4f,
    // pass origin x and y, speck dimming
    origin: vec4f,
    foam: vec4f,
    // colour, and where across the band it sits
    stops: array<vec4f, 5>,
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

    let slope = 0.42;
    let ripple = 16.0 * sin(x / 190.0 - t * 0.5) + 8.0 * sin(x / 71.0 + 1.3 - t * 0.9);
    // The swash breaks along the shore: in fast, out slow.
    let phase = fract((t - x / 700.0) / 7.0);
    let surge = smoothstep(0.0, 0.16, phase) * (1.0 - smoothstep(0.16, 1.0, phase));
    let line = 36.0 + slope * x + ripple + 36.0 * surge;
    let d = (y - line) / sqrt(1.0 + slope * slope);

    let above = smoothstep(-260.0, -10.0, d);
    let below = 1.0 - smoothstep(6.0, 64.0, d);
    var lit = above * above * below;
    var color = ramp(d);

    let froth = noise(pt / 7.0 + vec2f(t * 1.6, -t * 0.7));
    let foam = exp(-pow((d - 34.0) / 12.0, 2.0)) * surge * smoothstep(0.35, 0.75, froth);
    lit = max(lit, foam);
    color = mix(color, p.foam.rgb, clamp(foam * 1.6, 0.0, 1.0));

    let bottom = 1.0 - smoothstep(h - 90.0, h - 10.0, y);
    lit *= bottom * p.misc.w;

    var alpha = 1.0;
    if (lit <= bayer8(cell)) {
        // A speck glints for part of its cycle, fading in and out.
        let id = cell_hash(cell);
        let cycle = t * 0.8 + u01(id) * 9.0;
        let specks = 0.03 * (1.0 - smoothstep(220.0, 400.0, y)) * bottom;
        if (u01(id ^ pcg(u32(floor(cycle)))) >= specks) { discard; }
        let q = fract(cycle);
        alpha = smoothstep(0.0, 0.15, q) * (1.0 - smoothstep(0.6, 0.85, q));
        color = ramp(-200.0 + 230.0 * u01(id + 7u)) * p.origin.z;
    }
    if (p.misc.z < 0.5) { color = to_linear(color); }
    return vec4f(color * alpha, alpha);
}
"#;

const IMMEDIATES: usize = 4 * (4 + 5) * 4;

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
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(label),
        bind_group_layouts: &[],
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
    let [dark, ..] = site.data;
    let palette = if dark > 0.5 { &DARK } else { &LIGHT };
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
        0.0,
        fr,
        fg,
        fb,
        0.0,
    ];
    for (at, hex) in palette.stops {
        let [r, g, b] = rgb(hex);
        values.extend([r, g, b, at]);
    }
    values.iter().flat_map(|v| v.to_ne_bytes()).collect()
}

/// Draws frames only when asked: `window::poll` paces the motion at `FRAME`. Paints the shore and
/// the working chat's stage (`stage::MARK`).
#[derive(Default)]
pub struct Waves {
    device: Option<DeviceKey>,
    pipelines: HashMap<wgpu::TextureFormat, wgpu::RenderPipeline>,
    stage_pipelines: HashMap<wgpu::TextureFormat, wgpu::RenderPipeline>,
    seal_pipelines: HashMap<wgpu::TextureFormat, wgpu::RenderPipeline>,
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
        f.ui.draw_with(f.encoder, f.target, clear, &mut |pass, site| {
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
            let t = match Motion::of(site.data[1]) {
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
