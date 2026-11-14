//! Offscreen wgpu renderer for the avatar. Uses Slint's own device/queue; the result is a
//! wgpu::Texture that Slint displays as an Image.

use crate::avatar::Vertex;
use crate::template;
use glam::Mat4;
use image::RgbaImage;
use slint::wgpu_30::wgpu;
use wgpu::util::DeviceExt;

const SHADER: &str = r#"
// hl = template-UV rect (u0, v0, u1, v1) of the face under the cursor; off-range when none.
// hk.x = texture flag of the hovered part (arms and legs share template areas, so the rect alone
// would also tint the other limb).
struct U { mvp: mat4x4<f32>, hl: vec4<f32>, hk: vec4<f32> };
@group(0) @binding(0) var<uniform> u: U;
@group(0) @binding(1) var t: texture_2d<f32>;
@group(0) @binding(2) var s: sampler;
@group(0) @binding(3) var tp: texture_2d<f32>;
struct VO {
  @builtin(position) pos: vec4<f32>,
  @location(0) n: vec3<f32>,
  @location(1) uv: vec2<f32>,
  @location(2) c: vec4<f32>,
};
@vertex fn vs(@location(0) p: vec3<f32>, @location(1) n: vec3<f32>, @location(2) uv: vec2<f32>, @location(3) c: vec4<f32>) -> VO {
  var o: VO;
  o.pos = u.mvp * vec4<f32>(p, 1.0);
  o.n = n; o.uv = uv; o.c = c;
  return o;
}
@fragment fn fs(i: VO) -> @location(0) vec4<f32> {
  let shirt = textureSample(t, s, i.uv);
  let pants = textureSample(tp, s, i.uv);
  // c.a flag (avatar::texture_flag): 0 shirt, 0.25 pants then shirt, 0.5 pants, 1 none.
  // Like Roblox: base colour, pants over it, shirt over that; transparent pixels show through.
  // Textures are premultiplied (see `upload`), so filtering never pulls in hidden colours.
  var col = i.c.rgb;
  col = select(col, col * (1.0 - pants.a) + pants.rgb, i.c.a > 0.2 && i.c.a < 0.6);
  col = select(col, col * (1.0 - shirt.a) + shirt.rgb, i.c.a < 0.3);
  let hovered = abs(i.c.a - u.hk.x) < 0.01 && all(i.uv >= u.hl.xy) && all(i.uv <= u.hl.zw);
  col = select(col, mix(col, vec3<f32>(0.45, 0.68, 1.0), 0.3), hovered);
  let l = 0.55 + 0.45 * max(dot(normalize(i.n), normalize(vec3<f32>(0.4, 0.8, 0.6))), 0.0);
  return vec4<f32>(col * l, 1.0);
}
"#;

const COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
/// 4x is the MSAA level every wgpu adapter supports for these formats.
const SAMPLES: u32 = 4;
/// 585 -> 292 -> 146. Further levels would blend neighbouring template faces across the 2 px gaps.
const MIPS: u32 = 3;

pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::RenderPipeline,
    vbuf: wgpu::Buffer,
    ibuf: wgpu::Buffer,
    n_idx: u32,
    ubuf: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    shirt: wgpu::Texture,
    pants: wgpu::Texture,
}

