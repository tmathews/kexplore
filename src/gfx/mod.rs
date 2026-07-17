//! Vulkan context: instance, device, queue, per-frame resources and the
//! frame render loop. Requires Vulkan 1.3 (dynamicRendering + sync2).

pub mod blur;
pub mod renderer2d;
pub mod swapchain;
pub mod upload;

use ash::vk;
use std::ffi::{c_void, CStr};

use swapchain::Swapchain;

pub const FRAMES_IN_FLIGHT: usize = 2;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Before any pass: vertex/texture uploads.
    Upload,
    /// Inside the offscreen canvas pass (cleared to the background color).
    Scene,
    /// Inside the swapchain pass (scene composite + toolbar draw here).
    Overlay,
}

pub struct FrameRes {
    pub pool: vk::CommandPool,
    pub cmd: vk::CommandBuffer,
    pub image_available: vk::Semaphore,
    pub fence: vk::Fence,
}

pub struct Gfx {
    pub _entry: ash::Entry,
    pub instance: ash::Instance,
    debug: Option<(ash::ext::debug_utils::Instance, vk::DebugUtilsMessengerEXT)>,
    pub surface_loader: ash::khr::surface::Instance,
    pub surface: vk::SurfaceKHR,
    pub phys: vk::PhysicalDevice,
    pub device: ash::Device,
    pub queue: vk::Queue,
    pub swapchain_loader: ash::khr::swapchain::Device,
    pub swapchain: Swapchain,
    pub mem_props: vk::PhysicalDeviceMemoryProperties,
    pub blur: blur::Blur,
    frames: Vec<FrameRes>,
    /// One present-wait semaphore per swapchain image (a per-frame semaphore
    /// could still be pending when reused).
    present_sems: Vec<vk::Semaphore>,
    frame_idx: usize,
}

unsafe extern "system" fn debug_callback(
    severity: vk::DebugUtilsMessageSeverityFlagsEXT,
    _types: vk::DebugUtilsMessageTypeFlagsEXT,
    data: *const vk::DebugUtilsMessengerCallbackDataEXT<'_>,
    _user: *mut c_void,
) -> vk::Bool32 {
    let msg = CStr::from_ptr((*data).p_message).to_string_lossy();
    eprintln!("[vulkan {severity:?}] {msg}");
    vk::FALSE
}

impl Gfx {
    pub fn new(
        display_ptr: *mut c_void,
        surface_ptr: *mut c_void,
        extent: (u32, u32),
        band_h: u32,
    ) -> Result<Gfx, String> {
        unsafe { Self::new_inner(display_ptr, surface_ptr, extent, band_h) }
    }

