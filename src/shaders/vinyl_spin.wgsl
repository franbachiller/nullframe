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
@group(0) @binding(1) var artwork_texture: texture_2d<f32>;
@group(0) @binding(2) var artwork_sampler: sampler;

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

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    var uv = in.uv * 0.5 + vec2<f32>(0.5);
    uv.y = 1.0 - uv.y;
    
    // Normalize coordinates based on the MINIMUM canvas dimension to ensure the vinyl disc fits completely in 9:16, 16:9, 1:1 and 4:5
    let min_dim = max(min(u.resolution.x, u.resolution.y), 1.0);
    let p = (uv - vec2<f32>(0.5)) * 2.0 * (u.resolution / min_dim) * vec2<f32>(1.0, -1.0);

    let reaction = u.controls_a.x;
    let motion = u.controls_b.x;
    let kick = u.controls_b.y;

    // Rotation angle driven by time
    let spin_speed = 1.2 + motion * 1.5;
    let angle = u.time * spin_speed;
    let ca = cos(angle);
    let sa = sin(angle);

    // Subtle bass pulse scale
    let kick_scale = 1.0 + u.bass * reaction * kick * 0.035;
    let scaled_p = p / kick_scale;

    let rot_p = vec2<f32>(scaled_p.x * ca - scaled_p.y * sa, scaled_p.x * sa + scaled_p.y * ca);
    let r = length(scaled_p);

    // Pitch black background (0,0,0)
    var col = vec3<f32>(0.0, 0.0, 0.0);

    // Render ONLY the user's vinyl artwork image spinning in circular format
    let disc_radius = 0.85;
    if r <= disc_radius {
        // Map rotated position to texture UV [0, 1] and flip Y so image orientation is correct
        var art_uv = rot_p / (disc_radius * 2.0) + vec2<f32>(0.5);
        art_uv.y = 1.0 - art_uv.y;
        
        if art_uv.x >= 0.0 && art_uv.x <= 1.0 && art_uv.y >= 0.0 && art_uv.y <= 1.0 {
            let tex_sample = textureSample(artwork_texture, artwork_sampler, art_uv);
            let img_col = tex_sample.rgb;
            let img_alpha = tex_sample.a;

            // Soft anti-aliased edge mask for circular vinyl
            let edge_mask = smoothstep(disc_radius, disc_radius - 0.008, r) * img_alpha;

            // Subtle sheen / lighting reaction on kick
            let light_sheen = 1.0 + u.transient * reaction * 0.15 + u.bass * reaction * kick * 0.1;
            col = img_col * light_sheen * edge_mask;
        }
    }

    // Apply brightness control
    col *= u.controls_a.z;

    return vec4<f32>(col, 1.0);
}
