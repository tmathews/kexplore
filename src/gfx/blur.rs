//! Frosted-glass backdrop for the toolbar: the canvas renders into an
//! offscreen `scene` texture; the toolbar band of it is blurred with a
//! separable gaussian at half resolution (H into `blur_a`, V into `blur_b`),
//! and the overlay pass composites scene + blurred band + toolbar.

use ash::vk;

use super::upload::{self, Texture};

pub struct Blur {
    desc_layout: vk::DescriptorSetLayout,
    desc_pool: vk::DescriptorPool,
    sampler: vk::Sampler,
    pipeline_layout: vk::PipelineLayout,
    pipeline: vk::Pipeline,
    /// Full-size offscreen target the canvas renders into.
    pub scene: Texture,
    blur_a: Texture,
    pub blur_b: Texture,
    /// blur-side descriptor sets: sample scene (pass 1), sample blur_a (pass 2)
    set_scene: vk::DescriptorSet,
    set_a: vk::DescriptorSet,
    scene_extent: vk::Extent2D,
    band_h: u32,
    blur_extent: vk::Extent2D,
    format: vk::Format,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct PushConstants {
    uv_offset: [f32; 2],
    uv_scale: [f32; 2],
    dir: [f32; 2],
}

/// Widen the gaussian a little beyond 1-texel spacing; slight quality
/// tradeoff for a stronger blur at half res.
const TAP_SPACING: f32 = 1.5;

fn attachment_extent(extent: vk::Extent2D, band_h: u32) -> (vk::Extent2D, vk::Extent2D) {
    let band_h = band_h.clamp(1, extent.height);
    let blur = vk::Extent2D { width: (extent.width / 2).max(1), height: (band_h / 2).max(1) };
    (vk::Extent2D { width: extent.width, height: band_h }, blur)
}

impl Blur {
    pub fn new(
        device: &ash::Device,
        mem_props: &vk::PhysicalDeviceMemoryProperties,
        format: vk::Format,
        extent: vk::Extent2D,
        band_h: u32,
    ) -> Result<Blur, String> {
        unsafe { Self::new_inner(device, mem_props, format, extent, band_h) }
    }

    unsafe fn new_inner(
        device: &ash::Device,
        mem_props: &vk::PhysicalDeviceMemoryProperties,
        format: vk::Format,
        extent: vk::Extent2D,
        band_h: u32,
    ) -> Result<Blur, String> {
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
            .map_err(|e| format!("blur desc layout: {e}"))?;
        let pool_sizes = [vk::DescriptorPoolSize::default()
            .ty(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .descriptor_count(2)];
        let desc_pool = device
            .create_descriptor_pool(
                &vk::DescriptorPoolCreateInfo::default().max_sets(2).pool_sizes(&pool_sizes),
                None,
            )
            .map_err(|e| format!("blur desc pool: {e}"))?;
        let sampler = device
            .create_sampler(
                &vk::SamplerCreateInfo::default()
                    .mag_filter(vk::Filter::LINEAR)
                    .min_filter(vk::Filter::LINEAR)
                    .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE),
                None,
            )
            .map_err(|e| format!("blur sampler: {e}"))?;

        let pc_range = vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::FRAGMENT)
            .size(std::mem::size_of::<PushConstants>() as u32);
        let set_layouts = [desc_layout];
        let pipeline_layout = device
            .create_pipeline_layout(
                &vk::PipelineLayoutCreateInfo::default()
                    .set_layouts(&set_layouts)
                    .push_constant_ranges(std::slice::from_ref(&pc_range)),
                None,
            )
            .map_err(|e| format!("blur pipeline layout: {e}"))?;

