// ---------------------------------------------------------------- YUV -> RGB
cbuffer Conv : register(b1)
{
    float4 cp0; // y offset, y scale, chroma scale, unused
    float4 cp1; // R from V, G from U, G from V, B from U
};
Texture2D<float4> in0 : register(t0);
Texture2D<float4> in1 : register(t1);

float3 yuv2rgb(float y, float u, float v)
{
    y = (y - cp0.x) * cp0.y;
    u = (u - 0.5) * cp0.z;
    v = (v - 0.5) * cp0.z;
    return saturate(float3(y + cp1.x * v, y - cp1.y * u - cp1.z * v, y + cp1.w * u));
}

// YUY2 is uploaded as RGBA8 at half width: R=Y0 G=U B=Y1 A=V.
float4 ps_yuy2(VOut i) : SV_Target
{
    int2 p = int2(i.pos.xy);
    float4 t = in0.Load(int3(p.x >> 1, p.y, 0));
    float y = (p.x & 1) ? t.b : t.r;
    return float4(yuv2rgb(y, t.g, t.a), 1.0);
}

// NV12: full-size Y plane (R8) + half-size interleaved UV plane (R8G8).
float4 ps_nv12(VOut i) : SV_Target
{
    int2 p = int2(i.pos.xy);
    float y = in0.Load(int3(p, 0)).r;
    float2 uv = in1.Load(int3(p >> 1, 0)).rg;
    return float4(yuv2rgb(y, uv.x, uv.y), 1.0);
}