    unsafe fn new_inner(
        display_ptr: *mut c_void,
        surface_ptr: *mut c_void,
        extent: (u32, u32),
        band_h: u32,
    ) -> Result<Gfx, String> {
        let entry = ash::Entry::load().map_err(|e| format!("load libvulkan: {e}"))?;

        // Instance
        let app_info = vk::ApplicationInfo::default()
            .application_name(c"kexplore")
            .api_version(vk::API_VERSION_1_3);
        let mut exts = vec![ash::khr::surface::NAME.as_ptr(), ash::khr::wayland_surface::NAME.as_ptr()];
        let mut layers = Vec::new();
        let mut want_debug = false;
        if cfg!(debug_assertions) {
            let has_validation = entry
                .enumerate_instance_layer_properties()
                .map(|ls| {
                    ls.iter().any(|l| {
                        l.layer_name_as_c_str() == Ok(c"VK_LAYER_KHRONOS_validation")
                    })
                })
                .unwrap_or(false);
            let has_debug_ext = entry
                .enumerate_instance_extension_properties(None)
                .map(|es| {
                    es.iter()
                        .any(|e| e.extension_name_as_c_str() == Ok(ash::ext::debug_utils::NAME))
                })
                .unwrap_or(false);
            if has_validation && has_debug_ext {
                layers.push(c"VK_LAYER_KHRONOS_validation".as_ptr());
                exts.push(ash::ext::debug_utils::NAME.as_ptr());
                want_debug = true;
            }
        }
        let instance_info = vk::InstanceCreateInfo::default()
            .application_info(&app_info)
            .enabled_extension_names(&exts)
            .enabled_layer_names(&layers);
        let instance = entry
            .create_instance(&instance_info, None)
            .map_err(|e| format!("create instance: {e}"))?;

        let debug = if want_debug {
            let loader = ash::ext::debug_utils::Instance::new(&entry, &instance);
            let info = vk::DebugUtilsMessengerCreateInfoEXT::default()
                .message_severity(
                    vk::DebugUtilsMessageSeverityFlagsEXT::WARNING
                        | vk::DebugUtilsMessageSeverityFlagsEXT::ERROR,
                )
                .message_type(
                    vk::DebugUtilsMessageTypeFlagsEXT::VALIDATION
                        | vk::DebugUtilsMessageTypeFlagsEXT::PERFORMANCE
                        | vk::DebugUtilsMessageTypeFlagsEXT::GENERAL,
                )
                .pfn_user_callback(Some(debug_callback));
            let messenger = loader
                .create_debug_utils_messenger(&info, None)
                .map_err(|e| format!("debug messenger: {e}"))?;
            Some((loader, messenger))
        } else {
            None
        };

        // Wayland surface
        let wayland_loader = ash::khr::wayland_surface::Instance::new(&entry, &instance);
        let surface_info = vk::WaylandSurfaceCreateInfoKHR::default()
            .display(display_ptr)
            .surface(surface_ptr);
        let surface = wayland_loader
            .create_wayland_surface(&surface_info, None)
            .map_err(|e| format!("create wayland surface: {e}"))?;
        let surface_loader = ash::khr::surface::Instance::new(&entry, &instance);

        // Physical device + queue family: graphics with present support.
        let devices = instance
            .enumerate_physical_devices()
            .map_err(|e| format!("enumerate devices: {e}"))?;
        let mut picked = None;
        for phys in devices {
            let props = instance.get_physical_device_properties(phys);
            if props.api_version < vk::API_VERSION_1_3 {
                continue;
            }
            let qfams = instance.get_physical_device_queue_family_properties(phys);
            for (i, fam) in qfams.iter().enumerate() {
                let graphics = fam.queue_flags.contains(vk::QueueFlags::GRAPHICS);
                let present = surface_loader
                    .get_physical_device_surface_support(phys, i as u32, surface)
                    .unwrap_or(false);
                if graphics && present {
                    let discrete =
                        props.device_type == vk::PhysicalDeviceType::DISCRETE_GPU;
                    if picked.is_none() || discrete {
                        picked = Some((phys, i as u32, discrete));
                    }
                    break;
                }
            }
            if matches!(picked, Some((_, _, true))) {
                break;
            }
        }
        let (phys, queue_family, _) =
            picked.ok_or("no Vulkan 1.3 device with graphics+present support")?;

        // Logical device with dynamic rendering + synchronization2.
        let mut f13 = vk::PhysicalDeviceVulkan13Features::default()
            .dynamic_rendering(true)
            .synchronization2(true);
        let queue_prios = [1.0f32];
        let queue_infos = [vk::DeviceQueueCreateInfo::default()
            .queue_family_index(queue_family)
            .queue_priorities(&queue_prios)];
        let dev_exts = [ash::khr::swapchain::NAME.as_ptr()];
        let device_info = vk::DeviceCreateInfo::default()
            .queue_create_infos(&queue_infos)
            .enabled_extension_names(&dev_exts)
            .push_next(&mut f13);
        let device = instance
            .create_device(phys, &device_info, None)
            .map_err(|e| format!("create device: {e}"))?;
        let queue = device.get_device_queue(queue_family, 0);
        let mem_props = instance.get_physical_device_memory_properties(phys);

        let swapchain_loader = ash::khr::swapchain::Device::new(&instance, &device);
        let swapchain = Swapchain::new(
            &device,
            &swapchain_loader,
            &surface_loader,
            phys,
            surface,
            extent,
            vk::SwapchainKHR::null(),
        )?;

        let mut frames = Vec::new();
        for _ in 0..FRAMES_IN_FLIGHT {
            let pool = device
                .create_command_pool(
                    &vk::CommandPoolCreateInfo::default().queue_family_index(queue_family),
                    None,
                )
                .map_err(|e| format!("command pool: {e}"))?;
            let cmd = device
                .allocate_command_buffers(
                    &vk::CommandBufferAllocateInfo::default()
                        .command_pool(pool)
                        .level(vk::CommandBufferLevel::PRIMARY)
                        .command_buffer_count(1),
                )
                .map_err(|e| format!("command buffer: {e}"))?[0];
            let image_available = device
                .create_semaphore(&vk::SemaphoreCreateInfo::default(), None)
                .map_err(|e| format!("semaphore: {e}"))?;
            let fence = device
                .create_fence(
                    &vk::FenceCreateInfo::default().flags(vk::FenceCreateFlags::SIGNALED),
                    None,
                )
                .map_err(|e| format!("fence: {e}"))?;
            frames.push(FrameRes { pool, cmd, image_available, fence });
        }
        let present_sems = Self::make_present_sems(&device, swapchain.images.len())?;
        let blur =
            blur::Blur::new(&device, &mem_props, swapchain.format, swapchain.extent, band_h)?;

        Ok(Gfx {
            _entry: entry,
            instance,
            debug,
            surface_loader,
            surface,
            phys,
            device,
            queue,
            swapchain_loader,
            swapchain,
            mem_props,
            blur,
            frames,
            present_sems,
            frame_idx: 0,
        })
    }

