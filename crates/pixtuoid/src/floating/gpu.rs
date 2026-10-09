//! The window's GPU painter: a composition's layers as textured quads,
//! sampled nearest so each layer's whole-number scale stays pixel-exact.

use anyhow::Context;
use pixtuoid_core::sprite::RgbBuffer;

use super::compose::{Change, Composition, LayerId, LayerPixels};

/// The window's one pipeline: a textured quad per layer.
const SHADER: &str = r"
struct VOut { @builtin(position) pos: vec4f, @location(0) uv: vec2f }
@vertex fn vs(@location(0) pos: vec2f, @location(1) uv: vec2f) -> VOut {
    var o: VOut; o.pos = vec4f(pos, 0.0, 1.0); o.uv = uv; return o;
}
@group(0) @binding(0) var t: texture_2d<f32>;
@group(0) @binding(1) var s: sampler;
@fragment fn fs(i: VOut) -> @location(0) vec4f { return textureSample(t, s, i.uv); }
";

/// A vertex: clip-space position, then texture coordinate.
const VERTEX: [wgpu::VertexAttribute; 2] = wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2];
const VERTEX_FLOATS: usize = 4;
const VERTEX_BYTES: u64 = (VERTEX_FLOATS * std::mem::size_of::<f32>()) as u64;
/// Two triangles a quad.
const QUAD_VERTICES: u32 = 6;

/// The adapter to draw with: the host's, else its software fallback,
/// "generally a 'software' implementation" (wgpu's `force_fallback_adapter`).
pub(super) fn adapter(
    instance: &wgpu::Instance,
    surface: Option<&wgpu::Surface<'_>>,
) -> anyhow::Result<wgpu::Adapter> {
    let request = |force_fallback_adapter| {
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            // An ambient window has no reason to wake a discrete GPU.
            power_preference: wgpu::PowerPreference::LowPower,
            force_fallback_adapter,
            compatible_surface: surface,
            apply_limit_buckets: false,
        }))
    };
    request(false).or_else(|_| request(true)).context(
        "no GPU adapter, hardware or software; `pixtuoid run` shows the office in the terminal",
    )
}

/// A device that runs on a GLES3-only adapter, with the adapter's own
/// texture limits for a large window.
pub(super) fn device(adapter: &wgpu::Adapter) -> anyhow::Result<(wgpu::Device, wgpu::Queue)> {
    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("pixtuoid floating"),
        required_limits:
            wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits()),
        ..Default::default()
    }))
    .context("opening the GPU device")
}

/// One layer's texture, and the bind group the pass draws it with.
#[derive(Debug)]
struct Texture {
    tex: wgpu::Texture,
    bind: wgpu::BindGroup,
    size: (u32, u32),
}

/// Draws compositions: one texture per layer, kept between frames, so an
/// unchanged layer costs no upload.
#[derive(Debug)]
pub(super) struct Painter {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    verts: wgpu::Buffer,
    textures: [Option<Texture>; LayerId::ALL.len()],
    /// An office region's BGRA bytes on their way to its texture.
    staging: Vec<u8>,
}

