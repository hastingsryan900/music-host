//! PlayStation 3 XMB wave background, rendered on the GPU.
//!
//! A Rust/OpenGL port of linkev/PlayStation-3-XMB (`ps3xmbwave/`, MIT License,
//! © 2025 Mart): the reverse-engineered spline displacement pipeline
//! (`spline-reverse.js`), the background/wave shaders (`spline.js`), and the
//! sparkle particles (`particles.js`), with their default settings. The only
//! intentional change: the background gradient colour comes from the current
//! album cover instead of the PS3's monthly presets.

use eframe::glow::{self, HasContext};
use std::sync::Arc;

// ---------------------------------------------------------------- settings
// Defaults from spline-settings.js / particles-settings.js.

mod s {
    pub const FLOW_SPEED: f32 = 0.18;
    pub const TENSION: f32 = 0.12;
    pub const DAMPING: f32 = 0.0001;
    pub const LENGTH: f32 = 0.306001;
    pub const SPACING: f32 = 407.658;
    pub const TIME_STEP: f32 = 1.0;
    pub const BAND_AMPLITUDE: f32 = 0.200;
    pub const BAND_SECONDARY_FREQ: f32 = 7.0;
    pub const BAND_SECONDARY_AMP: f32 = 0.025;
    pub const TRAVEL_SPEED1: f32 = 0.25;
    pub const TRAVEL_AMP1: f32 = 0.014;
    pub const TRAVEL_SPEED2: f32 = 0.15;
    pub const TRAVEL_AMP2: f32 = 0.008;
    pub const PERTURBATION: f32 = 0.0998587;
    pub const PERTURBATION_SCALE: f32 = 0.07;
    pub const WAVE_COS_AMP: f32 = 0.09;
    pub const WAVE_BIAS: f32 = -0.1;
    pub const WAVE_HEIGHT_SCALE: f32 = 0.5;
    pub const WAVE_SOFT_CLIP: f32 = 0.22;
    pub const RE_PIPELINE_BLEND: f32 = 0.45;
    pub const RE_DESCRIPTOR_STRENGTH: f32 = 0.7;
    pub const RE_SEED: f32 = 1337.0;
    pub const RE_DESCRIPTOR_MOTION: f32 = 0.65;
    pub const RE_KERNEL_GAIN: f32 = 0.04;
    pub const RE_NORMALIZE_GAIN: f32 = 0.08;
    pub const RE_KERNEL_PHASE_STEP: f32 = 0.45;
    pub const RE_INDEX_JITTER: f32 = 0.006;
    pub const RE_TEMPORAL_SMOOTH: f32 = 0.84;
    pub const FRESNEL_POWER: f32 = 4.0;
    pub const FRESNEL_SCALE: f32 = 0.5;
    pub const OPACITY: f32 = 0.7;
    pub const BRIGHTNESS: f32 = 0.98;
    pub const Z_DETAIL_SCALE: f32 = 0.08;
    pub const FFD_SCALE1: [f32; 3] = [5.67726, 1.00077, 1.0];
    pub const FFD_SCALE2: [f32; 3] = [2.82755, 1.27579, 2.88782];
    pub const FFD_OFFSET: [f32; 3] = [0.0, -0.469999, 0.0];
    pub const FFD_Y_AMP: f32 = 0.05;
    pub const FFD_Z_AMP: f32 = 0.06;

    pub const PT_COUNT: usize = 2000;
    pub const PT_OPACITY: f32 = 0.75;
    pub const PT_SIZE_BASE: f32 = 2.6;
    pub const PT_SIZE_VAR: f32 = 1.5;
    pub const PT_FLOW_SPEED: f32 = 0.18;
}

// ---------------------------------------------------------------- spline pipeline (spline-reverse.js)