    fn make_present_sems(
        device: &ash::Device,
        count: usize,
    ) -> Result<Vec<vk::Semaphore>, String> {
        (0..count)
            .map(|_| unsafe {
                device
                    .create_semaphore(&vk::SemaphoreCreateInfo::default(), None)
                    .map_err(|e| format!("semaphore: {e}"))
            })
            .collect()
    }

    pub fn recreate_swapchain(&mut self, extent: (u32, u32), band_h: u32) -> Result<(), String> {
        unsafe {
            self.device.device_wait_idle().ok();
        }
        let old = self.swapchain.handle;
        let new = Swapchain::new(
            &self.device,
            &self.swapchain_loader,
            &self.surface_loader,
            self.phys,
            self.surface,
            extent,
            old,
        )?;
        let mut old_sc = std::mem::replace(&mut self.swapchain, new);
        unsafe {
            old_sc.destroy(&self.device, &self.swapchain_loader);
            for s in self.present_sems.drain(..) {
                self.device.destroy_semaphore(s, None);
            }
        }
        self.present_sems = Self::make_present_sems(&self.device, self.swapchain.images.len())?;
        self.blur.recreate(&self.device, &self.mem_props, self.swapchain.extent, band_h)?;
        Ok(())
    }

    /// Render one frame. `record` is called twice: with Phase::Upload before
    /// the render pass (vertex/texture uploads; runs after this frame's
    /// fence wait so per-frame buffers are safe to write), then with
    /// Phase::Pass inside an active dynamic-rendering pass cleared to the
    /// background color. Returns Ok(false) if the swapchain is out of date
    /// and must be recreated.
    pub fn render_frame<F>(&mut self, record: F) -> Result<bool, String>
    where
        F: FnMut(Phase, &ash::Device, vk::CommandBuffer, vk::Extent2D, usize) -> Result<(), String>,
    {
        unsafe { self.render_frame_inner(record) }
    }

