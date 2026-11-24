//! Splitscreen: several players' renderers draw offscreen on one device, and the
//! compositor puts each frame in its place on a window's surface: a texture copy where the
//! surface takes one, a draw elsewhere (`Draw`). A window holds views but never owns them, so
//! a view can later move to another window (another monitor, detached).
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
    suspended: bool,
    /// Set where the surface takes no copies of the frames: they are drawn on it.
    draw: Option<Draw>,
}

/// The offscreen frames' format (`Renderer::new`).
const FRAME: wgpu::TextureFormat = wgpu::TextureFormat::Bgra8Unorm;

impl Renderer {
    /// A compositor for `window` on this renderer's device, presenting on the surface it gave up.
    pub fn compositor(&self, window: Arc<Window>) -> Result<Compositor> {
        // The surface this renderer set aside for its successor (`release_surface`), else a new
        // one: a window can't keep two.
        let spare = self
            .spare_surface
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        let surface = match spare {
            Some(surface) => surface,
            None => self.instance.create_surface(window.clone())?,
        };
        Compositor::new(
            surface,
            &self.adapter,
            &self.device,
            &self.queue,
            window.inner_size().into(),
        )
    }

    /// The last offscreen frame (`--offscreen`), for the compositor.
    pub fn offscreen_frame(&self) -> Option<&wgpu::Texture> {
        self.offscreen.as_ref()
    }

    /// Splitscreen turned off: a compositor's surface, set aside for the renderer that draws on
    /// the window again (it takes it, as after `release_surface`).
    pub fn take_back(&self, compositor: Compositor) {
        *self
            .spare_surface
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(compositor.surface);
    }
}

impl Compositor {
    /// Present offscreen BGRA frames on a surface using their existing device. They are
    /// copied where the surface takes copies of their format, and drawn elsewhere, which
    /// needs frames with `TEXTURE_BINDING`: OpenGL's surfaces take no copies (#17756), and
    /// some Linux drivers offer Rgba8Unorm alone.
    pub fn new(
        surface: wgpu::Surface<'static>,
        adapter: &wgpu::Adapter,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        size: [u32; 2],
    ) -> Result<Self> {
        let caps = surface.get_capabilities(adapter);
        let copies =
            caps.usages.contains(wgpu::TextureUsages::COPY_DST) && caps.formats.contains(&FRAME);
        // The draw writes the frames' values as they are, so it takes 8-bit formats without sRGB
        // encoding: an sRGB one would encode them again, and wgpu shows a float one as linear light.
        let format = match [FRAME, wgpu::TextureFormat::Rgba8Unorm]
            .into_iter()
            .find(|format| caps.formats.contains(format))
        {
            Some(format) => format,
            None if caps.formats.is_empty() => {
                return Err("this adapter can't present on the window".into());
            }
            None => {
                return Err(format!(
                    "splitscreen needs a {FRAME:?} or Rgba8Unorm surface; this one offers {:?}",
                    caps.formats
                )
                .into());
            }
        };
        let mut usage = wgpu::TextureUsages::RENDER_ATTACHMENT;
        if copies {
            usage |= wgpu::TextureUsages::COPY_DST;
        } else {
            log::info!(
                "Horizon: splitscreen draws the views on this window ({format:?}): its surface takes no copies of {FRAME:?} frames"
            );
        }
        let max = device.limits().max_texture_dimension_2d;
        let config = wgpu::SurfaceConfiguration {
            usage,
            format,
            width: size[0].clamp(1, max),
            height: size[1].clamp(1, max),
            present_mode: wgpu::PresentMode::Fifo,
            desired_maximum_frame_latency: 2,
            alpha_mode: wgpu::CompositeAlphaMode::Opaque,
            view_formats: vec![],
            color_space: wgpu::SurfaceColorSpace::default(),
        };
        surface.configure(device, &config);
        Ok(Compositor {
            surface,
            device: device.clone(),
            queue: queue.clone(),
            config,
            present_modes: caps.present_modes,
            suspended: !super::drawable_size(size[0], size[1], max),
            draw: (!copies).then(|| Draw::new(device, format)),
        })
    }

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
        self.suspended =
            !super::drawable_size(width, height, self.device.limits().max_texture_dimension_2d);
        if !self.suspended {
            self.config.width = width;
            self.config.height = height;
            self.surface.configure(&self.device, &self.config);
        }
    }

    /// Put each view's frame at its top-left corner on the window and present. Frames that
    /// don't fit are clipped; parts of the window no view covers (a view before its first frame,
    /// one still at its old size after a resize) are black. Nothing when the window has no frame
    /// to give (occluded, resized, timed out). Returns true when a frame was submitted.
    pub fn present(&mut self, views: &[(&wgpu::Texture, [u32; 2])]) -> bool {
        if self.suspended {
            return false;
        }
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
        let target = frame.texture.create_view(&Default::default());
        let size = [self.config.width, self.config.height];
        match &mut self.draw {
            Some(draw) => draw.views(&self.device, &mut encoder, &target, size, views),
            None => {
                drop(clear(&mut encoder, &target));
                for &(texture, [x, y]) in views {
                    let [width, height] =
                        clipped_size([texture.width(), texture.height()], [x, y], size);
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
            }
        }
        self.queue.submit([encoder.finish()]);
        self.queue.present(frame);
        true
    }
}