        let vert_spv = ash::util::read_spv(&mut std::io::Cursor::new(
            &include_bytes!("../../shaders/blur.vert.spv")[..],
        ))
        .map_err(|e| format!("blur vert spv: {e}"))?;
        let frag_spv = ash::util::read_spv(&mut std::io::Cursor::new(
            &include_bytes!("../../shaders/blur.frag.spv")[..],
        ))
        .map_err(|e| format!("blur frag spv: {e}"))?;
        let vert_mod = device
            .create_shader_module(&vk::ShaderModuleCreateInfo::default().code(&vert_spv), None)
            .map_err(|e| format!("blur vert module: {e}"))?;
        let frag_mod = device
            .create_shader_module(&vk::ShaderModuleCreateInfo::default().code(&frag_spv), None)
            .map_err(|e| format!("blur frag module: {e}"))?;
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
        let vertex_input = vk::PipelineVertexInputStateCreateInfo::default();
        let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
            .topology(vk::PrimitiveTopology::TRIANGLE_LIST);
        let viewport_state =
            vk::PipelineViewportStateCreateInfo::default().viewport_count(1).scissor_count(1);
        let raster = vk::PipelineRasterizationStateCreateInfo::default()
            .polygon_mode(vk::PolygonMode::FILL)
            .cull_mode(vk::CullModeFlags::NONE)
            .line_width(1.0);
        let multisample = vk::PipelineMultisampleStateCreateInfo::default()
            .rasterization_samples(vk::SampleCountFlags::TYPE_1);
        let blend_attachment = [vk::PipelineColorBlendAttachmentState::default()
            .blend_enable(false)
            .color_write_mask(vk::ColorComponentFlags::RGBA)];
        let blend = vk::PipelineColorBlendStateCreateInfo::default().attachments(&blend_attachment);
        let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
        let dynamic = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);
        let color_formats = [format];
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
            .map_err(|(_, e)| format!("blur pipeline: {e}"))?[0];
        device.destroy_shader_module(vert_mod, None);
        device.destroy_shader_module(frag_mod, None);

