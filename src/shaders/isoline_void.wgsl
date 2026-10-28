struct Uniforms {
    resolution: vec2<f32>,
    time: f32,
    bass: f32,
    mids: f32,
    treble: f32,
    rms: f32,
    transient: f32,
    controls_a: vec4<f32>,
    controls_b: vec4<f32>,
};
@group(0) @binding(0) var<uniform> u: Uniforms;

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> VertexOut {
    var out: VertexOut;
    let x = f32(i32(index & 1u) * 4 - 1);
    let y = f32(i32(index >> 1u) * 4 - 1);
    out.position = vec4<f32>(x, y, 0.0, 1.0);
    out.uv = vec2<f32>(x, y);
    return out;
}

fn hash21(p: vec2<f32>) -> f32 {
    let q = fract(p * vec2<f32>(123.34, 456.21));
    return fract(dot(q, q + 45.32));
}

fn field(p: vec2<f32>) -> f32 {
    let reaction = u.controls_a.x;
    let motion = u.controls_b.x;
    let detail = u.controls_b.w;
    let time = u.time * (0.20 + motion * 0.45);
    let scale = 1.6 + detail * 2.8;

    var v = sin(p.x * scale + time * 0.8) * 0.4;
    v += sin(p.y * scale * 1.2 - time * 0.6) * 0.35;
    v += sin((p.x + p.y) * scale * 0.8 + time * 1.1) * 0.25;
    v += sin((p.x - p.y) * scale * 1.4 - time * 0.7) * 0.20;

    for (var i = 0; i < 4; i++) {
        let fi = f32(i);
        let ang = time * (0.25 + fi * 0.08) + fi * 1.57;
        let rad = 0.5 + 0.2 * sin(time * 0.5 + fi);
        let center = vec2<f32>(cos(ang), sin(ang)) * rad;
        let delta = p - center;
        let band_val = select(u.bass, select(u.mids, u.treble, i > 2), i > 0);
        let amp = 0.35 + reaction * band_val * 1.2;
        v += amp * exp(-dot(delta, delta) * (3.5 + detail * 2.0));
    }

    let r = length(p);
    let kick_pulse = u.transient * u.controls_b.y * reaction;
    v += kick_pulse * 0.8 * sin(r * 18.0 - u.time * 10.0) * exp(-r * 1.8);
    return v;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    var uv = in.uv * 0.5 + vec2<f32>(0.5);
    uv.y = 1.0 - uv.y;
    let asp = u.resolution.x / max(u.resolution.y, 1.0);
    let p = (uv - vec2<f32>(0.5)) * 2.0 * vec2<f32>(asp, -1.0);

    let v = field(p);
    let spacing = 0.12 - u.controls_b.w * 0.06 - u.treble * u.controls_a.x * 0.03;
    let scaled = v / max(spacing, 0.005);
    let dist_line = abs(abs(fract(scaled) - 0.5) - 0.5);
    let width = fwidth(scaled) + 0.0001;
    let line = 1.0 - smoothstep(0.0, width * 1.8, dist_line);

    // Dark underground palette: Obsidian base, deep amber / platinum silver accent
    let hue_val = u.controls_a.w;
    let amber = vec3<f32>(0.95, 0.45, 0.08);
    let silver = vec3<f32>(0.85, 0.90, 0.95);
    let crimson = vec3<f32>(0.85, 0.10, 0.15);
    let emerald = vec3<f32>(0.10, 0.80, 0.55);

    var base_color = mix(amber, silver, step(0.33, hue_val));
    if hue_val > 0.66 {
        base_color = mix(crimson, emerald, (hue_val - 0.66) * 3.0);
    }

    let kick_bright = u.bass * u.controls_a.x * u.controls_b.y * 0.6 + u.transient * 0.4;
    var col = base_color * line * (0.8 + kick_bright);
    col += vec3<f32>(line * line) * 0.5 * (1.0 + u.controls_b.z);

    // Core glow
    let r = length(p);
    col += base_color * 0.06 * smoothstep(1.5, 0.0, r) * (1.0 + u.rms * u.controls_a.x);

    // Film grain & dark vignette
    let grain = (hash21(uv * u.time) - 0.5) * 0.04;
    col += vec3<f32>(grain);
    let vign = smoothstep(1.4, 0.3, r);
    col *= vign;

    // Tonemap & Gamma
    let bright = u.controls_a.z * (0.9 + u.controls_b.z * 0.5 + u.rms * 0.3);
    col = 1.0 - exp(-col * bright);
    col = pow(col, vec3<f32>(0.95));

    return vec4<f32>(col, 1.0);
}
