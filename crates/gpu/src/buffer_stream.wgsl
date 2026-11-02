// Direct bounded reads for one source batch; binding 7 carries the prior result.
struct Chunk { origin_size: vec4<i32>, count: vec4<u32>, };
struct Source { region: vec4<i32>, fallback: vec4<f32>, opacity: f32, kind: u32, matrix: u32, lut_offset: u32, clip: vec4<i32>, };
struct Params { chunk: Chunk, sources: array<Source, 7>, };
struct Pixels { values: array<u32>, };
@group(0) @binding(0) var<storage, read> src0: Pixels;
@group(0) @binding(1) var<storage, read> src1: Pixels;
@group(0) @binding(2) var<storage, read> src2: Pixels;
@group(0) @binding(3) var<storage, read> src3: Pixels;
@group(0) @binding(4) var<storage, read> src4: Pixels;
@group(0) @binding(5) var<storage, read> src5: Pixels;
@group(0) @binding(6) var<storage, read> src6: Pixels;
@group(0) @binding(7) var previous: texture_2d<f32>;
@group(0) @binding(8) var<uniform> params: Params;
struct VOut { @builtin(position) pos: vec4<f32> };
@vertex fn vs(@builtin(vertex_index) i: u32) -> VOut {
    var p = vec2<f32>(-1.0, -1.0); if (i == 1u) { p = vec2<f32>(3.0, -1.0); } if (i == 2u) { p = vec2<f32>(-1.0, 3.0); }
    var o: VOut; o.pos = vec4<f32>(p,0.0,1.0); return o;
}
fn byte_at(i:u32, at:u32)->u32 {
    switch i { case 0u:{return (src0.values[at/4u] >> ((at%4u)*8u))&255u;} case 1u:{return (src1.values[at/4u] >> ((at%4u)*8u))&255u;} case 2u:{return (src2.values[at/4u] >> ((at%4u)*8u))&255u;} case 3u:{return (src3.values[at/4u] >> ((at%4u)*8u))&255u;} case 4u:{return (src4.values[at/4u] >> ((at%4u)*8u))&255u;} case 5u:{return (src5.values[at/4u] >> ((at%4u)*8u))&255u;} default:{return (src6.values[at/4u] >> ((at%4u)*8u))&255u;} }
}
fn read_source(i:u32, doc:vec2<i32>)->vec4<f32> {
    let s=params.sources[i];
    if (s.clip.z>0 && (doc.x<s.clip.x || doc.y<s.clip.y || doc.x>=s.clip.x+s.clip.z || doc.y>=s.clip.y+s.clip.w)) { return vec4<f32>(0.0); }
    if (doc.x<s.region.x || doc.y<s.region.y || doc.x>=s.region.x+s.region.z || doc.y>=s.region.y+s.region.w) { return s.fallback; }
    let p=doc-s.region.xy;
    if (s.kind==2u) {
        let w=u32(s.region.z); let h=u32(s.region.w); let x=u32(p.x); let y=u32(p.y);
        let ysize=w*h; let yy=f32(byte_at(i,y*w+x)); let uv=ysize+(y/2u)*w+(x/2u)*2u;
        let u=f32(byte_at(i,uv)); let v=f32(byte_at(i,uv+1u));
        var kr=0.299; var kb=0.114; if(s.matrix==1u){kr=0.2126;kb=0.0722;}
        let ylin=(yy-16.0)*(255.0/219.0); let cb=(u-128.0)*(255.0/224.0); let cr=(v-128.0)*(255.0/224.0); let kg=1.0-kr-kb;
        var rgb=clamp(vec3<f32>(ylin+2.0*(1.0-kr)*cr,ylin-2.0*kb*(1.0-kb)/kg*cb-2.0*kr*(1.0-kr)/kg*cr,ylin+2.0*(1.0-kb)*cb),vec3<f32>(0.0),vec3<f32>(255.0));
        rgb=floor(rgb+vec3<f32>(0.5));
        if(s.lut_offset!=0u){rgb=vec3<f32>(f32(byte_at(i,s.lut_offset+u32(rgb.x))),f32(byte_at(i,s.lut_offset+256u+u32(rgb.y))),f32(byte_at(i,s.lut_offset+512u+u32(rgb.z))));}
        return vec4<f32>(rgb/255.0,1.0);
    }
    var at:u32;
    if(s.kind==1u){at=(u32(p.y)*u32(s.region.z)+u32(p.x))*4u;} else {let cols=u32(s.region.z/256);let tile=u32(p.y/256)*cols+u32(p.x/256);at=tile*65536u+u32(p.y%256)*256u+u32(p.x%256);}
    if(s.kind==0u){switch i { case 0u:{return unpack4x8unorm(src0.values[at]);} case 1u:{return unpack4x8unorm(src1.values[at]);} case 2u:{return unpack4x8unorm(src2.values[at]);} case 3u:{return unpack4x8unorm(src3.values[at]);} case 4u:{return unpack4x8unorm(src4.values[at]);} case 5u:{return unpack4x8unorm(src5.values[at]);} default:{return unpack4x8unorm(src6.values[at]);} }}
    switch i { case 0u:{return unpack4x8unorm(src0.values[at/4u]);} case 1u:{return unpack4x8unorm(src1.values[at/4u]);} case 2u:{return unpack4x8unorm(src2.values[at/4u]);} case 3u:{return unpack4x8unorm(src3.values[at/4u]);} case 4u:{return unpack4x8unorm(src4.values[at/4u]);} case 5u:{return unpack4x8unorm(src5.values[at/4u]);} default:{return unpack4x8unorm(src6.values[at/4u]);} }
}
fn over(backdrop:vec4<f32>,source:vec4<f32>,opacity:f32)->vec4<f32>{let ab=backdrop.a;let as_=source.a*opacity;if(as_<=0.0){return backdrop;}let ao=as_+ab*(1.0-as_);if(ao<=0.0){return vec4<f32>(0.0);}let rgb=((1.0-as_)*ab*backdrop.rgb+(1.0-ab)*as_*source.rgb+as_*ab*source.rgb)/ao;return vec4<f32>(rgb,ao);}
@fragment fn fs(in:VOut)->@location(0) vec4<f32>{let pos=vec2<i32>(floor(in.pos.xy));let doc=params.chunk.origin_size.xy+pos;var result=vec4<f32>(0.0);if(params.chunk.count.y!=0u){result=textureLoad(previous,pos,0);}for(var i=0u;i<params.chunk.count.x;i+=1u){result=over(result,read_source(i,doc),params.sources[i].opacity);}return result;}
