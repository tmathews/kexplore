//! Single-pipeline 2D batch renderer. CPU-built quads, SDF rounded rects in
//! the fragment shader, premultiplied blending. The draw API takes logical
//! pixels and multiplies by the output scale when emitting vertices.

use ash::vk;

use super::upload::{self, Buffer, PendingUpload, Texture};
use super::FRAMES_IN_FLIGHT;
use crate::geom::{Point, Rect};

pub const MODE_SOLID: u32 = 0;
pub const MODE_RRECT_FILL: u32 = 1;
pub const MODE_RRECT_BORDER: u32 = 2;
pub const MODE_GLYPH: u32 = 3;
pub const MODE_IMAGE: u32 = 4;

/// Anti-aliasing margin added around SDF quads, in physical pixels.
const AA_MARGIN: f32 = 1.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rgba(pub [u8; 4]);

impl Rgba {
    pub const WHITE: Rgba = Rgba([255, 255, 255, 255]);

    pub fn new(r: f32, g: f32, b: f32, a: f32) -> Rgba {
        Rgba([
            (r.clamp(0.0, 1.0) * 255.0).round() as u8,
            (g.clamp(0.0, 1.0) * 255.0).round() as u8,
            (b.clamp(0.0, 1.0) * 255.0).round() as u8,
            (a.clamp(0.0, 1.0) * 255.0).round() as u8,
        ])
    }
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub pos: [f32; 2],
    pub uv: [f32; 2],
    pub color: [u8; 4],
    pub mode: u32,
    pub extra: [f32; 4],
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TexSlot {
    Atlas,
    Preview,
    /// Offscreen canvas texture, composited by the overlay pass.
    Scene,
    /// Blurred toolbar band.
    Blur,
}

/// Descriptor sets for each slot a draw list may reference. Missing optional
/// slots fall back to the atlas (harmless: nothing meaningful samples them).
#[derive(Clone, Copy)]
pub struct TexSets {
    pub atlas: vk::DescriptorSet,
    pub preview: Option<vk::DescriptorSet>,
    pub scene: Option<vk::DescriptorSet>,
    pub blur: Option<vk::DescriptorSet>,
}

impl TexSets {
    fn get(&self, slot: TexSlot) -> vk::DescriptorSet {
        match slot {
            TexSlot::Atlas => self.atlas,
            TexSlot::Preview => self.preview.unwrap_or(self.atlas),
            TexSlot::Scene => self.scene.unwrap_or(self.atlas),
            TexSlot::Blur => self.blur.unwrap_or(self.atlas),
        }
    }
}

struct Range {
    first: u32,
    count: u32,
    tex: TexSlot,
}

/// One frame's worth of geometry, built immediate-mode by ui code.
pub struct DrawList {
    verts: Vec<Vertex>,
    ranges: Vec<Range>,
    /// logical -> physical pixel scale
    pub scale: f32,
}

impl DrawList {
    pub fn new(scale: f32) -> DrawList {
        DrawList {
            verts: Vec::with_capacity(4096),
            ranges: vec![Range { first: 0, count: 0, tex: TexSlot::Atlas }],
            scale,
        }
    }

    fn require_tex(&mut self, tex: TexSlot) {
        let cur = self.ranges.last_mut().unwrap();
        if cur.tex != tex {
            if cur.count == 0 {
                cur.tex = tex;
            } else {
                let first = cur.first + cur.count;
                self.ranges.push(Range { first, count: 0, tex });
            }
        }
    }

    fn push_quad(&mut self, corners: [[f32; 2]; 4], uvs: [[f32; 2]; 4], color: Rgba, mode: u32, extra: [f32; 4]) {
        let v = |i: usize| Vertex { pos: corners[i], uv: uvs[i], color: color.0, mode, extra };
        // Two CCW triangles: 0-1-2, 0-2-3 (corners ordered TL, TR, BR, BL).
        self.verts.extend_from_slice(&[v(0), v(1), v(2), v(0), v(2), v(3)]);
        self.ranges.last_mut().unwrap().count += 6;
    }