    unsafe fn render_frame_inner<F>(&mut self, mut record: F) -> Result<bool, String>
    where
        F: FnMut(Phase, &ash::Device, vk::CommandBuffer, vk::Extent2D, usize) -> Result<(), String>,
    {
        let frame = &self.frames[self.frame_idx];
        let device = &self.device;

        device
            .wait_for_fences(&[frame.fence], true, u64::MAX)
            .map_err(|e| format!("wait fence: {e}"))?;

        let acquired = self.swapchain_loader.acquire_next_image(
            self.swapchain.handle,
            u64::MAX,
            frame.image_available,
            vk::Fence::null(),
        );
        let (image_index, _suboptimal) = match acquired {
            Ok(v) => v,
            Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => return Ok(false),
            Err(e) => return Err(format!("acquire: {e}")),
        };
        device.reset_fences(&[frame.fence]).map_err(|e| format!("reset fence: {e}"))?;

        device
            .reset_command_pool(frame.pool, vk::CommandPoolResetFlags::empty())
            .map_err(|e| format!("reset pool: {e}"))?;
        device
            .begin_command_buffer(
                frame.cmd,
                &vk::CommandBufferBeginInfo::default()
                    .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            )
            .map_err(|e| format!("begin cmd: {e}"))?;

        let image = self.swapchain.images[image_index as usize];
        let view = self.swapchain.views[image_index as usize];
        let extent = self.swapchain.extent;

        let to_attachment = vk::ImageMemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::TOP_OF_PIPE)
            .dst_stage_mask(vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT)
            .dst_access_mask(vk::AccessFlags2::COLOR_ATTACHMENT_WRITE)
            .old_layout(vk::ImageLayout::UNDEFINED)
            .new_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
            .image(image)
            .subresource_range(
                vk::ImageSubresourceRange::default()
                    .aspect_mask(vk::ImageAspectFlags::COLOR)
                    .level_count(1)
                    .layer_count(1),
            );
        record(Phase::Upload, device, frame.cmd, extent, self.frame_idx)?;