impl Renderer {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, verts: &[Vertex], idx: &[u32]) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("avatar"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let vbuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("verts"),
            contents: bytemuck::cast_slice(verts),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let ibuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("idx"),
            contents: bytemuck::cast_slice(idx),
            usage: wgpu::BufferUsages::INDEX,
        });
        let ubuf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("mvp"),
            size: 96, // mat4 + 2 vec4
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let clothing_texture = |label| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d { width: template::W, height: template::H, depth_or_array_layers: 1 },
                mip_level_count: MIPS,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                // Unorm (not sRGB): template bytes pass straight through to the Unorm output.
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            })
        };
        let shirt = clothing_texture("shirt");
        let pants = clothing_texture("pants");
        // Trilinear + 4x anisotropic, as Roblox draws clothing: no shimmer when the avatar is
        // small or seen edge-on. Bleed between faces is bounded by MIPS.
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            anisotropy_clamp: 4,
            ..Default::default()
        });
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let view = shirt.create_view(&Default::default());
        let pants_view = pants.create_view(&Default::default());
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: ubuf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&view) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&sampler) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&pants_view) },
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&bgl)],
            ..Default::default()
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("avatar"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Vertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x2, 3 => Float32x4],
                })],
            },
            primitive: wgpu::PrimitiveState::default(), // no culling
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState { count: SAMPLES, ..Default::default() },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: COLOR_FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        Renderer {
            device: device.clone(),
            queue: queue.clone(),
            pipeline,
            vbuf,
            ibuf,
            n_idx: idx.len() as u32,
            ubuf,
            bind_group,
            shirt,
            pants,
        }
    }

    pub fn set_mesh(&mut self, verts: &[Vertex], idx: &[u32]) {
        self.vbuf = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("verts"),
            contents: bytemuck::cast_slice(verts),
            usage: wgpu::BufferUsages::VERTEX,
        });
        self.ibuf = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("idx"),
            contents: bytemuck::cast_slice(idx),
            usage: wgpu::BufferUsages::INDEX,
        });
        self.n_idx = idx.len() as u32;
    }

    /// Upload a composed 585x559 garment texture (the shirt and pants each have their own).
    /// Only the GPU copy is premultiplied and mipmapped; `img` (the export) is untouched.
    pub fn upload(&self, garment: template::Garment, img: &RgbaImage) {
        assert_eq!(img.dimensions(), (template::W, template::H));
        let tex = match garment {
            template::Garment::Shirt => &self.shirt,
            template::Garment::Pants => &self.pants,
        };
        for (level, m) in mip_chain(img).iter().enumerate() {
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo { texture: tex, mip_level: level as u32, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                m.as_raw(),
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4 * m.width()), rows_per_image: Some(m.height()) },
                wgpu::Extent3d { width: m.width(), height: m.height(), depth_or_array_layers: 1 },
            );
        }
    }

    /// `highlight`: template-UV rect (u0, v0, u1, v1) and texture flag of the face to tint, e.g. the
    /// face under the cursor.
    pub fn render(&self, w: u32, h: u32, view_proj: Mat4, highlight: Option<([f32; 4], f32)>) -> wgpu::Texture {
        let size = wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 };
        let mk = |format, usage, sample_count| {
            self.device.create_texture(&wgpu::TextureDescriptor {
                label: None,
                size,
                mip_level_count: 1,
                sample_count,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        // shortcut: new target textures per frame; fine for on-demand redraws, pool them if profiling says so.
        let color = mk(COLOR_FORMAT, wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC, 1);
        let msaa = mk(COLOR_FORMAT, wgpu::TextureUsages::RENDER_ATTACHMENT, SAMPLES);
        let depth = mk(DEPTH_FORMAT, wgpu::TextureUsages::RENDER_ATTACHMENT, SAMPLES);
        let mut u = view_proj.to_cols_array().to_vec();
        let (rect, flag) = highlight.unwrap_or(([2.0; 4], -1.0)); // 2.0 is outside every UV: no tint
        u.extend(rect);
        u.extend([flag, 0.0, 0.0, 0.0]);
        self.queue.write_buffer(&self.ubuf, 0, bytemuck::cast_slice(&u));
        let (cv, mv, dv) = (color.create_view(&Default::default()), msaa.create_view(&Default::default()), depth.create_view(&Default::default()));
        let mut enc = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &mv,
                    depth_slice: None,
                    resolve_target: Some(&cv),
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.13, g: 0.14, b: 0.16, a: 1.0 }),
                        store: wgpu::StoreOp::Discard,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &dv,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Discard }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.set_vertex_buffer(0, self.vbuf.slice(..));
            pass.set_index_buffer(self.ibuf.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..self.n_idx, 0, 0..1);
        }
        self.queue.submit([enc.finish()]);
        color
    }
}