const B380_SIZE: usize = 0x2200;
const TABLE_ENTRIES: usize = 0x169;
const LOOP_ITERS: usize = 8;
const NORM_A: [f32; 4] = [0.39584, -0.0052389996, -0.58664495, 0.189007];
const NORM_B: [f32; 4] = [-0.003751, -0.57536095, 0.161975, 0.417137];
const R37_WORDS: [u32; 4] = [0x00, 0x11, 0x22, 0x33];
const CP_COUNT: usize = 28;
pub const TEX_W: usize = 256;
pub const TEX_H: usize = 64;

fn fract(v: f32) -> f32 {
    v - v.floor()
}

fn hash01(x: f32) -> f32 {
    fract(x.sin() * 43758.5453123)
}

fn bspline_basis(t: f32) -> [f32; 4] {
    let t2 = t * t;
    let t3 = t2 * t;
    [
        (1.0 - 3.0 * t + 3.0 * t2 - t3) / 6.0,
        (4.0 - 6.0 * t2 + 3.0 * t3) / 6.0,
        (1.0 + 3.0 * t + 3.0 * t2 - 3.0 * t3) / 6.0,
        t3 / 6.0,
    ]
}

fn eval_spline(cp: &[f32; CP_COUNT], u: f32) -> f32 {
    let n = (CP_COUNT - 3) as f32;
    let s = (u * n).clamp(0.0, n - 1e-6);
    let seg = s.floor() as usize;
    let b = bspline_basis(s - seg as f32);
    b[0] * cp[seg] + b[1] * cp[seg + 1] + b[2] * cp[seg + 2] + b[3] * cp[seg + 3]
}

fn table_index_from_word(word: u32) -> usize {
    let hi = (word >> 4) & 0xff;
    let lo = word & 0xf;
    ((19 * hi + lo) as usize) % TABLE_ENTRIES
}

fn sample_table(table: &[f32], idx: f32) -> [f32; 4] {
    let i0 = (idx.floor() as usize) % TABLE_ENTRIES;
    let i1 = (i0 + 1) % TABLE_ENTRIES;
    let t = idx - idx.floor();
    let (b0, b1) = (i0 * 4, i1 * 4);
    std::array::from_fn(|k| table[b0 + k] * (1.0 - t) + table[b1 + k] * t)
}

fn mix4(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    std::array::from_fn(|k| a[k] * (1.0 - t) + b[k] * t)
}

struct SplinePipeline {
    b380: Vec<u8>,
    b300: [f32; 4],
    coeffs: Vec<f32>,
    table: Vec<f32>,
    kernel_packed: Vec<f32>,
    kernel_prev: Vec<f32>,
    cp: [f32; CP_COUNT],
}

impl SplinePipeline {
    fn new() -> Self {
        // SyntheticDescriptorSource.buildB380 (depends only on settings).
        let b380 = (0..B380_SIZE)
            .map(|i| {
                let f = i as f32 / B380_SIZE as f32;
                let wave = ((f * 97.0) * (0.3 + s::LENGTH)).sin() * 0.5
                    + ((f * 211.0) * (0.15 + s::TENSION * 3.0)).sin() * 0.3
                    + ((f * 17.0) * (0.2 + s::PERTURBATION_SCALE * 2.0)).sin() * 0.2;
                let noise = hash01(i as f32 * 13.37 + s::RE_SEED * 0.01) * 2.0 - 1.0;
                let v = ((wave * s::RE_DESCRIPTOR_STRENGTH + noise * 0.35 + 1.0) * 0.5).clamp(0.0, 1.0);
                (v * 255.0) as u8
            })
            .collect();
        SplinePipeline {
            b380,
            // buildRuntimeInputs: only b300[0..4] feed the table.
            b300: [s::DAMPING, s::LENGTH, s::TENSION, s::SPACING * 0.001],
            coeffs: vec![0.0; TABLE_ENTRIES * 16],
            table: vec![0.0; TABLE_ENTRIES * 4],
            kernel_packed: vec![0.0; LOOP_ITERS * 8 * 4],
            kernel_prev: vec![0.0; LOOP_ITERS * 8 * 4],
            cp: [0.0; CP_COUNT],
        }
    }