        let sets = device
            .allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::default()
                    .descriptor_pool(desc_pool)
                    .set_layouts(&[desc_layout, desc_layout]),
            )
            .map_err(|e| format!("blur desc sets: {e}"))?;

        let (_, blur_extent) = attachment_extent(extent, band_h);
        let scene = Self::make_target(device, mem_props, format, extent)?;
        let blur_a = Self::make_target(device, mem_props, format, blur_extent)?;
        let blur_b = Self::make_target(device, mem_props, format, blur_extent)?;

        let blur = Blur {
            desc_layout,
            desc_pool,
            sampler,
            pipeline_layout,
            pipeline,
            scene,
            blur_a,
            blur_b,
            set_scene: sets[0],
            set_a: sets[1],
            scene_extent: extent,
            band_h: band_h.clamp(1, extent.height),
            blur_extent,
            format,
        };
        blur.write_sets(device);
        Ok(blur)
    }

    fn make_target(
        device: &ash::Device,
        mem_props: &vk::PhysicalDeviceMemoryProperties,
        format: vk::Format,
        extent: vk::Extent2D,
    ) -> Result<Texture, String> {
        upload::create_texture_ex(
            device,
            mem_props,
            extent.width,
            extent.height,
            format,
            vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::COLOR_ATTACHMENT,
        )
    }

    fn write_sets(&self, device: &ash::Device) {
        let scene_info = [vk::DescriptorImageInfo::default()
            .sampler(self.sampler)
            .image_view(self.scene.view)
            .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
        let a_info = [vk::DescriptorImageInfo::default()
            .sampler(self.sampler)
            .image_view(self.blur_a.view)
            .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
        let writes = [
            vk::WriteDescriptorSet::default()
                .dst_set(self.set_scene)
                .dst_binding(0)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .image_info(&scene_info),
            vk::WriteDescriptorSet::default()
                .dst_set(self.set_a)
                .dst_binding(0)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .image_info(&a_info),
        ];
        unsafe { device.update_descriptor_sets(&writes, &[]) };
    }

    /// Recreate the render targets for a new size/scale. Caller must have
    /// waited for the device to be idle.
    pub fn recreate(
        &mut self,
        device: &ash::Device,
        mem_props: &vk::PhysicalDeviceMemoryProperties,
        extent: vk::Extent2D,
        band_h: u32,
    ) -> Result<(), String> {
        unsafe {
            upload::destroy_texture(device, &self.scene);
            upload::destroy_texture(device, &self.blur_a);
            upload::destroy_texture(device, &self.blur_b);
        }
        let (_, blur_extent) = attachment_extent(extent, band_h);
        self.scene = Self::make_target(device, mem_props, self.format, extent)?;
        self.blur_a = Self::make_target(device, mem_props, self.format, blur_extent)?;
        self.blur_b = Self::make_target(device, mem_props, self.format, blur_extent)?;
        self.scene_extent = extent;
        self.band_h = band_h.clamp(1, extent.height);
        self.blur_extent = blur_extent;
        self.write_sets(device);
        Ok(())
    }

    /// Record both blur passes. The scene texture must already be in
    /// SHADER_READ_ONLY layout.
    pub unsafe fn record(&self, device: &ash::Device, cmd: vk::CommandBuffer) {
        let band_frac = self.band_h as f32 / self.scene_extent.height as f32;
        // Pass 1: horizontal, scene band region -> blur_a (half res).
        self.pass(
            device,
            cmd,
            &self.blur_a,
            self.set_scene,
            PushConstants {
                uv_offset: [0.0, 0.0],
                uv_scale: [1.0, band_frac],
                dir: [TAP_SPACING / self.scene_extent.width as f32, 0.0],
            },
        );
        // Pass 2: vertical, blur_a -> blur_b.
        self.pass(
            device,
            cmd,
            &self.blur_b,
            self.set_a,
            PushConstants {
                uv_offset: [0.0, 0.0],
                uv_scale: [1.0, 1.0],
                dir: [0.0, TAP_SPACING / self.blur_extent.height as f32],
            },
        );
    }

    unsafe fn pass(
        &self,
        device: &ash::Device,
        cmd: vk::CommandBuffer,
        target: &Texture,
        src_set: vk::DescriptorSet,
        pc: PushConstants,
    ) {
        let to_attachment = vk::ImageMemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::FRAGMENT_SHADER)
            .dst_stage_mask(vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT)
            .dst_access_mask(vk::AccessFlags2::COLOR_ATTACHMENT_WRITE)
            .old_layout(vk::ImageLayout::UNDEFINED)
            .new_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
            .image(target.image)
            .subresource_range(
                vk::ImageSubresourceRange::default()
                    .aspect_mask(vk::ImageAspectFlags::COLOR)
                    .level_count(1)
                    .layer_count(1),
            );
        device.cmd_pipeline_barrier2(
            cmd,
            &vk::DependencyInfo::default()
                .image_memory_barriers(std::slice::from_ref(&to_attachment)),
        );

        let attachment = vk::RenderingAttachmentInfo::default()
            .image_view(target.view)
            .image_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
            .load_op(vk::AttachmentLoadOp::DONT_CARE)
            .store_op(vk::AttachmentStoreOp::STORE);
        let info = vk::RenderingInfo::default()
            .render_area(vk::Rect2D { offset: vk::Offset2D { x: 0, y: 0 }, extent: self.blur_extent })
            .layer_count(1)
            .color_attachments(std::slice::from_ref(&attachment));
        device.cmd_begin_rendering(cmd, &info);
        device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.pipeline);
        device.cmd_set_viewport(
            cmd,
            0,
            &[vk::Viewport {
                x: 0.0,
                y: 0.0,
                width: self.blur_extent.width as f32,
                height: self.blur_extent.height as f32,
                min_depth: 0.0,
                max_depth: 1.0,
            }],
        );
        device.cmd_set_scissor(
            cmd,
            0,
            &[vk::Rect2D { offset: vk::Offset2D { x: 0, y: 0 }, extent: self.blur_extent }],
        );
        device.cmd_bind_descriptor_sets(
            cmd,
            vk::PipelineBindPoint::GRAPHICS,
            self.pipeline_layout,
            0,
            &[src_set],
            &[],
        );
        device.cmd_push_constants(
            cmd,
            self.pipeline_layout,
            vk::ShaderStageFlags::FRAGMENT,
            0,
            bytemuck::bytes_of(&pc),
        );
        device.cmd_draw(cmd, 3, 1, 0, 0);
        device.cmd_end_rendering(cmd);

        let to_read = vk::ImageMemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT)
            .src_access_mask(vk::AccessFlags2::COLOR_ATTACHMENT_WRITE)
            .dst_stage_mask(vk::PipelineStageFlags2::FRAGMENT_SHADER)
            .dst_access_mask(vk::AccessFlags2::SHADER_SAMPLED_READ)
            .old_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
            .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
            .image(target.image)
            .subresource_range(
                vk::ImageSubresourceRange::default()
                    .aspect_mask(vk::ImageAspectFlags::COLOR)
                    .level_count(1)
                    .layer_count(1),
            );
        device.cmd_pipeline_barrier2(
            cmd,
            &vk::DependencyInfo::default().image_memory_barriers(std::slice::from_ref(&to_read)),
        );
    }

    pub fn destroy(&mut self, device: &ash::Device) {
        unsafe {
            upload::destroy_texture(device, &self.scene);
            upload::destroy_texture(device, &self.blur_a);
            upload::destroy_texture(device, &self.blur_b);
            device.destroy_pipeline(self.pipeline, None);
            device.destroy_pipeline_layout(self.pipeline_layout, None);
            device.destroy_sampler(self.sampler, None);
            device.destroy_descriptor_pool(self.desc_pool, None);
            device.destroy_descriptor_set_layout(self.desc_layout, None);
        }
    }
}
