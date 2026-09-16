#include <metal_stdlib>

using namespace metal;

// Aspect-fit present for the viewport: the fullscreen quad is letterboxed
// via the encoder viewport (`draw(in:)` sets it to `aspectFitRect`), so the
// shader just samples the adopted frame texture. UVs are Y-flipped so
// texture row 0 (top) lands on NDC y = +1 (screen top), preserving the
// orientation of the previous 1:1 blit path.
struct ViewportPresentOut {
    float4 position [[position]];
    float2 texCoord;
};

vertex ViewportPresentOut viewportPresentVertex(uint vertexID [[vertex_id]]) {
    float2 positions[4] = {
        float2(-1.0, -1.0),
        float2(1.0, -1.0),
        float2(-1.0, 1.0),
        float2(1.0, 1.0)
    };
    float2 texCoords[4] = {
        float2(0.0, 1.0),
        float2(1.0, 1.0),
        float2(0.0, 0.0),
        float2(1.0, 0.0)
    };
    ViewportPresentOut out;
    out.position = float4(positions[vertexID], 0.0, 1.0);
    out.texCoord = texCoords[vertexID];
    return out;
}

fragment float4 viewportPresentFragment(
    ViewportPresentOut in [[stage_in]],
    texture2d<float> frameTexture [[texture(0)]],
    sampler frameSampler [[sampler(0)]]) {
    return frameTexture.sample(frameSampler, in.texCoord);
}