/// Level 0 premultiplied; each next level a 2x2 average in linear light (odd edges clamp).
fn mip_chain(img: &RgbaImage) -> Vec<RgbaImage> {
    use crate::compose::{to_srgb_u8, TO_LINEAR};
    let premul = |p: &image::Rgba<u8>| {
        let m = |c: u8| ((c as u16 * p[3] as u16 + 127) / 255) as u8;
        image::Rgba([m(p[0]), m(p[1]), m(p[2]), p[3]])
    };
    let mut levels = vec![RgbaImage::from_fn(img.width(), img.height(), |x, y| premul(img.get_pixel(x, y)))];
    let mut src = img.clone();
    for _ in 1..MIPS {
        let (w, h) = ((src.width() / 2).max(1), (src.height() / 2).max(1));
        let next = RgbaImage::from_fn(w, h, |x, y| {
            let (mut rgb, mut a) = ([0f32; 3], 0f32);
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let p = src.get_pixel((2 * x + dx).min(src.width() - 1), (2 * y + dy).min(src.height() - 1));
                let pa = p[3] as f32 / 255.0;
                for c in 0..3 {
                    rgb[c] += TO_LINEAR[p[c] as usize] * pa;
                }
                a += pa;
            }
            let s = |c: usize| if a > 0.0 { to_srgb_u8(rgb[c] / a) } else { 0 };
            image::Rgba([s(0), s(1), s(2), (a / 4.0 * 255.0).round() as u8])
        });
        levels.push(RgbaImage::from_fn(w, h, |x, y| premul(next.get_pixel(x, y))));
        src = next;
    }
    levels
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mips_are_premultiplied_and_ignore_hidden_colours() {
        // Opaque red next to transparent pixels that hide pure green.
        let img = RgbaImage::from_fn(template::W, template::H, |x, _| {
            if x % 2 == 0 { image::Rgba([220, 20, 20, 255]) } else { image::Rgba([0, 255, 0, 0]) }
        });
        let m = mip_chain(&img);
        assert_eq!(m.iter().map(|l| l.dimensions()).collect::<Vec<_>>(), [(585, 559), (292, 279), (146, 139)]);
        assert_eq!(m[0].get_pixel(1, 0).0, [0, 0, 0, 0]);
        // Level 1: half-covered red: premultiplied red stays red, green never appears.
        let p = m[1].get_pixel(0, 0).0;
        assert_eq!((p[1], p[3]), (10, 128), "{p:?}");
        assert!((p[0] as i32 - 110).abs() <= 1, "{p:?}");
    }

    fn block_on<F: std::future::Future>(f: F) -> F::Output {
        let mut f = std::pin::pin!(f);
        let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
        loop {
            if let std::task::Poll::Ready(v) = f.as_mut().poll(&mut cx) {
                return v;
            }
            std::thread::yield_now();
        }
    }

    /// Renders the avatar wearing docs/test_art.png at several sizes, saves PNGs to
    /// $CS_QUALITY_DIR and prints GPU frame times. Needs a GPU.
    #[test]
    #[ignore]
    fn preview_report() {
        let instance = wgpu::Instance::default();
        let adapter = block_on(instance.request_adapter(&Default::default())).expect("no GPU adapter");
        let (device, queue) = block_on(adapter.request_device(&Default::default())).unwrap();
        let (v, i) = crate::avatar::mesh(&crate::avatar::rig());
        let r = Renderer::new(&device, &queue, &v, &i);
        let art = image::open("docs/test_art.png").unwrap().to_rgba8();
        let shirt = image::imageops::resize(&art, template::W, template::H, image::imageops::FilterType::Triangle);
        r.upload(template::Garment::Shirt, &shirt);
        r.upload(template::Garment::Pants, &RgbaImage::new(template::W, template::H));
        let dir = std::env::var("CS_QUALITY_DIR").ok();
        for (w, h, dist, yaw) in [(800u32, 800u32, 9.0f32, 0.0f32), (200, 200, 9.0, 0.0), (400, 400, 9.0, 1.2), (160, 160, 20.0, 0.5)] {
            let cam = crate::avatar::Camera { dist, yaw, ..Default::default() };
            let vp = cam.view_proj(w as f32 / h as f32);
            let _ = r.render(w, h, vp, None);
            device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
            let t = std::time::Instant::now();
            const N: u32 = 50;
            for _ in 0..N {
                let _ = r.render(w, h, vp, None);
            }
            device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
            println!("{w}x{h} dist {dist} yaw {yaw}: {:.3} ms/frame", t.elapsed().as_secs_f64() * 1000.0 / N as f64);
            let tex = r.render(w, h, vp, None);
            let row = (4 * w).div_ceil(256) * 256;
            let buf = device.create_buffer(&wgpu::BufferDescriptor { label: None, size: (row * h) as u64, usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
            let mut enc = device.create_command_encoder(&Default::default());
            enc.copy_texture_to_buffer(
                tex.as_image_copy(),
                wgpu::TexelCopyBufferInfo { buffer: &buf, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(h) } },
                tex.size(),
            );
            queue.submit([enc.finish()]);
            buf.slice(..).map_async(wgpu::MapMode::Read, |_| {});
            device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
            let data = buf.slice(..).get_mapped_range().unwrap();
            let img = RgbaImage::from_fn(w, h, |x, y| {
                let o = (y * row + 4 * x) as usize;
                image::Rgba([data[o], data[o + 1], data[o + 2], 255])
            });
            if let Some(d) = &dir {
                std::fs::create_dir_all(d).unwrap();
                img.save(format!("{d}/preview_{w}_d{dist}_y{yaw}.png")).unwrap();
            }
        }
    }
}