impl Painter {
    /// A painter drawing into textures of format `target`.
    pub(super) fn new(
        device: wgpu::Device,
        queue: wgpu::Queue,
        target: wgpu::TextureFormat,
    ) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: None,
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: None,
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: VERTEX_BYTES,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &VERTEX,
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        let verts = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: LayerId::ALL.len() as u64 * u64::from(QUAD_VERTICES) * VERTEX_BYTES,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            device,
            queue,
            pipeline,
            layout,
            sampler,
            verts,
            textures: Default::default(),
            staging: Vec::new(),
        }
    }

    pub(super) fn device(&self) -> &wgpu::Device {
        &self.device
    }

    pub(super) fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    /// Bring each layer's texture to `c`: the whole layer when `whole`, it is
    /// new or resized, or it changed `All`; only its rects for
    /// [`Change::Rects`]; nothing when unchanged.
    pub(super) fn upload(&mut self, c: &Composition<'_>, whole: bool) {
        for layer in &c.layers {
            let size = layer.pixels.size();
            if size.0 == 0 || size.1 == 0 {
                continue;
            }
            let fresh = self.ensure(layer.id, size);
            let change = if whole || fresh {
                Change::All
            } else {
                layer.change
            };
            match (layer.pixels, change) {
                (_, Change::Unchanged) => {}
                (LayerPixels::Rgba(rgba), _) => self.write(layer.id, (0, 0), size, rgba.bytes()),
                (LayerPixels::Opaque(buf), Change::All) => {
                    self.write_office(layer.id, buf, (0, 0, size.0, size.1));
                }
                (LayerPixels::Opaque(buf), Change::Rects(rects)) => {
                    for r in rects {
                        let x1 = (u32::from(r.x) + u32::from(r.width)).min(size.0);
                        let y1 = (u32::from(r.y) + u32::from(r.height)).min(size.1);
                        let (x0, y0) = (u32::from(r.x).min(x1), u32::from(r.y).min(y1));
                        self.write_office(layer.id, buf, (x0, y0, x1 - x0, y1 - y0));
                    }
                }
            }
        }
    }

    /// Give layer `id` a texture `size` big; whether it had to make one.
    fn ensure(&mut self, id: LayerId, size: (u32, u32)) -> bool {
        if self.textures[id.index()]
            .as_ref()
            .is_some_and(|t| t.size == size)
        {
            return false;
        }
        let tex = self.device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Bgra8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = tex.create_view(&Default::default());
        let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        self.textures[id.index()] = Some(Texture { tex, bind, size });
        true
    }

    /// `bytes`, BGRA rows `w` wide, into layer `id`'s texture at `(x, y)`.
    fn write(&self, id: LayerId, (x, y): (u32, u32), (w, h): (u32, u32), bytes: &[u8]) {
        let Some(texture) = &self.textures[id.index()] else {
            return;
        };
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture.tex,
                mip_level: 0,
                origin: wgpu::Origin3d { x, y, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            bytes,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * w),
                rows_per_image: Some(h),
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
    }

    /// The `(x, y, w, h)` region of `buf`, converted to BGRA, to its texture.
    fn write_office(&mut self, id: LayerId, buf: &RgbBuffer, (x, y, w, h): (u32, u32, u32, u32)) {
        if w == 0 || h == 0 {
            return;
        }
        let bw = usize::from(buf.width());
        let src = buf.as_slice();
        let mut staging = std::mem::take(&mut self.staging);
        staging.clear();
        for row in y as usize..(y + h) as usize {
            let line = &src[row * bw + x as usize..][..w as usize];
            staging.extend(line.iter().flat_map(|p| [p.b, p.g, p.r, u8::MAX]));
        }
        self.write(id, (x, y), (w, h), &staging);
        self.staging = staging;
    }

    /// Draw `c` into `target`, cleared to black, one quad a layer in paint
    /// order.
    pub(super) fn draw(&mut self, c: &Composition<'_>, target: &wgpu::TextureView) {
        let (sw, sh) = (c.window.0.max(1) as f32, c.window.1.max(1) as f32);
        let mut floats: Vec<f32> = Vec::new();
        let mut drawn = Vec::new();
        for layer in &c.layers {
            let (tw, th) = layer.pixels.size();
            let empty = tw == 0 || th == 0 || layer.extent.0 == 0 || layer.extent.1 == 0;
            if empty || self.textures[layer.id.index()].is_none() {
                continue;
            }
            let s = f32::from(layer.scale.max(1));
            let (x, y) = (layer.origin.0 as f32, layer.origin.1 as f32);
            let (w, h) = (layer.extent.0 as f32, layer.extent.1 as f32);
            let (x0, x1) = (x / sw * 2.0 - 1.0, (x + w) / sw * 2.0 - 1.0);
            let (y0, y1) = (1.0 - y / sh * 2.0, 1.0 - (y + h) / sh * 2.0);
            // Past the texture's own pixels × scale, `ClampToEdge` repeats
            // its edge texel, as the CPU compositor does.
            let (u1, v1) = (w / (tw as f32 * s), h / (th as f32 * s));
            floats.extend_from_slice(&[
                x0, y0, 0.0, 0.0, x1, y0, u1, 0.0, x0, y1, 0.0, v1, //
                x0, y1, 0.0, v1, x1, y0, u1, 0.0, x1, y1, u1, v1,
            ]);
            drawn.push(layer.id);
        }
        let bytes: Vec<u8> = floats.iter().flat_map(|f| f.to_ne_bytes()).collect();
        if !bytes.is_empty() {
            self.queue.write_buffer(&self.verts, 0, &bytes);
        }
        let mut encoder = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_vertex_buffer(0, self.verts.slice(..));
            for (quad, id) in (0u32..).zip(drawn) {
                if let Some(texture) = &self.textures[id.index()] {
                    pass.set_bind_group(0, &texture.bind, &[]);
                    let first = quad * QUAD_VERTICES;
                    pass.draw(first..first + QUAD_VERTICES, 0..1);
                }
            }
        }
        self.queue.submit([encoder.finish()]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::floating::compose::{
        Change, Composition, Layer, LayerId, LayerPixels, PxRect, RgbaLayer, XrgbSurface, composite,
    };
    use pixtuoid_core::sprite::{Rgb, RgbBuffer};
    use pixtuoid_scene::cutaway::Canvas;
    use pixtuoid_scene::layout::Bounds;

    /// A painter on whatever adapter the host has, hardware or software.
    /// None fails the test: a skip would pass with the presenter untested.
    fn painter() -> Painter {
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapter = adapter(&instance, None).expect(
            "a GPU adapter: Linux needs mesa-vulkan-drivers (lavapipe), which CI's setup-gpu installs",
        );
        let (device, queue) = device(&adapter).expect("a device");
        Painter::new(device, queue, wgpu::TextureFormat::Bgra8Unorm)
    }

    /// `c` drawn by `painter`, read back as `0x00RRGGBB` rows.
    fn drawn(painter: &mut Painter, c: &Composition<'_>, whole: bool) -> Vec<u32> {
        let (w, h) = c.window;
        let target = painter.device().create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Bgra8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        painter.upload(c, whole);
        painter.draw(c, &target.create_view(&Default::default()));
        let row = (4 * w).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let out = painter.device().create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: u64::from(row * h),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = painter.device().create_command_encoder(&Default::default());
        enc.copy_texture_to_buffer(
            target.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &out,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row),
                    rows_per_image: Some(h),
                },
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        painter.queue().submit([enc.finish()]);
        out.slice(..)
            .map_async(wgpu::MapMode::Read, |r| r.expect("mapped"));
        painter
            .device()
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("polled");
        let bytes = out.slice(..).get_mapped_range().expect("mapped range");
        (0..h as usize)
            .flat_map(|y| {
                bytes[y * row as usize..][..4 * w as usize]
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|[b, g, r, _]| u32::from(*r) << 16 | u32::from(*g) << 8 | u32::from(*b))
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    fn cpu(c: &Composition<'_>) -> Vec<u32> {
        let (w, h) = (c.window.0 as usize, c.window.1 as usize);
        let mut px = vec![0u32; w * h];
        composite(c, &mut XrgbSurface::new(&mut px, w, h).expect("sized"));
        px
    }

    fn assert_close(gpu: &[u32], cpu: &[u32], what: &str) {
        assert_eq!(gpu.len(), cpu.len(), "{what}: sizes");
        for (i, (&g, &c)) in gpu.iter().zip(cpu).enumerate() {
            let close =
                (0..3).all(|k| ((g >> (8 * k)) & 0xFF).abs_diff((c >> (8 * k)) & 0xFF) <= 1);
            assert!(close, "{what}: px {i}: gpu {g:06x} cpu {c:06x}");
        }
    }

    /// The GPU draws a composition as the CPU compositor does: the office's
    /// whole-number upscale with its edge remainder, an overlay's opaque,
    /// clear and shadow texels, and a partial office update after a whole
    /// one.
    #[test]
    fn gpu_matches_the_cpu_compositor() {
        let mut painter = painter();
        let px = |r, g, b| Rgb { r, g, b };
        let mut office = RgbBuffer::filled(3, 2, px(0, 0, 0));
        for (x, y, c) in [
            (0, 0, px(255, 0, 0)),
            (1, 0, px(0, 255, 0)),
            (2, 0, px(0, 0, 255)),
            (0, 1, px(10, 20, 30)),
            (1, 1, px(200, 100, 50)),
            (2, 1, px(1, 2, 3)),
        ] {
            office.put(x, y, c);
        }
        let mut tip = RgbaLayer::new(PxRect {
            x: 2,
            y: 1,
            w: 3,
            h: 2,
        });
        tip.set(2, 1, px(250, 240, 230));
        tip.shade(3, 1, pixtuoid_scene::display::cells::CARD_SHADOW);
        let window = (9, 7);
        let frame = |office, change| Composition {
            window,
            layers: vec![
                Layer {
                    id: LayerId::Office,
                    pixels: LayerPixels::Opaque(office),
                    origin: (0, 0),
                    scale: 2,
                    extent: (9, 6),
                    change,
                },
                Layer {
                    id: LayerId::Tooltip,
                    pixels: LayerPixels::Rgba(&tip),
                    origin: tip.origin(),
                    scale: 1,
                    extent: tip.size(),
                    change: Change::All,
                },
            ],
        };
        let whole = frame(&office, Change::All);
        assert_close(&drawn(&mut painter, &whole, true), &cpu(&whole), "whole");
        let mut changed = office.clone();
        changed.put(2, 1, px(99, 98, 97));
        let rect = [Bounds {
            x: 2,
            y: 1,
            width: 1,
            height: 1,
        }];
        let partial = frame(&changed, Change::Rects(&rect));
        assert_close(
            &drawn(&mut painter, &partial, false),
            &cpu(&partial),
            "partial",
        );
    }
}
