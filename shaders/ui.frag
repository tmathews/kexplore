// Rebuild: glslc shaders/ui.frag -o shaders/ui.frag.spv
#version 450

layout(location = 0) in vec2 v_uv;
layout(location = 1) in vec4 v_color;
layout(location = 2) flat in uint v_mode;
layout(location = 3) in vec4 v_extra;

layout(set = 0, binding = 0) uniform sampler2D tex;

layout(location = 0) out vec4 out_color; // premultiplied

const uint MODE_SOLID = 0u;
const uint MODE_RRECT_FILL = 1u;
const uint MODE_RRECT_BORDER = 2u;
const uint MODE_GLYPH = 3u;
const uint MODE_IMAGE = 4u;

// Signed distance to a rounded rect centered at origin.
float rrect_sd(vec2 p, vec2 half_size, float radius) {
    vec2 q = abs(p) - half_size + radius;
    return length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - radius;
}

void main() {
    vec4 c = v_color;
    if (v_mode == MODE_RRECT_FILL) {
        float sd = rrect_sd(v_uv, v_extra.xy, v_extra.z);
        c.a *= clamp(0.5 - sd, 0.0, 1.0);
    } else if (v_mode == MODE_RRECT_BORDER) {
        float sd = rrect_sd(v_uv, v_extra.xy, v_extra.z);
        c.a *= clamp(0.5 - (abs(sd) - v_extra.w * 0.5), 0.0, 1.0);
    } else if (v_mode == MODE_GLYPH) {
        c.a *= textureLod(tex, v_uv, 0.0).r;
    } else if (v_mode == MODE_IMAGE) {
        out_color = textureLod(tex, v_uv, 0.0); // uploaded premultiplied
        return;
    }
    out_color = vec4(c.rgb * c.a, c.a);
}