    /// Axis-aligned SDF quad centered on `center` (physical px), half size
    /// `half` (physical px), inflated by `inflate` for AA/border coverage.
    fn sdf_quad(&mut self, center: [f32; 2], half: [f32; 2], inflate: f32, color: Rgba, mode: u32, extra: [f32; 4]) {
        let hx = half[0] + inflate;
        let hy = half[1] + inflate;
        let corners = [
            [center[0] - hx, center[1] - hy],
            [center[0] + hx, center[1] - hy],
            [center[0] + hx, center[1] + hy],
            [center[0] - hx, center[1] + hy],
        ];
        let uvs = [[-hx, -hy], [hx, -hy], [hx, hy], [-hx, hy]];
        self.push_quad(corners, uvs, color, mode, extra);
    }

    /// Filled rounded rect. `r` in logical px, `radius` in logical px.
    pub fn rect(&mut self, r: Rect, color: Rgba, radius: f32) {
        let s = self.scale;
        let half = [r.width() * 0.5 * s, r.height() * 0.5 * s];
        let center = [r.center().x * s, r.center().y * s];
        self.sdf_quad(
            center,
            half,
            AA_MARGIN,
            color,
            MODE_RRECT_FILL,
            [half[0], half[1], radius * s, 0.0],
        );
    }

    /// Stroked rounded rect; the stroke straddles the rect edge like a cairo
    /// stroke does.
    pub fn rect_stroke(&mut self, r: Rect, color: Rgba, radius: f32, width: f32) {
        let s = self.scale;
        let half = [r.width() * 0.5 * s, r.height() * 0.5 * s];
        let center = [r.center().x * s, r.center().y * s];
        let w_phys = (width * s).max(1.0);
        self.sdf_quad(
            center,
            half,
            w_phys * 0.5 + AA_MARGIN,
            color,
            MODE_RRECT_BORDER,
            [half[0], half[1], radius * s, w_phys],
        );
    }

    /// Line segment as a rotated SDF quad.
    pub fn line(&mut self, a: Point, b: Point, width: f32, color: Rgba) {
        let s = self.scale;
        let ax = a.x * s;
        let ay = a.y * s;
        let bx = b.x * s;
        let by = b.y * s;
        let dx = bx - ax;
        let dy = by - ay;
        let len = (dx * dx + dy * dy).sqrt();
        if len < 0.001 {
            return;
        }
        let (ux, uy) = (dx / len, dy / len); // unit along the line
        let (nx, ny) = (-uy, ux); // unit normal
        let cx = (ax + bx) * 0.5;
        let cy = (ay + by) * 0.5;
        let hw = len * 0.5;
        let hh = (width * s).max(1.0) * 0.5;
        let (ihw, ihh) = (hw + AA_MARGIN, hh + AA_MARGIN);
        let corners = [
            [cx - ux * ihw - nx * ihh, cy - uy * ihw - ny * ihh],
            [cx + ux * ihw - nx * ihh, cy + uy * ihw - ny * ihh],
            [cx + ux * ihw + nx * ihh, cy + uy * ihw + ny * ihh],
            [cx - ux * ihw + nx * ihh, cy - uy * ihw + ny * ihh],
        ];
        let uvs = [[-ihw, -ihh], [ihw, -ihh], [ihw, ihh], [-ihw, ihh]];
        self.push_quad(corners, uvs, color, MODE_RRECT_FILL, [hw, hh, 0.0, 0.0]);
    }

    /// Full-bleed solid rect (no AA) — for bars/background fills on pixel
    /// boundaries.
    pub fn solid(&mut self, r: Rect, color: Rgba) {
        let s = self.scale;
        let (x0, y0) = (r.min.x * s, r.min.y * s);
        let (x1, y1) = (r.max.x * s, r.max.y * s);
        self.push_quad(
            [[x0, y0], [x1, y0], [x1, y1], [x0, y1]],
            [[0.0; 2]; 4],
            color,
            MODE_SOLID,
            [0.0; 4],
        );
    }

