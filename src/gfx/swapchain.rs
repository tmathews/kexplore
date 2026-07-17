use ash::vk;

pub struct Swapchain {
    pub handle: vk::SwapchainKHR,
    pub images: Vec<vk::Image>,
    pub views: Vec<vk::ImageView>,
    pub format: vk::Format,
    pub extent: vk::Extent2D,
}

impl Swapchain {
    pub fn new(
        device: &ash::Device,
        loader: &ash::khr::swapchain::Device,
        surface_loader: &ash::khr::surface::Instance,
        phys: vk::PhysicalDevice,
        surface: vk::SurfaceKHR,
        requested: (u32, u32),
        old: vk::SwapchainKHR,
    ) -> Result<Swapchain, String> {
        unsafe { Self::new_inner(device, loader, surface_loader, phys, surface, requested, old) }
    }

    unsafe fn new_inner(
        device: &ash::Device,
        loader: &ash::khr::swapchain::Device,
        surface_loader: &ash::khr::surface::Instance,
        phys: vk::PhysicalDevice,
        surface: vk::SurfaceKHR,
        requested: (u32, u32),
        old: vk::SwapchainKHR,
    ) -> Result<Swapchain, String> {
        let caps = surface_loader
            .get_physical_device_surface_capabilities(phys, surface)
            .map_err(|e| format!("surface caps: {e}"))?;
        let formats = surface_loader
            .get_physical_device_surface_formats(phys, surface)
            .map_err(|e| format!("surface formats: {e}"))?;

        // UNORM so we write and blend in sRGB space like Cairo did — this
        // keeps colors and blended edges looking identical to the C app.
        let format = formats
            .iter()
            .find(|f| {
                f.format == vk::Format::B8G8R8A8_UNORM
                    && f.color_space == vk::ColorSpaceKHR::SRGB_NONLINEAR
            })
            .or_else(|| formats.iter().find(|f| f.format == vk::Format::R8G8B8A8_UNORM))
            .or(formats.first())
            .copied()
            .ok_or("no surface formats")?;

        let extent = if caps.current_extent.width != u32::MAX {
            caps.current_extent
        } else {
            vk::Extent2D {
                width: requested.0.clamp(
                    caps.min_image_extent.width.max(1),
                    caps.max_image_extent.width,
                ),
                height: requested.1.clamp(
                    caps.min_image_extent.height.max(1),
                    caps.max_image_extent.height,
                ),
            }
        };

        let mut image_count = caps.min_image_count + 1;
        if caps.max_image_count > 0 {
            image_count = image_count.min(caps.max_image_count);
        }

        // Preserve the C app's translucent background where the compositor
        // supports it.
        let composite_alpha = if caps
            .supported_composite_alpha
            .contains(vk::CompositeAlphaFlagsKHR::PRE_MULTIPLIED)
        {
            vk::CompositeAlphaFlagsKHR::PRE_MULTIPLIED
        } else if caps.supported_composite_alpha.contains(vk::CompositeAlphaFlagsKHR::OPAQUE) {
            vk::CompositeAlphaFlagsKHR::OPAQUE
        } else {
            vk::CompositeAlphaFlagsKHR::INHERIT
        };

        let info = vk::SwapchainCreateInfoKHR::default()
            .surface(surface)
            .min_image_count(image_count)
            .image_format(format.format)
            .image_color_space(format.color_space)
            .image_extent(extent)
            .image_array_layers(1)
            .image_usage(vk::ImageUsageFlags::COLOR_ATTACHMENT)
            .image_sharing_mode(vk::SharingMode::EXCLUSIVE)
            .pre_transform(vk::SurfaceTransformFlagsKHR::IDENTITY)
            .composite_alpha(composite_alpha)
            .present_mode(vk::PresentModeKHR::FIFO)
            .clipped(true)
            .old_swapchain(old);
        let handle = loader
            .create_swapchain(&info, None)
            .map_err(|e| format!("create swapchain: {e}"))?;

        let images = loader
            .get_swapchain_images(handle)
            .map_err(|e| format!("swapchain images: {e}"))?;
        let views = images
            .iter()
            .map(|&image| {
                let view_info = vk::ImageViewCreateInfo::default()
                    .image(image)
                    .view_type(vk::ImageViewType::TYPE_2D)
                    .format(format.format)
                    .subresource_range(
                        vk::ImageSubresourceRange::default()
                            .aspect_mask(vk::ImageAspectFlags::COLOR)
                            .level_count(1)
                            .layer_count(1),
                    );
                device
                    .create_image_view(&view_info, None)
                    .map_err(|e| format!("image view: {e}"))
            })
            .collect::<Result<Vec<_>, _>>()?;

        Ok(Swapchain { handle, images, views, format: format.format, extent })
    }

    pub unsafe fn destroy(&mut self, device: &ash::Device, loader: &ash::khr::swapchain::Device) {
        for view in self.views.drain(..) {
            device.destroy_image_view(view, None);
        }
        loader.destroy_swapchain(self.handle, None);
        self.handle = vk::SwapchainKHR::null();
    }
}
