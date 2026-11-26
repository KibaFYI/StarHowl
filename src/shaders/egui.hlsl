// ---------------------------------------------------------------- egui
cbuffer Egui : register(b0)
{
    float2 screen_pts;
    float2 egui_pad;
};
struct EIn { float2 pos : POSITION; float2 uv : TEXCOORD0; float4 col : COLOR0; };
struct EOut { float4 pos : SV_Position; float2 uv : TEXCOORD0; float4 col : COLOR0; };
Texture2D etex : register(t0);
SamplerState esamp : register(s0);

EOut vs_egui(EIn i)
{
    EOut o;
    o.pos = float4(i.pos.x / screen_pts.x * 2.0 - 1.0, 1.0 - i.pos.y / screen_pts.y * 2.0, 0.0, 1.0);
    o.uv = i.uv;
    o.col = i.col;
    return o;
}

float4 ps_egui(EOut i) : SV_Target
{
    return i.col * etex.Sample(esamp, i.uv);
}