    /// Textured quad sampling the glyph atlas as an alpha mask, tinted with
    /// `color`, optionally rotated around the rect center (for the spinner).
    pub fn glyph_quad(&mut self, r: Rect, uv: [f32; 4], color: Rgba, rotation: f32) {
        self.require_tex(TexSlot::Atlas);
        let s = self.scale;
        let c = r.center();
        let (cx, cy) = (c.x * s, c.y * s);
        let hw = r.width() * 0.5 * s;
        let hh = r.height() * 0.5 * s;
        let (sin, cos) = rotation.sin_cos();
        let rot = |x: f32, y: f32| [cx + x * cos - y * sin, cy + x * sin + y * cos];
        let corners = [rot(-hw, -hh), rot(hw, -hh), rot(hw, hh), rot(-hw, hh)];
        let uvs = [[uv[0], uv[1]], [uv[2], uv[1]], [uv[2], uv[3]], [uv[0], uv[3]]];
        self.push_quad(corners, uvs, color, MODE_GLYPH, [0.0; 4]);
    }

    /// Glyph quad in physical pixel coordinates (text rendering rounds
    /// per-glyph positions to physical pixels itself).
    pub fn glyph_quad_phys(&mut self, r: [f32; 4], uv: [f32; 4], color: Rgba) {
        self.require_tex(TexSlot::Atlas);
        let corners = [[r[0], r[1]], [r[2], r[1]], [r[2], r[3]], [r[0], r[3]]];
        let uvs = [[uv[0], uv[1]], [uv[2], uv[1]], [uv[2], uv[3]], [uv[0], uv[3]]];
        self.push_quad(corners, uvs, color, MODE_GLYPH, [0.0; 4]);
    }

    /// Textured RGBA quad (the preview image).
    pub fn image(&mut self, r: Rect) {
        self.image_slot(r, TexSlot::Preview);
    }

    /// Full-uv textured quad sampling an arbitrary slot (scene composite,
    /// blurred band).
    pub fn image_slot(&mut self, r: Rect, slot: TexSlot) {
        self.require_tex(slot);
        let s = self.scale;
        let (x0, y0) = (r.min.x * s, r.min.y * s);
        let (x1, y1) = (r.max.x * s, r.max.y * s);
        self.push_quad(
            [[x0, y0], [x1, y0], [x1, y1], [x0, y1]],
            [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
            Rgba::WHITE,
            MODE_IMAGE,
            [0.0; 4],
        );
    }

    pub fn is_empty(&self) -> bool {
        self.verts.is_empty()
    }

    pub fn byte_size(&self) -> u64 {
        (self.verts.len() * std::mem::size_of::<Vertex>()) as u64
    }
}

struct FrameBuffers {
    vertex: Buffer,
    staging: Buffer,
}

pub struct Renderer2d {
    mem_props: vk::PhysicalDeviceMemoryProperties,
    pub desc_layout: vk::DescriptorSetLayout,
    desc_pool: vk::DescriptorPool,
    pub sampler: vk::Sampler,
    pipeline_layout: vk::PipelineLayout,
    pipeline: vk::Pipeline,
    frames: Vec<FrameBuffers>,
}

const VERTEX_BUF_START: u64 = 256 * 1024;
const STAGING_BUF_START: u64 = 1024 * 1024;

impl Renderer2d {
    pub fn new(
        device: &ash::Device,
        mem_props: vk::PhysicalDeviceMemoryProperties,
        color_format: vk::Format,
    ) -> Result<Renderer2d, String> {
        unsafe { Self::new_inner(device, mem_props, color_format) }
    }

    unsafe fn new_inner(
        device: &ash::Device,
        mem_props: vk::PhysicalDeviceMemoryProperties,
        color_format: vk::Format,
    ) -> Result<Renderer2d, String> {
        let bindings = [vk::DescriptorSetLayoutBinding::default()
            .binding(0)
            .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::FRAGMENT)];
        let desc_layout = device
            .create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings),
                None,
            )
            .map_err(|e| format!("desc layout: {e}"))?;