    fn decode_coefficients(&mut self, t: f32) {
        let len = self.b380.len();
        let stride = 0x130;
        let phase = t * s::FLOW_SPEED * s::TIME_STEP * s::RE_DESCRIPTOR_MOTION;
        for entry in 0..TABLE_ENTRIES {
            let entry_base = entry * 16;
            let desc_base = (entry * stride) % len;
            for blk in 0..4 {
                for lane in 0..4 {
                    let byte_idx = (desc_base + blk * 0x10 + lane * 4 + (entry % 7)) % len;
                    let centered = self.b380[byte_idx] as f32 / 255.0 * 2.0 - 1.0;
                    let harmonic = (entry as f32 * 0.07 + blk as f32 * 0.91 + lane as f32 * 1.37 + phase * 0.23).sin();
                    self.coeffs[entry_base + blk * 4 + lane] = centered * 0.75 + harmonic * 0.25;
                }
            }
        }
    }

    fn build_table(&mut self, t: f32) {
        self.decode_coefficients(t);
        for i in 0..TABLE_ENTRIES {
            let c = i * 16;
            for lane in 0..4 {
                let raw = self.coeffs[c + lane] * self.b300[0]
                    + self.coeffs[c + 4 + lane] * self.b300[1]
                    + self.coeffs[c + 8 + lane] * self.b300[2]
                    + self.coeffs[c + 12 + lane] * self.b300[3];
                let denom_abs = NORM_B[lane].abs().max(0.05);
                let denom = if NORM_B[lane] < 0.0 { -denom_abs } else { denom_abs };
                let norm = ((raw - NORM_A[lane]) / denom) * s::RE_NORMALIZE_GAIN;
                self.table[i * 4 + lane] = norm.tanh();
            }
        }
    }

    fn run_kernel(&mut self, t: f32) {
        for iter in 0..LOOP_ITERS {
            let phase = t * s::RE_KERNEL_PHASE_STEP + iter as f32 * 0.37;
            let idx: [f32; 4] = std::array::from_fn(|lane| {
                let base_word = (R37_WORDS[lane] + iter as u32 * 0x13) & 0xff;
                let base = table_index_from_word(base_word) as f32;
                let off = (phase + lane as f32 * 0.77).sin() * s::RE_INDEX_JITTER * TABLE_ENTRIES as f32;
                (base + off).rem_euclid(TABLE_ENTRIES as f32)
            });
            let v: [[f32; 4]; 4] = std::array::from_fn(|k| sample_table(&self.table, idx[k]));
            let mix_a = 0.5 + 0.5 * (phase * 0.7).sin();
            let mix_b = 0.5 + 0.5 * (phase * 0.9).cos();
            let diff = |a: [f32; 4], b: [f32; 4]| -> [f32; 4] { std::array::from_fn(|k| a[k] - b[k]) };
            let stores = [
                mix4(v[0], v[1], mix_a),
                mix4(v[1], v[2], mix_b),
                mix4(v[2], v[3], mix_a),
                mix4(v[3], v[0], mix_b),
                diff(v[1], v[0]),
                diff(v[2], v[1]),
                diff(v[3], v[2]),
                diff(v[0], v[3]),
            ];
            for (si, vec) in stores.iter().enumerate() {
                let base = (iter * 8 + si) * 4;
                self.kernel_packed[base..base + 4].copy_from_slice(vec);
            }
        }
        let temporal = s::RE_TEMPORAL_SMOOTH.clamp(0.0, 0.999);
        for i in 0..self.kernel_packed.len() {
            let v = self.kernel_prev[i] * temporal + self.kernel_packed[i] * (1.0 - temporal);
            self.kernel_packed[i] = v;
            self.kernel_prev[i] = v;
        }
    }

