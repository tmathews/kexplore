// One direction of a separable 9-tap gaussian, sampling a sub-region of src.
// Rebuild: glslc shaders/blur.frag -o shaders/blur.frag.spv
#version 450

layout(location = 0) in vec2 v_uv;

layout(set = 0, binding = 0) uniform sampler2D src;

layout(push_constant) uniform PC {
    vec2 uv_offset; // region of src to read
    vec2 uv_scale;
    vec2 dir;       // blur direction, pre-multiplied by src texel size
} pc;

layout(location = 0) out vec4 out_color;

const float W[5] = float[](0.2270270270, 0.1945945946, 0.1216216216, 0.0540540541, 0.0162162162);

void main() {
    vec2 base = pc.uv_offset + v_uv * pc.uv_scale;
    vec4 c = texture(src, base) * W[0];
    for (int i = 1; i < 5; i++) {
        c += texture(src, base + pc.dir * float(i)) * W[i];
        c += texture(src, base - pc.dir * float(i)) * W[i];
    }
    out_color = c;
}
