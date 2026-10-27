//! Splitscreen (spike): several players' renderers draw offscreen on one device, and the
//! compositor copies each frame into its place on a window's surface. A window holds views but
//! never owns them, so a view can later move to another window (another monitor, detached).
use super::Renderer;
use crate::Result;
use std::sync::Arc;
use winit::window::Window;

pub struct Compositor {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    present_modes: Vec<wgpu::PresentMode>,
}

impl Renderer {
    /// A compositor for `window` on this renderer's device.
    pub fn compositor(&self, window: Arc<Window>) -> Result<Compositor> {
        let surface = self.instance.create_surface(window.clone())?;
        let caps = surface.get_capabilities(&self.adapter);
        // ponytail: a plain texture copy needs the offscreen frames' format; a blit pass would
        // take any surface format (Rgba8Unorm on some Linux drivers).
        let format = wgpu::TextureFormat::Bgra8Unorm;
        if !caps.formats.contains(&format) {
            return Err(format!(
                "splitscreen needs a {format:?} surface; this one offers {:?}",
                caps.formats
            )
            .into());
        }
        let size = window.inner_size();
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_DST,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            desired_maximum_frame_latency: 2,
            alpha_mode: wgpu::CompositeAlphaMode::Opaque,
            view_formats: vec![],
            color_space: wgpu::SurfaceColorSpace::default(),
        };
        surface.configure(&self.device, &config);
        Ok(Compositor {
            surface,
            device: self.device.clone(),
            queue: self.queue.clone(),
            config,
            present_modes: caps.present_modes,
        })
    }

    /// The last offscreen frame (`--offscreen`), for the compositor.
    pub fn offscreen_frame(&self) -> Option<&wgpu::Texture> {
        self.offscreen.as_ref()
    }
}

impl Compositor {
    /// Vsync on: presents wait for the display. Off: they don't (Mailbox, else Immediate).
    pub fn set_vsync(&mut self, vsync: bool) {
        let mode = [wgpu::PresentMode::Mailbox, wgpu::PresentMode::Immediate]
            .into_iter()
            .find(|m| !vsync && self.present_modes.contains(m))
            .unwrap_or(wgpu::PresentMode::Fifo);
        if mode != self.config.present_mode {
            self.config.present_mode = mode;
            self.surface.configure(&self.device, &self.config);
        }
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if width > 0 && height > 0 {
            self.config.width = width;
            self.config.height = height;
            self.surface.configure(&self.device, &self.config);
        }
    }

    /// Copy each view's frame to its top-left corner on the window and present. Frames that
    /// don't fit are clipped; uncovered parts of the window keep whatever was there. False when
    /// the window had no frame to give (occluded, resized, timed out).
    pub fn present(&mut self, views: &[(&wgpu::Texture, [u32; 2])]) -> bool {
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f)
            | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.device, &self.config);
                return false;
            }
            _ => return false,
        };
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Splitscreen compositor"),
            });
        for &(texture, [x, y]) in views {
            let width = texture.width().min(self.config.width.saturating_sub(x));
            let height = texture.height().min(self.config.height.saturating_sub(y));
            if width == 0 || height == 0 {
                continue;
            }
            encoder.copy_texture_to_texture(
                texture.as_image_copy(),
                wgpu::TexelCopyTextureInfo {
                    origin: wgpu::Origin3d { x, y, z: 0 },
                    ..frame.texture.as_image_copy()
                },
                wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
            );
        }
        self.queue.submit([encoder.finish()]);
        self.queue.present(frame);
        true
    }
}