    /// writeDisplacementTexture: fills a TEX_W × TEX_H height field.
    fn write(&mut self, t: f32, out: &mut [f32]) {
        self.build_table(t);
        self.run_kernel(t);
        let kvc = LOOP_ITERS * 8;
        let flow = t * s::FLOW_SPEED * s::TIME_STEP;
        for row in 0..TEX_H {
            let z = (row as f32 / (TEX_H - 1) as f32) * 2.0 - 1.0;
            let row_phase = flow * 0.25 + z * 1.7;
            for i in 0..CP_COUNT {
                let x = i as f32 / (CP_COUNT - 1) as f32;
                let kvf = row as f32 * 0.93 + i as f32 * 0.61 + flow * 0.35;
                let kv0 = (kvf.floor() as i64).rem_euclid(kvc as i64) as usize;
                let kv1 = (kv0 + 1) % kvc;
                let kt = kvf - kvf.floor();
                let k = |o: usize| self.kernel_packed[kv0 * 4 + o] * (1.0 - kt) + self.kernel_packed[kv1 * 4 + o] * kt;
                let (kx, ky, kz, kw) = (k(0), k(1), k(2), k(3));
                let re_core = (kx * 0.45 + ky * 0.25 + kz * 0.2 + kw * 0.1) * s::RE_KERNEL_GAIN
                    + (row_phase + x * 6.2).sin() * s::BAND_AMPLITUDE
                    + (z * s::BAND_SECONDARY_FREQ + x * 4.8 + flow * 0.09).cos() * s::BAND_SECONDARY_AMP;
                let pi = std::f32::consts::PI;
                let legacy = ((x * pi * 1.3 + z * 0.8) - flow * s::TRAVEL_SPEED1).sin() * s::TRAVEL_AMP1 * s::TENSION
                    + ((x * pi * 2.8 - z * 1.2) + flow * s::TRAVEL_SPEED2).sin() * s::TRAVEL_AMP2
                    + s::PERTURBATION
                        * s::PERTURBATION_SCALE
                        * ((x * (4.0 + s::LENGTH * 2.0) + z * 4.0 - flow * 0.6) * (s::SPACING * 0.01)).sin();
                self.cp[i] = re_core * s::RE_PIPELINE_BLEND + legacy * (1.0 - s::RE_PIPELINE_BLEND);
            }
            let base = row * TEX_W;
            for xi in 0..TEX_W {
                out[base + xi] = eval_spline(&self.cp, xi as f32 / (TEX_W - 1) as f32);
            }
        }
    }
}

// ---------------------------------------------------------------- shaders (spline.js / particles.js)

const BG_VS: &str = r#"#version 330 core
layout(location = 0) in vec2 aPos;
out vec2 vUvYDown;
void main() {
  vUvYDown = vec2(aPos.x * 0.5 + 0.5, 1.0 - (aPos.y * 0.5 + 0.5));
  gl_Position = vec4(aPos, 0.0, 1.0);
}"#;

const BG_FS: &str = r#"#version 330 core
in vec2 vUvYDown;
out vec4 oColor;
uniform vec3 uColorStart;
uniform vec3 uColorEnd;
uniform vec2 uDir;
uniform float uTMin;
uniform float uTSpan;
void main() {
  float t = dot(vUvYDown, uDir);
  float u = clamp((t - uTMin) / max(uTSpan, 1e-6), 0.0, 1.0);
  float g = u * u * (3.0 - 2.0 * u);
  oColor = vec4(mix(uColorStart, uColorEnd, g), 1.0);
}"#;

