//! Buffer/texture creation and upload helpers. Allocation strategy is plain
//! vkAllocateMemory per resource — this app has a handful of allocations
//! total, far under maxMemoryAllocationCount.

use ash::vk;

pub struct Buffer {
    pub buf: vk::Buffer,
    pub mem: vk::DeviceMemory,
    pub size: u64,
    pub mapped: *mut u8,
}

pub struct Texture {
    pub image: vk::Image,
    pub mem: vk::DeviceMemory,
    pub view: vk::ImageView,
}

pub fn find_memory_type(
    props: &vk::PhysicalDeviceMemoryProperties,
    type_bits: u32,
    flags: vk::MemoryPropertyFlags,
) -> Result<u32, String> {
    for i in 0..props.memory_type_count {
        if type_bits & (1 << i) != 0
            && props.memory_types[i as usize].property_flags.contains(flags)
        {
            return Ok(i);
        }
    }
    Err("no suitable memory type".into())
}

/// Create a host-visible, coherent, persistently-mapped buffer.
pub fn create_host_buffer(
    device: &ash::Device,
    mem_props: &vk::PhysicalDeviceMemoryProperties,
    size: u64,
    usage: vk::BufferUsageFlags,
) -> Result<Buffer, String> {
    unsafe {
        let buf = device
            .create_buffer(
                &vk::BufferCreateInfo::default()
                    .size(size)
                    .usage(usage)
                    .sharing_mode(vk::SharingMode::EXCLUSIVE),
                None,
            )
            .map_err(|e| format!("create buffer: {e}"))?;
        let reqs = device.get_buffer_memory_requirements(buf);
        let mem_type = find_memory_type(
            mem_props,
            reqs.memory_type_bits,
            vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
        )?;
        let mem = device
            .allocate_memory(
                &vk::MemoryAllocateInfo::default()
                    .allocation_size(reqs.size)
                    .memory_type_index(mem_type),
                None,
            )
            .map_err(|e| format!("allocate memory: {e}"))?;
        device.bind_buffer_memory(buf, mem, 0).map_err(|e| format!("bind buffer: {e}"))?;
        let mapped = device
            .map_memory(mem, 0, vk::WHOLE_SIZE, vk::MemoryMapFlags::empty())
            .map_err(|e| format!("map memory: {e}"))? as *mut u8;
        Ok(Buffer { buf, mem, size, mapped })
    }
}

pub unsafe fn destroy_buffer(device: &ash::Device, b: &Buffer) {
    device.destroy_buffer(b.buf, None);
    device.free_memory(b.mem, None);
}

/// Create a device-local sampled texture (no contents yet; layout UNDEFINED).
pub fn create_texture(
    device: &ash::Device,
    mem_props: &vk::PhysicalDeviceMemoryProperties,
    width: u32,
    height: u32,
    format: vk::Format,
) -> Result<Texture, String> {
    unsafe {
        let image = device
            .create_image(
                &vk::ImageCreateInfo::default()
                    .image_type(vk::ImageType::TYPE_2D)
                    .format(format)
                    .extent(vk::Extent3D { width, height, depth: 1 })
                    .mip_levels(1)
                    .array_layers(1)
                    .samples(vk::SampleCountFlags::TYPE_1)
                    .tiling(vk::ImageTiling::OPTIMAL)
                    .usage(vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST)
                    .sharing_mode(vk::SharingMode::EXCLUSIVE)
                    .initial_layout(vk::ImageLayout::UNDEFINED),
                None,
            )
            .map_err(|e| format!("create image: {e}"))?;
        let reqs = device.get_image_memory_requirements(image);
        let mem_type = find_memory_type(
            mem_props,
            reqs.memory_type_bits,
            vk::MemoryPropertyFlags::DEVICE_LOCAL,
        )?;
        let mem = device
            .allocate_memory(
                &vk::MemoryAllocateInfo::default()
                    .allocation_size(reqs.size)
                    .memory_type_index(mem_type),
                None,
            )
            .map_err(|e| format!("allocate image memory: {e}"))?;
        device.bind_image_memory(image, mem, 0).map_err(|e| format!("bind image: {e}"))?;
        let view = device
            .create_image_view(
                &vk::ImageViewCreateInfo::default()
                    .image(image)
                    .view_type(vk::ImageViewType::TYPE_2D)
                    .format(format)
                    .subresource_range(
                        vk::ImageSubresourceRange::default()
                            .aspect_mask(vk::ImageAspectFlags::COLOR)
                            .level_count(1)
                            .layer_count(1),
                    ),
                None,
            )
            .map_err(|e| format!("image view: {e}"))?;
        Ok(Texture { image, mem, view })
    }
}

