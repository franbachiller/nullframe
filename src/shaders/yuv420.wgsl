// Convierte el frame RGBA renderizado a YUV 4:2:0 planar (yuv420p), BT.709,
// rango limitado (16-235 / 16-240), empaquetado en bytes dentro de u32.
//
// Cada invocación procesa un bloque de 8x2 píxeles: escribe 2 palabras de
// luma por fila (8 bytes) y 1 palabra por plano de croma (4 muestras, cada una
// promedio de un bloque 2x2). Requiere ancho múltiplo de 8 y alto par.

struct Dims {
    width: u32,
    height: u32,
    _pad0: u32,
    _pad1: u32,
};

@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var<storage, read_write> dst: array<u32>;
@group(0) @binding(2) var<uniform> dims: Dims;

const KR: f32 = 0.2126;
const KG: f32 = 0.7152;
const KB: f32 = 0.0722;

fn to_byte(v: f32) -> u32 {
    return u32(clamp(round(v), 0.0, 255.0));
}

fn luma(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(KR, KG, KB));
}

fn load(x: u32, y: u32) -> vec3<f32> {
    return clamp(textureLoad(src, vec2<i32>(i32(x), i32(y)), 0).rgb, vec3<f32>(0.0), vec3<f32>(1.0));
}

fn pack4(a: u32, b: u32, c: u32, d: u32) -> u32 {
    return a | (b << 8u) | (c << 16u) | (d << 24u);
}

@compute @workgroup_size(8, 8)
fn cs_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let x0 = gid.x * 8u;
    let y0 = gid.y * 2u;
    if x0 >= dims.width || y0 >= dims.height {
        return;
    }

    let w = dims.width;
    var y_top: array<u32, 8>;
    var y_bottom: array<u32, 8>;
    var cb: array<u32, 4>;
    var cr: array<u32, 4>;

    for (var k = 0u; k < 4u; k++) {
        let xa = x0 + k * 2u;
        let a = load(xa, y0);
        let b = load(xa + 1u, y0);
        let c = load(xa, y0 + 1u);
        let d = load(xa + 1u, y0 + 1u);

        y_top[k * 2u] = to_byte(16.0 + 219.0 * luma(a));
        y_top[k * 2u + 1u] = to_byte(16.0 + 219.0 * luma(b));
        y_bottom[k * 2u] = to_byte(16.0 + 219.0 * luma(c));
        y_bottom[k * 2u + 1u] = to_byte(16.0 + 219.0 * luma(d));

        let avg = (a + b + c + d) * 0.25;
        let l = luma(avg);
        cb[k] = to_byte(128.0 + 224.0 * (avg.b - l) / (2.0 * (1.0 - KB)));
        cr[k] = to_byte(128.0 + 224.0 * (avg.r - l) / (2.0 * (1.0 - KR)));
    }

    let top = (y0 * w + x0) / 4u;
    let bottom = ((y0 + 1u) * w + x0) / 4u;
    dst[top] = pack4(y_top[0], y_top[1], y_top[2], y_top[3]);
    dst[top + 1u] = pack4(y_top[4], y_top[5], y_top[6], y_top[7]);
    dst[bottom] = pack4(y_bottom[0], y_bottom[1], y_bottom[2], y_bottom[3]);
    dst[bottom + 1u] = pack4(y_bottom[4], y_bottom[5], y_bottom[6], y_bottom[7]);

    let chroma_w = w / 2u;
    let chroma_plane = chroma_w * (dims.height / 2u);
    let chroma_index = (gid.y * chroma_w + gid.x * 4u) / 4u;
    let u_base = (w * dims.height) / 4u;
    let v_base = u_base + chroma_plane / 4u;
    dst[u_base + chroma_index] = pack4(cb[0], cb[1], cb[2], cb[3]);
    dst[v_base + chroma_index] = pack4(cr[0], cr[1], cr[2], cr[3]);
}