const WAVE_VS: &str = r#"#version 330 core
layout(location = 0) in vec2 aPos;
uniform sampler2D uSplineTex;
uniform float uTime;
uniform float flowSpeed;
uniform float tension;
uniform float damping;
uniform float uLength;
uniform float spacing;
uniform float perturbation;
uniform float perturbationScale;
uniform float timeStep;
uniform float waveCosAmp;
uniform float waveBias;
uniform float waveHeightScale;
uniform float waveSoftClip;
uniform float ffdYAmp;
uniform float ffdZAmp;
uniform float zDetailScale;
uniform vec3 ffdScale1;
uniform vec3 ffdScale2;
uniform vec3 ffdOffset;
out vec3 vPos;
void main() {
  vec3 p = vec3(aPos.x, 0.0, aPos.y);
  vec2 uv = (aPos + 1.0) * 0.5;
  p.y = texture(uSplineTex, uv).r;
  vec3 ffd1 = p * ffdScale1 + ffdOffset;
  vec3 ffd2 = p * ffdScale2 + ffdOffset;
  p.y += sin(ffd1.x + uTime * flowSpeed) * ffdYAmp;
  p.z += cos(ffd2.z + uTime * flowSpeed) * ffdZAmp;
  float baseWave = cos(p.x * 2.0 - uTime * 0.5 * timeStep) * waveCosAmp + waveBias;
  baseWave *= (1.0 - damping);
  baseWave += tension * sin(p.x * uLength + uTime * flowSpeed * timeStep * 0.25);
  float structured = perturbation * perturbationScale * (
    sin((p.x * uLength * 6.0 + p.z * 0.5) * spacing * 0.01 + uTime * flowSpeed * timeStep * 0.7) * 0.5 +
    sin((p.x * uLength * 10.0 - p.z * 0.8) * spacing * 0.005 - uTime * flowSpeed * timeStep * 0.35) * 0.25
  );
  float totalWave = (baseWave + structured) * waveHeightScale;
  totalWave = waveSoftClip * tanh(totalWave / max(waveSoftClip, 1e-4));
  p.y -= totalWave;
  vec2 uv2 = uv;
  uv2.x = fract(uv2.x - uTime * flowSpeed * 0.04 * timeStep);
  p.z -= texture(uSplineTex, uv2).r * zDetailScale;
  gl_Position = vec4(p, 1.0);
  vPos = p;
}"#;

const WAVE_FS: &str = r#"#version 330 core
in vec3 vPos;
out vec4 oColor;
uniform float opacity;
uniform float brightness;
uniform float fresnelPower;
uniform float fresnelScale;
void main() {
  vec3 dx = dFdx(vPos);
  vec3 dy = dFdy(vPos);
  vec3 N = normalize(cross(dx, dy));
  float F = fresnelScale * pow(1.0 + dot(vec3(0.0, 0.0, -1.0), N), fresnelPower);
  oColor = vec4(vec3(1.0), F * opacity * brightness);
}"#;

const PT_VS: &str = r#"#version 330 core
layout(location = 0) in vec3 aSeed;
uniform float uTime;
uniform float flowSpeed;
uniform float ratio;
uniform float ptSizeBase;
uniform float ptSizeVar;
uniform float uPxScale;
out float vAlpha;
void main() {
  gl_PointSize = (aSeed.z * ptSizeVar + ptSizeBase) * uPxScale;
  float time = uTime * flowSpeed;
  float x = fract(time * (aSeed.x - 0.5) / 15.0 + aSeed.y * 50.0) * 2.0 - 1.0;
  float y = sin(sign(aSeed.y) * time * (aSeed.y + 1.5) / 4.0 + aSeed.x * 100.0)
          / ((6.0 - aSeed.x * 4.0 * aSeed.y) / ratio);
  float opVar = mix(
    sin(time * (aSeed.x + 0.5) * 12.0 + aSeed.y * 10.0),
    sin(time * (aSeed.y + 1.5) * 6.0 + aSeed.x * 4.0),
    y * 0.5 + 0.5) * aSeed.x + aSeed.y;
  vAlpha = opVar * opVar * (1.0 - fract(aSeed.x + time * 0.00285));
  gl_Position = vec4(x, y, 0.0, 1.0);
}"#;

