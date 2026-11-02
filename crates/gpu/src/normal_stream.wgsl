// Bounded source-over batches: binding 0 is the previous batch result, bindings 1..7
// are the next source layers. The first batch ignores binding 0.
struct Chunk { origin_size: vec4<i32>, count: vec4<u32>, };
struct Source { region: vec4<i32>, fallback: vec4<f32>, opacity: f32, };
struct Params { chunk: Chunk, sources: array<Source, 8>, };
@group(0) @binding(0) var previous: texture_2d<f32>;
@group(0) @binding(1) var tex0: texture_2d<f32>;
@group(0) @binding(2) var tex1: texture_2d<f32>;
@group(0) @binding(3) var tex2: texture_2d<f32>;
@group(0) @binding(4) var tex3: texture_2d<f32>;
@group(0) @binding(5) var tex4: texture_2d<f32>;
@group(0) @binding(6) var tex5: texture_2d<f32>;
@group(0) @binding(7) var tex6: texture_2d<f32>;
@group(0) @binding(8) var<uniform> params: Params;
struct VOut { @builtin(position) pos: vec4<f32> };
@vertex fn vs(@builtin(vertex_index) i: u32) -> VOut {
    var p = vec2<f32>(-1.0, -1.0);
    if (i == 1u) { p = vec2<f32>(3.0, -1.0); }
    if (i == 2u) { p = vec2<f32>(-1.0, 3.0); }
    var o: VOut; o.pos = vec4<f32>(p, 0.0, 1.0); return o;
}
fn read_source(i: u32, pos: vec2<i32>, doc: vec2<i32>) -> vec4<f32> {
    let s = params.sources[i];
    if (doc.x < s.region.x || doc.y < s.region.y || doc.x >= s.region.x+s.region.z || doc.y >= s.region.y+s.region.w) { return s.fallback; }
    let p = doc - s.region.xy;
    switch i {
        case 0u: { return textureLoad(tex0,p,0); } case 1u: { return textureLoad(tex1,p,0); }
        case 2u: { return textureLoad(tex2,p,0); } case 3u: { return textureLoad(tex3,p,0); }
        case 4u: { return textureLoad(tex4,p,0); } case 5u: { return textureLoad(tex5,p,0); }
        default: { return textureLoad(tex6,p,0); }
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
    if (params.chunk.count.y != 0u) { result = textureLoad(previous, pos, 0); }
    for (var i=0u; i<params.chunk.count.x; i+=1u) { result = over(result, read_source(i,pos,doc), params.sources[i].opacity); }
    return result;
}
