// ---------------------------------------------------------------- composite
cbuffer Comp : register(b0)
{
    float2 win;        // window size in pixels
    float2 tex;        // video frame size in pixels
    float4 vrect;      // video rectangle: x0 y0 x1 y1
    float4 frame_col;  // rgb + outline thickness in pixels
    uint4  zones;      // fill for left, right, top, bottom: 0 black 1 white 2 galaxy 3 image
    float4 img_rect;   // where the custom image lands: x0 y0 x1 y1
    float  time;
    uint   filter;     // 0 sharp 1 smooth 2 pixel
    uint   has_video;
    uint   twinkle;
    float  img_dim;
    float  star_density;
    uint   has_img;
    float  pad0;
};
Texture2D frame_tex : register(t0);
Texture2D bg_img : register(t1);
SamplerState s_lin : register(s0);
SamplerState s_pt : register(s1);

uint hash2(uint2 v)
{
    v = v * 1664525u + 1013904223u;
    v.x += v.y * 1664525u;
    v.y += v.x * 1664525u;
    v = v ^ (v >> 16u);
    v.x += v.y * 1664525u;
    v.y += v.x * 1664525u;
    v = v ^ (v >> 16u);
    return v.x ^ (v.y * 747796405u);
}

float rnd(uint2 c, uint salt)
{
    return (hash2(c + uint2(salt * 7919u, salt * 104729u)) & 0xFFFFFFu) / 16777216.0;
}

float vnoise(float2 p)
{
    float2 i = floor(p);
    float2 f = frac(p);
    float2 u = f * f * (3.0 - 2.0 * f);
    uint2 c = uint2(int2(i) + 4096);
    float a = rnd(c, 1u);
    float b = rnd(c + uint2(1, 0), 1u);
    float d = rnd(c + uint2(0, 1), 1u);
    float e = rnd(c + uint2(1, 1), 1u);
    return lerp(lerp(a, b, u.x), lerp(d, e, u.x), u.y);
}

float fbm(float2 p)
{
    float s = 0.0;
    float a = 0.5;
    [unroll] for (int k = 0; k < 4; k++)
    {
        s += a * vnoise(p);
        p = p * 2.03 + 17.1;
        a *= 0.5;
    }
    return s;
}

// Deep purple sky with faint nebula wisps and sparse tiny stars.
float3 galaxy(float2 p)
{
    float2 c = (p - 0.5 * win) / win.y;
    float r = length(c * float2(0.8, 1.0));
    float3 col = lerp(float3(0.105, 0.052, 0.200), float3(0.028, 0.014, 0.058), saturate(r * 1.15));

    float n1 = fbm(p / win.y * 2.2 + float2(3.1, 7.7));
    float n2 = fbm(p / win.y * 4.0 + float2(11.0, 2.0));
    col += float3(0.28, 0.10, 0.45) * smoothstep(0.50, 0.85, n1) * 0.22;
    col += float3(0.10, 0.14, 0.40) * smoothstep(0.55, 0.90, n2) * 0.10;

    // One possible star per 22 px cell, kept away from the cell edges.
    const float cell = 22.0;
    float2 gp = p / cell;
    int2 gi = int2(floor(gp));
    uint2 cu = uint2(gi + 8192);
    if (rnd(cu, 2u) < star_density)
    {
        float2 sp = (float2(gi) + 0.15 + 0.7 * float2(rnd(cu, 3u), rnd(cu, 4u))) * cell;
        float d = length(p - sp);
        float size = rnd(cu, 5u) > 0.93 ? 1.35 : 0.75;
        float bright = 0.45 + 0.55 * rnd(cu, 6u);
        if (twinkle != 0)
            bright *= 0.72 + 0.28 * sin(time * (0.6 + 1.6 * rnd(cu, 7u)) + 6.2831 * rnd(cu, 8u));
        float g = exp(-d * d / (size * size));
        float3 tint = lerp(float3(1.0, 0.97, 1.0), float3(0.78, 0.70, 1.0), rnd(cu, 9u));
        col += tint * g * bright;
    }
    return col;
}

float3 fill(uint kind, float2 p)
{
    if (kind == 0) return float3(0.0, 0.0, 0.0);
    if (kind == 1) return float3(1.0, 1.0, 1.0);
    if (kind == 3 && has_img != 0)
    {
        float2 uv = (p - img_rect.xy) / (img_rect.zw - img_rect.xy);
        if (any(uv < 0.0) || any(uv > 1.0)) return float3(0.0, 0.0, 0.0);
        float4 c = bg_img.SampleLevel(s_lin, uv, 0);
        return c.rgb * c.a * (1.0 - img_dim);
    }
    return galaxy(p);
}

float3 sample_video(float2 uv)
{
    if (filter == 2) return frame_tex.SampleLevel(s_pt, uv, 0).rgb;
    if (filter == 0)
    {
        // "Sharp bilinear": nearest-neighbour at whole steps, a 1px blend between.
        float2 vsize = vrect.zw - vrect.xy;
        float2 pre = max(floor(vsize / tex), 1.0);
        float2 texel = uv * tex;
        float2 tf = floor(texel);
        float2 s = frac(texel);
        float2 rr = 0.5 - 0.5 / pre;
        float2 cd = s - 0.5;
        float2 f = (cd - clamp(cd, -rr, rr)) * pre + 0.5;
        return frame_tex.SampleLevel(s_lin, (tf + f) / tex, 0).rgb;
    }
    return frame_tex.SampleLevel(s_lin, uv, 0).rgb;
}

float4 ps_comp(VOut i) : SV_Target
{
    float2 p = i.pos.xy;
    if (has_video != 0)
    {
        if (p.x >= vrect.x && p.x < vrect.z && p.y >= vrect.y && p.y < vrect.w)
        {
            float2 uv = (p - vrect.xy) / (vrect.zw - vrect.xy);
            return float4(sample_video(uv), 1.0);
        }
        float t = frame_col.a;
        if (t > 0.0 && p.x >= vrect.x - t && p.x < vrect.z + t && p.y >= vrect.y - t && p.y < vrect.w + t)
            return float4(frame_col.rgb, 1.0);
    }
    uint z = p.x < vrect.x ? zones.x : (p.x >= vrect.z ? zones.y : (p.y < vrect.y ? zones.z : zones.w));
    return float4(fill(z, p), 1.0);
}