const PT_FS: &str = r#"#version 330 core
in float vAlpha;
out vec4 oColor;
uniform float particleOpacity;
void main() {
  vec2 c = gl_PointCoord * 2.0 - 1.0;
  float d = dot(c, c);
  if (d > 1.0) discard;
  float sparkle = (1.0 - d) * (1.0 - d);
  float a = vAlpha * particleOpacity * sparkle;
  oColor = vec4(vec3(a), 1.0);
}"#;

// ---------------------------------------------------------------- renderer

/// Per-frame inputs from the UI.
#[derive(Clone, Copy)]
pub struct Frame {
    /// Seconds of wave time.
    pub time: f32,
    /// Gradient colours (top → bottom), 0..1 RGB.
    pub top: [f32; 3],
    pub bottom: [f32; 3],
}

pub struct Xmb {
    bg_prog: glow::Program,
    wave_prog: glow::Program,
    pt_prog: glow::Program,
    quad_vao: glow::VertexArray,
    grid_vao: glow::VertexArray,
    grid_count: i32,
    pt_vao: glow::VertexArray,
    tex: glow::Texture,
    data: Vec<f32>,
    pipeline: SplinePipeline,
    particle_offset: f32,
}

unsafe fn compile(gl: &glow::Context, vs: &str, fs: &str) -> Result<glow::Program, String> {
    let program = gl.create_program()?;
    let mut shaders = vec![];
    for (kind, src) in [(glow::VERTEX_SHADER, vs), (glow::FRAGMENT_SHADER, fs)] {
        let sh = gl.create_shader(kind)?;
        gl.shader_source(sh, src);
        gl.compile_shader(sh);
        if !gl.get_shader_compile_status(sh) {
            return Err(gl.get_shader_info_log(sh));
        }
        gl.attach_shader(program, sh);
        shaders.push(sh);
    }
    gl.link_program(program);
    for sh in shaders {
        gl.detach_shader(program, sh);
        gl.delete_shader(sh);
    }
    if !gl.get_program_link_status(program) {
        return Err(gl.get_program_info_log(program));
    }
    Ok(program)
}

fn as_bytes<T>(v: &[T]) -> &[u8] {
    // Plain-old-data (f32/u16) slices → bytes for GL uploads.
    unsafe { std::slice::from_raw_parts(v.as_ptr() as *const u8, std::mem::size_of_val(v)) }
}