        // --- Scene pass: canvas into the offscreen texture ---
        let scene_image = self.blur.scene.image;
        let scene_range = vk::ImageSubresourceRange::default()
            .aspect_mask(vk::ImageAspectFlags::COLOR)
            .level_count(1)
            .layer_count(1);
        let scene_to_attachment = vk::ImageMemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::FRAGMENT_SHADER)
            .dst_stage_mask(vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT)
            .dst_access_mask(vk::AccessFlags2::COLOR_ATTACHMENT_WRITE)
            .old_layout(vk::ImageLayout::UNDEFINED) // fully redrawn each frame
            .new_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
            .image(scene_image)
            .subresource_range(scene_range);
        device.cmd_pipeline_barrier2(
            frame.cmd,
            &vk::DependencyInfo::default()
                .image_memory_barriers(std::slice::from_ref(&scene_to_attachment)),
        );
        // Premultiplied clear: rgb is already multiplied by a (rgb = 0).
        let clear = vk::ClearValue {
            color: vk::ClearColorValue { float32: [0.0, 0.0, 0.0, 0.8] },
        };
        let scene_attachment = vk::RenderingAttachmentInfo::default()
            .image_view(self.blur.scene.view)
            .image_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
            .load_op(vk::AttachmentLoadOp::CLEAR)
            .store_op(vk::AttachmentStoreOp::STORE)
            .clear_value(clear);
        let scene_rendering = vk::RenderingInfo::default()
            .render_area(vk::Rect2D { offset: vk::Offset2D { x: 0, y: 0 }, extent })
            .layer_count(1)
            .color_attachments(std::slice::from_ref(&scene_attachment));
        device.cmd_begin_rendering(frame.cmd, &scene_rendering);
        record(Phase::Scene, device, frame.cmd, extent, self.frame_idx)?;
        device.cmd_end_rendering(frame.cmd);
        let scene_to_read = vk::ImageMemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT)
            .src_access_mask(vk::AccessFlags2::COLOR_ATTACHMENT_WRITE)
            .dst_stage_mask(vk::PipelineStageFlags2::FRAGMENT_SHADER)
            .dst_access_mask(vk::AccessFlags2::SHADER_SAMPLED_READ)
            .old_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
            .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
            .image(scene_image)
            .subresource_range(scene_range);
        device.cmd_pipeline_barrier2(
            frame.cmd,
            &vk::DependencyInfo::default()
                .image_memory_barriers(std::slice::from_ref(&scene_to_read)),
        );

        // --- Blur passes: toolbar band, H then V at half res ---
        self.blur.record(device, frame.cmd);

        // --- Overlay pass: composite + toolbar into the swapchain image ---
        device.cmd_pipeline_barrier2(
            frame.cmd,
            &vk::DependencyInfo::default().image_memory_barriers(std::slice::from_ref(&to_attachment)),
        );
        // Transparent clear; the scene composite quad supplies every pixel.
        let clear = vk::ClearValue {
            color: vk::ClearColorValue { float32: [0.0, 0.0, 0.0, 0.0] },
        };
        let color_attachment = vk::RenderingAttachmentInfo::default()
            .image_view(view)
            .image_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
            .load_op(vk::AttachmentLoadOp::CLEAR)
            .store_op(vk::AttachmentStoreOp::STORE)
            .clear_value(clear);
        let rendering_info = vk::RenderingInfo::default()
            .render_area(vk::Rect2D { offset: vk::Offset2D { x: 0, y: 0 }, extent })
            .layer_count(1)
            .color_attachments(std::slice::from_ref(&color_attachment));
        device.cmd_begin_rendering(frame.cmd, &rendering_info);

        record(Phase::Overlay, device, frame.cmd, extent, self.frame_idx)?;

        device.cmd_end_rendering(frame.cmd);

        let to_present = vk::ImageMemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT)
            .src_access_mask(vk::AccessFlags2::COLOR_ATTACHMENT_WRITE)
            .dst_stage_mask(vk::PipelineStageFlags2::BOTTOM_OF_PIPE)
            .old_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
            .new_layout(vk::ImageLayout::PRESENT_SRC_KHR)
            .image(image)
            .subresource_range(
                vk::ImageSubresourceRange::default()
                    .aspect_mask(vk::ImageAspectFlags::COLOR)
                    .level_count(1)
                    .layer_count(1),
            );
        device.cmd_pipeline_barrier2(
            frame.cmd,
            &vk::DependencyInfo::default().image_memory_barriers(std::slice::from_ref(&to_present)),
        );

        device.end_command_buffer(frame.cmd).map_err(|e| format!("end cmd: {e}"))?;

        let present_sem = self.present_sems[image_index as usize];
        let wait_info = vk::SemaphoreSubmitInfo::default()
            .semaphore(frame.image_available)
            .stage_mask(vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT);
        let signal_info = vk::SemaphoreSubmitInfo::default()
            .semaphore(present_sem)
            .stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS);
        let cmd_info = vk::CommandBufferSubmitInfo::default().command_buffer(frame.cmd);
        let submit = vk::SubmitInfo2::default()
            .wait_semaphore_infos(std::slice::from_ref(&wait_info))
            .command_buffer_infos(std::slice::from_ref(&cmd_info))
            .signal_semaphore_infos(std::slice::from_ref(&signal_info));
        device
            .queue_submit2(self.queue, &[submit], frame.fence)
            .map_err(|e| format!("submit: {e}"))?;

        let swapchains = [self.swapchain.handle];
        let indices = [image_index];
        let wait_sems = [present_sem];
        let present_info = vk::PresentInfoKHR::default()
            .wait_semaphores(&wait_sems)
            .swapchains(&swapchains)
            .image_indices(&indices);
        let present = self.swapchain_loader.queue_present(self.queue, &present_info);
        self.frame_idx = (self.frame_idx + 1) % FRAMES_IN_FLIGHT;
        match present {
            Ok(false) => Ok(true),
            Ok(true) | Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => Ok(false),
            Err(e) => Err(format!("present: {e}")),
        }
    }
}

impl Drop for Gfx {
    fn drop(&mut self) {
        unsafe {
            self.device.device_wait_idle().ok();
            self.blur.destroy(&self.device);
            for f in &self.frames {
                self.device.destroy_semaphore(f.image_available, None);
                self.device.destroy_fence(f.fence, None);
                self.device.destroy_command_pool(f.pool, None);
            }
            for s in self.present_sems.drain(..) {
                self.device.destroy_semaphore(s, None);
            }
            self.swapchain.destroy(&self.device, &self.swapchain_loader);
            self.device.destroy_device(None);
            self.surface_loader.destroy_surface(self.surface, None);
            if let Some((loader, messenger)) = self.debug.take() {
                loader.destroy_debug_utils_messenger(messenger, None);
            }
            self.instance.destroy_instance(None);
        }
    }
}
