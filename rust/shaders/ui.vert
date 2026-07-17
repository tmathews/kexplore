// Rebuild: glslc shaders/ui.vert -o shaders/ui.vert.spv
#version 450

layout(location = 0) in vec2 in_pos;   // physical pixels
layout(location = 1) in vec2 in_uv;    // texture uv, or rect-local px for SDF modes
layout(location = 2) in vec4 in_color; // sRGB, straight alpha
layout(location = 3) in uint in_mode;
layout(location = 4) in vec4 in_extra; // SDF: half_w, half_h, corner_radius, border_width

layout(push_constant) uniform PC {
    vec2 viewport; // physical pixels
} pc;

layout(location = 0) out vec2 v_uv;
layout(location = 1) out vec4 v_color;
layout(location = 2) flat out uint v_mode;
layout(location = 3) out vec4 v_extra;

void main() {
    gl_Position = vec4(in_pos / pc.viewport * 2.0 - 1.0, 0.0, 1.0);
    v_uv = in_uv;
    v_color = in_color;
    v_mode = in_mode;
    v_extra = in_extra;
}