        let pool_sizes = [vk::DescriptorPoolSize::default()
            .ty(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .descriptor_count(16)];
        let desc_pool = device
            .create_descriptor_pool(
                &vk::DescriptorPoolCreateInfo::default()
                    .flags(vk::DescriptorPoolCreateFlags::FREE_DESCRIPTOR_SET)
                    .max_sets(16)
                    .pool_sizes(&pool_sizes),
                None,
            )
            .map_err(|e| format!("desc pool: {e}"))?;

        let sampler = device
            .create_sampler(
                &vk::SamplerCreateInfo::default()
                    .mag_filter(vk::Filter::LINEAR)
                    .min_filter(vk::Filter::LINEAR)
                    .mipmap_mode(vk::SamplerMipmapMode::NEAREST)
                    .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE),
                None,
            )
            .map_err(|e| format!("sampler: {e}"))?;

        let pc_range = vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::VERTEX)
            .size(8);
        let set_layouts = [desc_layout];
        let pipeline_layout = device
            .create_pipeline_layout(
                &vk::PipelineLayoutCreateInfo::default()
                    .set_layouts(&set_layouts)
                    .push_constant_ranges(std::slice::from_ref(&pc_range)),
                None,
            )
            .map_err(|e| format!("pipeline layout: {e}"))?;

        let vert_spv = ash::util::read_spv(&mut std::io::Cursor::new(
            &include_bytes!("../../shaders/ui.vert.spv")[..],
        ))
        .map_err(|e| format!("vert spv: {e}"))?;
        let frag_spv = ash::util::read_spv(&mut std::io::Cursor::new(
            &include_bytes!("../../shaders/ui.frag.spv")[..],
        ))
        .map_err(|e| format!("frag spv: {e}"))?;
        let vert_mod = device
            .create_shader_module(&vk::ShaderModuleCreateInfo::default().code(&vert_spv), None)
            .map_err(|e| format!("vert module: {e}"))?;
        let frag_mod = device
            .create_shader_module(&vk::ShaderModuleCreateInfo::default().code(&frag_spv), None)
            .map_err(|e| format!("frag module: {e}"))?;

        let stages = [
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::VERTEX)
                .module(vert_mod)
                .name(c"main"),
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::FRAGMENT)
                .module(frag_mod)
                .name(c"main"),
        ];

        let vertex_bindings = [vk::VertexInputBindingDescription::default()
            .binding(0)
            .stride(std::mem::size_of::<Vertex>() as u32)
            .input_rate(vk::VertexInputRate::VERTEX)];
        let vertex_attrs = [
            vk::VertexInputAttributeDescription::default()
                .location(0)
                .format(vk::Format::R32G32_SFLOAT)
                .offset(0),
            vk::VertexInputAttributeDescription::default()
                .location(1)
                .format(vk::Format::R32G32_SFLOAT)
                .offset(8),
            vk::VertexInputAttributeDescription::default()
                .location(2)
                .format(vk::Format::R8G8B8A8_UNORM)
                .offset(16),
            vk::VertexInputAttributeDescription::default()
                .location(3)
                .format(vk::Format::R32_UINT)
                .offset(20),
            vk::VertexInputAttributeDescription::default()
                .location(4)
                .format(vk::Format::R32G32B32A32_SFLOAT)
                .offset(24),
        ];
        let vertex_input = vk::PipelineVertexInputStateCreateInfo::default()
            .vertex_binding_descriptions(&vertex_bindings)
            .vertex_attribute_descriptions(&vertex_attrs);

        let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
            .topology(vk::PrimitiveTopology::TRIANGLE_LIST);
        let viewport_state =
            vk::PipelineViewportStateCreateInfo::default().viewport_count(1).scissor_count(1);
        let raster = vk::PipelineRasterizationStateCreateInfo::default()
            .polygon_mode(vk::PolygonMode::FILL)
            .cull_mode(vk::CullModeFlags::NONE)
            .front_face(vk::FrontFace::COUNTER_CLOCKWISE)
            .line_width(1.0);
        let multisample = vk::PipelineMultisampleStateCreateInfo::default()
            .rasterization_samples(vk::SampleCountFlags::TYPE_1);
        let blend_attachment = [vk::PipelineColorBlendAttachmentState::default()
            .blend_enable(true)
            .src_color_blend_factor(vk::BlendFactor::ONE)
            .dst_color_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
            .color_blend_op(vk::BlendOp::ADD)
            .src_alpha_blend_factor(vk::BlendFactor::ONE)
            .dst_alpha_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
            .alpha_blend_op(vk::BlendOp::ADD)
            .color_write_mask(vk::ColorComponentFlags::RGBA)];
        let blend = vk::PipelineColorBlendStateCreateInfo::default().attachments(&blend_attachment);
        let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
        let dynamic = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);

        let color_formats = [color_format];
        let mut rendering_info =
            vk::PipelineRenderingCreateInfo::default().color_attachment_formats(&color_formats);

        let pipeline_info = vk::GraphicsPipelineCreateInfo::default()
            .stages(&stages)
            .vertex_input_state(&vertex_input)
            .input_assembly_state(&input_assembly)
            .viewport_state(&viewport_state)
            .rasterization_state(&raster)
            .multisample_state(&multisample)
            .color_blend_state(&blend)
            .dynamic_state(&dynamic)
            .layout(pipeline_layout)
            .push_next(&mut rendering_info);
        let pipeline = device
            .create_graphics_pipelines(vk::PipelineCache::null(), &[pipeline_info], None)
            .map_err(|(_, e)| format!("create pipeline: {e}"))?[0];

        device.destroy_shader_module(vert_mod, None);
        device.destroy_shader_module(frag_mod, None);

        let mut frames = Vec::new();
        for _ in 0..FRAMES_IN_FLIGHT {
            frames.push(FrameBuffers {
                vertex: upload::create_host_buffer(
                    device,
                    &mem_props,
                    VERTEX_BUF_START,
                    vk::BufferUsageFlags::VERTEX_BUFFER,
                )?,
                staging: upload::create_host_buffer(
                    device,
                    &mem_props,
                    STAGING_BUF_START,
                    vk::BufferUsageFlags::TRANSFER_SRC,
                )?,
            });
        }

        Ok(Renderer2d {
            mem_props,
            desc_layout,
            desc_pool,
            sampler,
            pipeline_layout,
            pipeline,
            frames,
        })
    }

    /// Allocate and write a descriptor set for sampling `texture`.
    pub fn register_texture(
        &self,
        device: &ash::Device,
        texture: &Texture,
    ) -> Result<vk::DescriptorSet, String> {
        unsafe {
            let layouts = [self.desc_layout];
            let set = device
                .allocate_descriptor_sets(
                    &vk::DescriptorSetAllocateInfo::default()
                        .descriptor_pool(self.desc_pool)
                        .set_layouts(&layouts),
                )
                .map_err(|e| format!("alloc descriptor: {e}"))?[0];
            let image_info = [vk::DescriptorImageInfo::default()
                .sampler(self.sampler)
                .image_view(texture.view)
                .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
            let write = vk::WriteDescriptorSet::default()
                .dst_set(set)
                .dst_binding(0)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .image_info(&image_info);
            device.update_descriptor_sets(&[write], &[]);
            Ok(set)
        }
    }

    pub fn free_texture_set(&self, device: &ash::Device, set: vk::DescriptorSet) {
        unsafe {
            device.free_descriptor_sets(self.desc_pool, &[set]).ok();
        }
    }

    /// Phase 1, outside the render pass: upload the vertices of all lists
    /// (concatenated) + pending texture data. Must run after this frame's
    /// fence has been waited on. Returns each list's base vertex offset.
    pub fn record_pre(
        &mut self,
        device: &ash::Device,
        cmd: vk::CommandBuffer,
        frame_idx: usize,
        lists: &[&DrawList],
        uploads: &[PendingUpload],
    ) -> Result<Vec<u32>, String> {
        let fb = &mut self.frames[frame_idx];

        let vb_size: u64 = lists.iter().map(|l| l.byte_size()).sum();
        if vb_size > fb.vertex.size {
            unsafe { upload::destroy_buffer(device, &fb.vertex) };
            fb.vertex = upload::create_host_buffer(
                device,
                &self.mem_props,
                vb_size.next_power_of_two(),
                vk::BufferUsageFlags::VERTEX_BUFFER,
            )?;
        }
        let mut offsets = Vec::with_capacity(lists.len());
        let mut byte_off = 0usize;
        let mut vert_off = 0u32;
        for list in lists {
            offsets.push(vert_off);
            let bytes: &[u8] = bytemuck::cast_slice(&list.verts);
            if !bytes.is_empty() {
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        bytes.as_ptr(),
                        fb.vertex.mapped.add(byte_off),
                        bytes.len(),
                    );
                }
            }
            byte_off += bytes.len();
            vert_off += list.verts.len() as u32;
        }

        let up_size = upload::uploads_byte_size(uploads);
        if up_size > fb.staging.size {
            unsafe { upload::destroy_buffer(device, &fb.staging) };
            fb.staging = upload::create_host_buffer(
                device,
                &self.mem_props,
                up_size.next_power_of_two(),
                vk::BufferUsageFlags::TRANSFER_SRC,
            )?;
        }
        upload::record_uploads(device, cmd, &fb.staging, uploads);
        Ok(offsets)
    }

    /// Phase 2, inside a render pass: bind and draw all ranges of one list,
    /// whose vertices start at `base` in this frame's vertex buffer.
    pub fn record_pass(
        &self,
        device: &ash::Device,
        cmd: vk::CommandBuffer,
        extent: vk::Extent2D,
        frame_idx: usize,
        list: &DrawList,
        base: u32,
        sets: &TexSets,
    ) {
        if list.is_empty() {
            return;
        }
        unsafe {
            device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.pipeline);
            device.cmd_set_viewport(
                cmd,
                0,
                &[vk::Viewport {
                    x: 0.0,
                    y: 0.0,
                    width: extent.width as f32,
                    height: extent.height as f32,
                    min_depth: 0.0,
                    max_depth: 1.0,
                }],
            );
            device.cmd_set_scissor(
                cmd,
                0,
                &[vk::Rect2D { offset: vk::Offset2D { x: 0, y: 0 }, extent }],
            );
            device.cmd_bind_vertex_buffers(cmd, 0, &[self.frames[frame_idx].vertex.buf], &[0]);
            let viewport = [extent.width as f32, extent.height as f32];
            device.cmd_push_constants(
                cmd,
                self.pipeline_layout,
                vk::ShaderStageFlags::VERTEX,
                0,
                bytemuck::cast_slice(&viewport),
            );
            for range in &list.ranges {
                if range.count == 0 {
                    continue;
                }
                device.cmd_bind_descriptor_sets(
                    cmd,
                    vk::PipelineBindPoint::GRAPHICS,
                    self.pipeline_layout,
                    0,
                    &[sets.get(range.tex)],
                    &[],
                );
                device.cmd_draw(cmd, range.count, 1, base + range.first, 0);
            }
        }
    }

    pub fn destroy(&mut self, device: &ash::Device) {
        unsafe {
            for fb in &self.frames {
                upload::destroy_buffer(device, &fb.vertex);
                upload::destroy_buffer(device, &fb.staging);
            }
            self.frames.clear();
            device.destroy_pipeline(self.pipeline, None);
            device.destroy_pipeline_layout(self.pipeline_layout, None);
            device.destroy_sampler(self.sampler, None);
            device.destroy_descriptor_pool(self.desc_pool, None);
            device.destroy_descriptor_set_layout(self.desc_layout, None);
        }
    }
}