/// A pass on `target` that starts black: what no view covers stays so.
fn clear<'a>(
    encoder: &'a mut wgpu::CommandEncoder,
    target: &wgpu::TextureView,
) -> wgpu::RenderPass<'a> {
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("Splitscreen compositor"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: target,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                store: wgpu::StoreOp::Store,
            },
        })],
        ..Default::default()
    })
}

/// The frames drawn on a surface that takes no copies of them: each texel on its pixel, as
/// the copy leaves it (`compositor.wgsl`).
struct Draw {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    /// The frames of the last present, bound for the pass: a view's stays until it resizes.
    frames: Vec<(wgpu::Texture, wgpu::BindGroup)>,
}

impl Draw {
    /// For a target of `format`.
    fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Splitscreen compositor frame layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            }],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Splitscreen compositor shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("compositor.wgsl").into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Splitscreen compositor layout"),
            bind_group_layouts: &[Some(&layout)],
            ..Default::default()
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Splitscreen compositor"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vertex"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fragment"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        Draw {
            pipeline,
            layout,
            frames: Vec::new(),
        }
    }

    /// Clears `target`, of `size`, and draws each view's frame at its place. A frame that
    /// can't be bound (no `TEXTURE_BINDING`) is left out.
    fn views(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        size: [u32; 2],
        views: &[(&wgpu::Texture, [u32; 2])],
    ) {
        self.frames
            .retain(|(frame, _)| views.iter().any(|&(view, _)| view == frame));
        for &(texture, _) in views {
            if self.frames.iter().any(|(frame, _)| frame == texture)
                || !texture
                    .usage()
                    .contains(wgpu::TextureUsages::TEXTURE_BINDING)
            {
                continue;
            }
            let bound = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Splitscreen compositor frame"),
                layout: &self.layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(
                        &texture.create_view(&Default::default()),
                    ),
                }],
            });
            self.frames.push((texture.clone(), bound));
        }
        let mut pass = clear(encoder, target);
        pass.set_pipeline(&self.pipeline);
        for &(texture, [x, y]) in views {
            let [width, height] = clipped_size([texture.width(), texture.height()], [x, y], size);
            let bound = self.frames.iter().find(|(frame, _)| frame == texture);
            let Some((_, bound)) = bound.filter(|_| width > 0 && height > 0) else {
                continue;
            };
            // The viewport is the whole frame's place, past the target's edges too, so a
            // clipped frame keeps its scale; the scissor is the part on the target.
            pass.set_viewport(
                x as f32,
                y as f32,
                texture.width() as f32,
                texture.height() as f32,
                0.0,
                1.0,
            );
            pass.set_scissor_rect(x, y, width, height);
            pass.set_bind_group(0, bound, &[]);
            pass.draw(0..3, 0..1);
        }
    }
}