pub unsafe fn destroy_texture(device: &ash::Device, t: &Texture) {
    device.destroy_image_view(t.view, None);
    device.destroy_image(t.image, None);
    device.free_memory(t.mem, None);
}

/// A pending copy of pixel bytes into a region of a texture, executed at the
/// start of the next recorded frame.
pub struct PendingUpload {
    pub texture_image: vk::Image,
    pub bytes: Vec<u8>,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    /// Whether the image is in SHADER_READ_ONLY layout already (false for a
    /// freshly created texture in UNDEFINED layout).
    pub initialized: bool,
}

/// Record all pending uploads into `cmd` using `staging` (grown by caller if
/// needed). Returns bytes consumed. Barriers: -> TRANSFER_DST, copy,
/// -> SHADER_READ_ONLY.
pub fn record_uploads(
    device: &ash::Device,
    cmd: vk::CommandBuffer,
    staging: &Buffer,
    uploads: &[PendingUpload],
) {
    if uploads.is_empty() {
        return;
    }
    unsafe {
        let mut offset: u64 = 0;
        // Copy all pixel data into the staging buffer first.
        for up in uploads {
            std::ptr::copy_nonoverlapping(
                up.bytes.as_ptr(),
                staging.mapped.add(offset as usize),
                up.bytes.len(),
            );
            offset += up.bytes.len() as u64;
            offset = (offset + 3) & !3; // keep 4-byte alignment for copies
        }

        // Transition every target image to TRANSFER_DST.
        let barriers: Vec<vk::ImageMemoryBarrier2> = uploads
            .iter()
            .map(|up| {
                vk::ImageMemoryBarrier2::default()
                    .src_stage_mask(vk::PipelineStageFlags2::FRAGMENT_SHADER)
                    .src_access_mask(if up.initialized {
                        vk::AccessFlags2::SHADER_SAMPLED_READ
                    } else {
                        vk::AccessFlags2::empty()
                    })
                    .dst_stage_mask(vk::PipelineStageFlags2::TRANSFER)
                    .dst_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
                    .old_layout(if up.initialized {
                        vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL
                    } else {
                        vk::ImageLayout::UNDEFINED
                    })
                    .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                    .image(up.texture_image)
                    .subresource_range(
                        vk::ImageSubresourceRange::default()
                            .aspect_mask(vk::ImageAspectFlags::COLOR)
                            .level_count(1)
                            .layer_count(1),
                    )
            })
            .collect();
        device.cmd_pipeline_barrier2(
            cmd,
            &vk::DependencyInfo::default().image_memory_barriers(&barriers),
        );

        let mut offset: u64 = 0;
        for up in uploads {
            let region = vk::BufferImageCopy::default()
                .buffer_offset(offset)
                .image_subresource(
                    vk::ImageSubresourceLayers::default()
                        .aspect_mask(vk::ImageAspectFlags::COLOR)
                        .layer_count(1),
                )
                .image_offset(vk::Offset3D { x: up.x as i32, y: up.y as i32, z: 0 })
                .image_extent(vk::Extent3D { width: up.width, height: up.height, depth: 1 });
            device.cmd_copy_buffer_to_image(
                cmd,
                staging.buf,
                up.texture_image,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &[region],
            );
            offset += up.bytes.len() as u64;
            offset = (offset + 3) & !3;
        }

        // Transition back to SHADER_READ_ONLY for sampling.
        let barriers: Vec<vk::ImageMemoryBarrier2> = uploads
            .iter()
            .map(|up| {
                vk::ImageMemoryBarrier2::default()
                    .src_stage_mask(vk::PipelineStageFlags2::TRANSFER)
                    .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
                    .dst_stage_mask(vk::PipelineStageFlags2::FRAGMENT_SHADER)
                    .dst_access_mask(vk::AccessFlags2::SHADER_SAMPLED_READ)
                    .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                    .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                    .image(up.texture_image)
                    .subresource_range(
                        vk::ImageSubresourceRange::default()
                            .aspect_mask(vk::ImageAspectFlags::COLOR)
                            .level_count(1)
                            .layer_count(1),
                    )
            })
            .collect();
        device.cmd_pipeline_barrier2(
            cmd,
            &vk::DependencyInfo::default().image_memory_barriers(&barriers),
        );
    }
}

pub fn uploads_byte_size(uploads: &[super::upload::PendingUpload]) -> u64 {
    let mut total: u64 = 0;
    for up in uploads {
        total += up.bytes.len() as u64;
        total = (total + 3) & !3;
    }
    total
}
