// Direct mapped-storage equivalent of normal_stack.wgsl.
struct Chunk { origin_size: vec4<i32>, count: vec4<u32>, };
struct Source { region: vec4<i32>, fallback: vec4<f32>, opacity: f32, };
struct Params { chunk: Chunk, sources: array<Source, 8>, };
struct Pixels { values: array<u32>, };
@group(0) @binding(0) var<storage, read> src0: Pixels;
@group(0) @binding(1) var<storage, read> src1: Pixels;
@group(0) @binding(2) var<storage, read> src2: Pixels;
@group(0) @binding(3) var<storage, read> src3: Pixels;
@group(0) @binding(4) var<storage, read> src4: Pixels;
@group(0) @binding(5) var<storage, read> src5: Pixels;
@group(0) @binding(6) var<storage, read> src6: Pixels;
@group(0) @binding(7) var<storage, read> src7: Pixels;
@group(0) @binding(8) var<uniform> params: Params;
struct VOut { @builtin(position) pos: vec4<f32> };
@vertex fn vs(@builtin(vertex_index) i: u32) -> VOut {
    var p = vec2<f32>(-1.0, -1.0);
    if (i == 1u) { p = vec2<f32>(3.0, -1.0); }
    if (i == 2u) { p = vec2<f32>(-1.0, 3.0); }
    var o: VOut; o.pos = vec4<f32>(p, 0.0, 1.0); return o;
}
fn read_source(i: u32, doc: vec2<i32>) -> vec4<f32> {
    let s = params.sources[i];
    if (doc.x < s.region.x || doc.y < s.region.y || doc.x >= s.region.x+s.region.z || doc.y >= s.region.y+s.region.w) { return s.fallback; }
    let p = doc - s.region.xy;
    let tile_columns = u32(s.region.z / 256);
    let tile_index = u32(p.y / 256) * tile_columns + u32(p.x / 256);
    let index = tile_index * 65536u + u32(p.y % 256) * 256u + u32(p.x % 256);
    switch i {
        case 0u: { return unpack4x8unorm(src0.values[index]); }
        case 1u: { return unpack4x8unorm(src1.values[index]); }
        case 2u: { return unpack4x8unorm(src2.values[index]); }
        case 3u: { return unpack4x8unorm(src3.values[index]); }
        case 4u: { return unpack4x8unorm(src4.values[index]); }
        case 5u: { return unpack4x8unorm(src5.values[index]); }
        case 6u: { return unpack4x8unorm(src6.values[index]); }
        default: { return unpack4x8unorm(src7.values[index]); }
    }
}
fn over(backdrop: vec4<f32>, source: vec4<f32>, opacity: f32) -> vec4<f32> {
    let ab = backdrop.a; let as_ = source.a * opacity;
    if (as_ <= 0.0) { return backdrop; }
    let ao = as_ + ab * (1.0-as_);
    if (ao <= 0.0) { return vec4<f32>(0.0); }
    let rgb = ((1.0-as_)*ab*backdrop.rgb + (1.0-ab)*as_*source.rgb + as_*ab*source.rgb)/ao;
    return vec4<f32>(rgb, ao);
}
@fragment fn fs(in: VOut) -> @location(0) vec4<f32> {
    let pos = vec2<i32>(floor(in.pos.xy)); let doc = params.chunk.origin_size.xy + pos;
    var result = vec4<f32>(0.0);
    for (var i=0u; i<params.chunk.count.x; i+=1u) { result = over(result, read_source(i, doc), params.sources[i].opacity); }
    return result;
}