/// Frames never reach past the surface, including while a resized view still has its old frame.
fn clipped_size(view: [u32; 2], at: [u32; 2], surface: [u32; 2]) -> [u32; 2] {
    [
        view[0].min(surface[0].saturating_sub(at[0])),
        view[1].min(surface[1].saturating_sub(at[1])),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_are_clipped_to_the_window_during_resize() {
        assert_eq!(clipped_size([640, 480], [0, 0], [1280, 720]), [640, 480]);
        assert_eq!(
            clipped_size([640, 480], [1000, 600], [1280, 720]),
            [280, 120]
        );
        assert_eq!(clipped_size([640, 480], [1280, 720], [1280, 720]), [0, 0]);
        assert_eq!(
            clipped_size([640, 480], [u32::MAX, 0], [1280, 720]),
            [0, 480]
        );
    }

    /// A BGRA frame whose every texel says where it is: blue its column, green its row, and
    /// red `mark`.
    fn frame(device: &wgpu::Device, queue: &wgpu::Queue, mark: u8) -> wgpu::Texture {
        let size = wgpu::Extent3d {
            width: 8,
            height: 8,
            depth_or_array_layers: 1,
        };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FRAME,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let texels: Vec<u8> = (0..8)
            .flat_map(|row| (0..8).flat_map(move |column| [column, row, mark, 255]))
            .collect();
        queue.write_texture(
            texture.as_image_copy(),
            &texels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(8 * 4),
                rows_per_image: Some(8),
            },
            size,
        );
        texture
    }

    #[test]
    fn drawn_frames_land_texel_for_pixel_where_the_copy_puts_them() {
        const WIDTH: u32 = 12;
        const HEIGHT: u32 = 8;
        let _gpu = crate::gfx::test_gpu::lock();
        let Some(adapter) = crate::gfx::test_gpu::adapter() else {
            return;
        };
        let info = adapter.get_info();
        eprintln!("drawn on {} ({:?})", info.name, info.backend);
        let (device, queue) = crate::gfx::test_gpu::device(&adapter);
        // A surface that offers Rgba8Unorm alone: the frames' channels swap on the way.
        let format = wgpu::TextureFormat::Rgba8Unorm;
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: WIDTH,
                height: HEIGHT,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let frames = [100, 200, 50].map(|mark| frame(&device, &queue, mark));
        let mut encoder = device.create_command_encoder(&Default::default());
        // The second is cut at the right and the bottom; the third starts past the edge.
        Draw::new(&device, format).views(
            &device,
            &mut encoder,
            &target.create_view(&Default::default()),
            [WIDTH, HEIGHT],
            &[
                (&frames[0], [0, 0]),
                (&frames[1], [8, 2]),
                (&frames[2], [WIDTH, 0]),
            ],
        );
        let stride = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: u64::from(stride * HEIGHT),
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        encoder.copy_texture_to_buffer(
            target.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(stride),
                    rows_per_image: Some(HEIGHT),
                },
            },
            target.size(),
        );
        queue.submit([encoder.finish()]);
        buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, |r| r.unwrap());
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        let data = buffer.slice(..).get_mapped_range().unwrap();
        for y in 0..HEIGHT {
            for x in 0..WIDTH {
                let at = (y * stride + x * 4) as usize;
                let (x, y) = (x as u8, y as u8);
                let expected = match (x, y) {
                    (0..8, _) => [100, y, x, 255],
                    (8.., 2..) => [200, y - 2, x - 8, 255],
                    _ => [0, 0, 0, 255],
                };
                assert_eq!(data[at..at + 4], expected, "pixel ({x}, {y}), as RGBA");
            }
        }
    }
}