impl Xmb {
    pub fn new(gl: &Arc<glow::Context>) -> Result<Self, String> {
        unsafe {
            let bg_prog = compile(gl, BG_VS, BG_FS)?;
            let wave_prog = compile(gl, WAVE_VS, WAVE_FS)?;
            let pt_prog = compile(gl, PT_VS, PT_FS)?;

            // Fullscreen quad.
            let quad_vao = gl.create_vertex_array()?;
            gl.bind_vertex_array(Some(quad_vao));
            let qb = gl.create_buffer()?;
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(qb));
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, as_bytes(&[-1f32, -1., 1., -1., -1., 1., 1., 1.]), glow::STATIC_DRAW);
            gl.enable_vertex_attrib_array(0);
            gl.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, 0, 0);

            // 100×100 grid as one triangle strip with degenerate joins.
            let res = 100usize;
            let strips = res - 1;
            let vps = res * 2;
            let mut verts = Vec::with_capacity(strips * vps * 2);
            for y in 0..strips {
                for x in 0..res {
                    let fx = (x as f32 / (res - 1) as f32) * 2.0 - 1.0;
                    verts.push(fx);
                    verts.push(((y + 1) as f32 / (res - 1) as f32) * 2.0 - 1.0);
                    verts.push(fx);
                    verts.push((y as f32 / (res - 1) as f32) * 2.0 - 1.0);
                }
            }
            let mut idx: Vec<u16> = Vec::new();
            let mut base = 0u16;
            for s in 0..strips {
                if s > 0 {
                    idx.push(base - 1);
                    idx.push(base);
                }
                for i in 0..vps as u16 {
                    idx.push(base + i);
                }
                base += vps as u16;
            }
            let grid_vao = gl.create_vertex_array()?;
            gl.bind_vertex_array(Some(grid_vao));
            let vb = gl.create_buffer()?;
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vb));
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, as_bytes(&verts), glow::STATIC_DRAW);
            gl.enable_vertex_attrib_array(0);
            gl.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, 8, 0);
            let ib = gl.create_buffer()?;
            gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(ib));
            gl.buffer_data_u8_slice(glow::ELEMENT_ARRAY_BUFFER, as_bytes(&idx), glow::STATIC_DRAW);

            // Particle seeds (same distribution as particles.js).
            let mut rng = 0x2545F4914F6CDD1Du64;
            let mut rand = move || {
                rng ^= rng << 13;
                rng ^= rng >> 7;
                rng ^= rng << 17;
                (rng >> 11) as f32 / (1u64 << 53) as f32
            };
            let mut seeds = Vec::with_capacity(s::PT_COUNT * 3);
            for _ in 0..s::PT_COUNT {
                seeds.push(rand());
                seeds.push(rand());
                seeds.push(rand().powi(8) + 0.1);
            }
            let pt_vao = gl.create_vertex_array()?;
            gl.bind_vertex_array(Some(pt_vao));
            let pb = gl.create_buffer()?;
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(pb));
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, as_bytes(&seeds), glow::STATIC_DRAW);
            gl.enable_vertex_attrib_array(0);
            gl.vertex_attrib_pointer_f32(0, 3, glow::FLOAT, false, 0, 0);
            gl.bind_vertex_array(None);

            // Displacement texture (R32F, linear).
            let tex = gl.create_texture()?;
            gl.bind_texture(glow::TEXTURE_2D, Some(tex));
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::LINEAR as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
            let data = vec![0f32; TEX_W * TEX_H];
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::R32F as i32,
                TEX_W as i32,
                TEX_H as i32,
                0,
                glow::RED,
                glow::FLOAT,
                glow::PixelUnpackData::Slice(Some(as_bytes(&data))),
            );
            gl.bind_texture(glow::TEXTURE_2D, None);

            Ok(Xmb {
                bg_prog,
                wave_prog,
                pt_prog,
                quad_vao,
                grid_vao,
                grid_count: idx.len() as i32,
                pt_vao,
                tex,
                data,
                pipeline: SplinePipeline::new(),
                particle_offset: rand() * 1000.0,
            })
        }
    }

    /// Draw into the current viewport (set by egui to the callback rect).
    pub fn paint(&mut self, gl: &glow::Context, f: &Frame, viewport_px: [i32; 4], pixels_per_point: f32) {
        self.pipeline.write(f.time, &mut self.data);
        unsafe {
            gl.viewport(viewport_px[0], viewport_px[1], viewport_px[2], viewport_px[3]);
            gl.disable(glow::DEPTH_TEST);
            gl.disable(glow::CULL_FACE);

            // 1. Background gradient (top → bottom).
            gl.disable(glow::BLEND);
            gl.use_program(Some(self.bg_prog));
            let u = |p, n| gl.get_uniform_location(p, n);
            gl.uniform_3_f32(u(self.bg_prog, "uColorStart").as_ref(), f.top[0], f.top[1], f.top[2]);
            gl.uniform_3_f32(u(self.bg_prog, "uColorEnd").as_ref(), f.bottom[0], f.bottom[1], f.bottom[2]);
            gl.uniform_2_f32(u(self.bg_prog, "uDir").as_ref(), 0.0, 1.0);
            gl.uniform_1_f32(u(self.bg_prog, "uTMin").as_ref(), 0.0);
            gl.uniform_1_f32(u(self.bg_prog, "uTSpan").as_ref(), 1.0);
            gl.bind_vertex_array(Some(self.quad_vao));
            gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);

            // 2. Wave mesh.
            gl.bind_texture(glow::TEXTURE_2D, Some(self.tex));
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 4);
            gl.tex_sub_image_2d(
                glow::TEXTURE_2D,
                0,
                0,
                0,
                TEX_W as i32,
                TEX_H as i32,
                glow::RED,
                glow::FLOAT,
                glow::PixelUnpackData::Slice(Some(as_bytes(&self.data))),
            );
            gl.enable(glow::BLEND);
            gl.blend_func(glow::SRC_ALPHA, glow::ONE_MINUS_SRC_ALPHA);
            let w = self.wave_prog;
            gl.use_program(Some(w));
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(self.tex));
            gl.uniform_1_i32(u(w, "uSplineTex").as_ref(), 0);
            for (name, v) in [
                ("uTime", f.time),
                ("flowSpeed", s::FLOW_SPEED),
                ("tension", s::TENSION),
                ("damping", s::DAMPING),
                ("uLength", s::LENGTH),
                ("spacing", s::SPACING),
                ("perturbation", s::PERTURBATION),
                ("perturbationScale", s::PERTURBATION_SCALE),
                ("timeStep", s::TIME_STEP),
                ("waveCosAmp", s::WAVE_COS_AMP),
                ("waveBias", s::WAVE_BIAS),
                ("waveHeightScale", s::WAVE_HEIGHT_SCALE),
                ("waveSoftClip", s::WAVE_SOFT_CLIP),
                ("ffdYAmp", s::FFD_Y_AMP),
                ("ffdZAmp", s::FFD_Z_AMP),
                ("zDetailScale", s::Z_DETAIL_SCALE),
                ("opacity", s::OPACITY),
                ("brightness", s::BRIGHTNESS),
                ("fresnelPower", s::FRESNEL_POWER),
                ("fresnelScale", s::FRESNEL_SCALE),
            ] {
                gl.uniform_1_f32(u(w, name).as_ref(), v);
            }
            for (name, v) in [("ffdScale1", s::FFD_SCALE1), ("ffdScale2", s::FFD_SCALE2), ("ffdOffset", s::FFD_OFFSET)] {
                gl.uniform_3_f32(u(w, name).as_ref(), v[0], v[1], v[2]);
            }
            gl.bind_vertex_array(Some(self.grid_vao));
            gl.draw_elements(glow::TRIANGLE_STRIP, self.grid_count, glow::UNSIGNED_SHORT, 0);

            // 3. Sparkles (additive).
            gl.blend_func(glow::ONE, glow::ONE);
            gl.enable(glow::PROGRAM_POINT_SIZE);
            let p = self.pt_prog;
            gl.use_program(Some(p));
            let aspect = viewport_px[2] as f32 / viewport_px[3].max(1) as f32;
            gl.uniform_1_f32(u(p, "uTime").as_ref(), f.time + self.particle_offset);
            gl.uniform_1_f32(u(p, "flowSpeed").as_ref(), s::PT_FLOW_SPEED);
            gl.uniform_1_f32(u(p, "ratio").as_ref(), aspect.clamp(1.0, 2.0) * 0.375);
            gl.uniform_1_f32(u(p, "particleOpacity").as_ref(), s::PT_OPACITY);
            gl.uniform_1_f32(u(p, "ptSizeBase").as_ref(), s::PT_SIZE_BASE);
            gl.uniform_1_f32(u(p, "ptSizeVar").as_ref(), s::PT_SIZE_VAR);
            // Point sizes are in physical pixels, exactly like the web canvas.
            let _ = pixels_per_point;
            gl.uniform_1_f32(u(p, "uPxScale").as_ref(), 1.0);
            gl.bind_vertex_array(Some(self.pt_vao));
            gl.draw_arrays(glow::POINTS, 0, s::PT_COUNT as i32);

            gl.disable(glow::PROGRAM_POINT_SIZE);
            gl.bind_vertex_array(None);
            gl.use_program(None);
            gl.bind_texture(glow::TEXTURE_2D, None);
        }
    }
}
