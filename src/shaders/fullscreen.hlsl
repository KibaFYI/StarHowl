// ---------------------------------------------------------------- shared
struct VOut { float4 pos : SV_Position; float2 uv : TEXCOORD0; };

// One triangle that covers the whole target; no vertex buffer needed.
VOut vs_full(uint id : SV_VertexID)
{
    VOut o;
    float2 uv = float2((id << 1) & 2, id & 2);
    o.pos = float4(uv * float2(2.0, -2.0) + float2(-1.0, 1.0), 0.0, 1.0);
    o.uv = uv;
    return o;
}
