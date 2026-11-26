//! Draws the main window: the video (converted from YUV on the GPU) plus the
//! borders / background around it, in a single full-screen pass.

use crate::gfx::{viewport, Gfx};
use crate::video::{Frame, PixFmt};
use windows::core::Result;
use windows::Win32::Graphics::Direct3D::D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;

const FULLSCREEN: &str = include_str!("shaders/fullscreen.hlsl");
const CONVERT: &str = include_str!("shaders/convert.hlsl");
const COMPOSITE: &str = include_str!("shaders/composite.hlsl");

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct CompCb {
    win: [f32; 2],
    tex: [f32; 2],
    vrect: [f32; 4],
    frame_col: [f32; 4],
    zones: [u32; 4],
    img_rect: [f32; 4],
    time: f32,
    filter: u32,
    has_video: u32,
    twinkle: u32,
    img_dim: f32,
    star_density: f32,
    has_img: u32,
    pad0: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct ConvCb {
    cp0: [f32; 4],
    cp1: [f32; 4],
}

/// Everything the composite pass needs to know about the current look.
pub struct Look {
    /// Where the video goes, in window pixels: x0, y0, x1, y1.
    pub vrect: [f32; 4],
    pub has_video: bool,
    /// Fill for left, right, top, bottom (0 black, 1 white, 2 galaxy, 3 image).
    pub zones: [u32; 4],
    pub frame_rgb: [f32; 3],
    pub frame_px: f32,
    /// 0 sharp, 1 smooth, 2 pixel
    pub filter: u32,
    pub time: f32,
    pub twinkle: bool,
    pub star_density: f32,
    pub img_rect: [f32; 4],
    pub img_dim: f32,
}

struct Input {
    fmt: PixFmt,
    w: u32,
    h: u32,
    t0: (ID3D11Texture2D, ID3D11ShaderResourceView),
    t1: Option<(ID3D11Texture2D, ID3D11ShaderResourceView)>,
}

struct Target {
    w: u32,
    h: u32,
    srv: ID3D11ShaderResourceView,
    rtv: ID3D11RenderTargetView,
}

pub struct VideoRenderer {
    vs: ID3D11VertexShader,
    ps_yuy2: ID3D11PixelShader,
    ps_nv12: ID3D11PixelShader,
    ps_comp: ID3D11PixelShader,
    cb_comp: ID3D11Buffer,
    cb_conv: ID3D11Buffer,
    lin: ID3D11SamplerState,
    pt: ID3D11SamplerState,
    input: Option<Input>,
    target: Option<Target>,
    /// What the composite pass samples: the converted frame, or the RGB upload directly.
    frame_srv: Option<ID3D11ShaderResourceView>,
    pub frame_size: Option<(u32, u32)>,
    img: Option<(ID3D11ShaderResourceView, u32, u32)>,
    black: ID3D11ShaderResourceView,
}

impl VideoRenderer {
    pub fn new(gfx: &Gfx) -> Result<Self> {
        let vs_bytes = gfx.compile(FULLSCREEN, "vs_full", "vs_4_0")?;
        let vs = gfx.vertex_shader(&vs_bytes)?;
        let conv = format!("{FULLSCREEN}\n{CONVERT}");
        let comp = format!("{FULLSCREEN}\n{COMPOSITE}");
        let (_, black) = gfx.static_texture(1, 1, DXGI_FORMAT_R8G8B8A8_UNORM, &[0, 0, 0, 255], 4)?;
        Ok(Self {
            vs,
            ps_yuy2: gfx.pixel_shader(&conv, "ps_yuy2")?,
            ps_nv12: gfx.pixel_shader(&conv, "ps_nv12")?,
            ps_comp: gfx.pixel_shader(&comp, "ps_comp")?,
            cb_comp: gfx.dynamic_cbuffer(std::mem::size_of::<CompCb>())?,
            cb_conv: gfx.dynamic_cbuffer(std::mem::size_of::<ConvCb>())?,
            lin: gfx.sampler(D3D11_FILTER_MIN_MAG_MIP_LINEAR)?,
            pt: gfx.sampler(D3D11_FILTER_MIN_MAG_MIP_POINT)?,
            input: None,
            target: None,
            frame_srv: None,
            frame_size: None,
            img: None,
            black,
        })
    }

    pub fn has_frame(&self) -> bool {
        self.frame_srv.is_some()
    }

    pub fn clear_frame(&mut self) {
        self.frame_srv = None;
        self.frame_size = None;
    }

    /// Replaces the custom background image (RGBA8, unpremultiplied).
    pub fn set_image(&mut self, gfx: &Gfx, img: Option<(&[u8], u32, u32)>) {
        self.img = img.and_then(|(px, w, h)| {
            gfx.static_texture(w, h, DXGI_FORMAT_R8G8B8A8_UNORM, px, w * 4)
                .ok()
                .map(|(_, srv)| (srv, w, h))
        });
    }

    pub fn image_size(&self) -> Option<(u32, u32)> {
        self.img.as_ref().map(|(_, w, h)| (*w, *h))
    }

    /// Uploads a new video frame and converts it to RGB.
    pub fn upload(&mut self, gfx: &Gfx, f: &Frame, full_range: bool, bt709: bool) -> Result<()> {
        let (w, h) = (f.width, f.height);
        if w == 0 || h == 0 {
            return Ok(());
        }
        let need_new = !matches!(&self.input, Some(i) if i.fmt == f.fmt && i.w == w && i.h == h);
        if need_new {
            self.input = Some(match f.fmt {
                PixFmt::Yuy2 => Input {
                    fmt: f.fmt,
                    w,
                    h,
                    t0: gfx.dynamic_texture(w / 2, h, DXGI_FORMAT_R8G8B8A8_UNORM)?,
                    t1: None,
                },
                PixFmt::Nv12 => Input {
                    fmt: f.fmt,
                    w,
                    h,
                    t0: gfx.dynamic_texture(w, h, DXGI_FORMAT_R8_UNORM)?,
                    t1: Some(gfx.dynamic_texture(w / 2, h / 2, DXGI_FORMAT_R8G8_UNORM)?),
                },
                PixFmt::Bgra => Input {
                    fmt: f.fmt,
                    w,
                    h,
                    t0: gfx.dynamic_texture(w, h, DXGI_FORMAT_B8G8R8A8_UNORM)?,
                    t1: None,
                },
            });
        }
        let input = self.input.as_ref().unwrap();

        match f.fmt {
            PixFmt::Yuy2 => {
                let row = w as usize * 2;
                if f.data.len() < row * h as usize {
                    return Ok(());
                }
                write_rows(gfx, &input.t0.0, &f.data, row, h as usize);
            }
            PixFmt::Nv12 => {
                let row = w as usize;
                let y_size = row * h as usize;
                if f.data.len() < y_size + row * (h as usize / 2) {
                    return Ok(());
                }
                write_rows(gfx, &input.t0.0, &f.data[..y_size], row, h as usize);
                if let Some(t1) = &input.t1 {
                    write_rows(gfx, &t1.0, &f.data[y_size..], row, h as usize / 2);
                }
            }
            PixFmt::Bgra => {
                let row = w as usize * 4;
                if f.data.len() < row * h as usize {
                    return Ok(());
                }
                write_rows(gfx, &input.t0.0, &f.data, row, h as usize);
                self.frame_srv = Some(input.t0.1.clone());
                self.frame_size = Some((w, h));
                return Ok(());
            }
        }

        // YUV -> RGB into a texture the composite pass can scale smoothly.
        if !matches!(&self.target, Some(t) if t.w == w && t.h == h) {
            let (tex, srv) = gfx.texture(
                w,
                h,
                DXGI_FORMAT_B8G8R8A8_UNORM,
                D3D11_USAGE_DEFAULT,
                (D3D11_BIND_SHADER_RESOURCE.0 | D3D11_BIND_RENDER_TARGET.0) as u32,
                0,
                None,
            )?;
            let mut rtv = None;
            unsafe { gfx.device.CreateRenderTargetView(&tex, None, Some(&mut rtv))? };
            self.target = Some(Target { w, h, srv, rtv: rtv.unwrap() });
        }
        let target = self.target.as_ref().unwrap();

        let (yoff, ys, cs) = if full_range { (0.0, 1.0, 1.0) } else { (16.0 / 255.0, 255.0 / 219.0, 255.0 / 224.0) };
        let k = if bt709 { [1.5748, 0.187324, 0.468124, 1.8556] } else { [1.402, 0.344136, 0.714136, 1.772] };
        gfx.write_buffer(&self.cb_conv, &[ConvCb { cp0: [yoff, ys, cs, 0.0], cp1: k }]);

        let ps = if f.fmt == PixFmt::Nv12 { &self.ps_nv12 } else { &self.ps_yuy2 };
        unsafe {
            let c = &gfx.ctx;
            c.OMSetRenderTargets(Some(&[Some(target.rtv.clone())]), None);
            c.RSSetViewports(Some(&[viewport(w, h)]));
            c.RSSetState(None);
            c.OMSetBlendState(None, None, 0xffff_ffff);
            c.IASetInputLayout(None);
            c.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            c.VSSetShader(&self.vs, None);
            c.PSSetShader(ps, None);
            c.PSSetConstantBuffers(1, Some(&[Some(self.cb_conv.clone())]));
            c.PSSetShaderResources(0, Some(&[Some(input.t0.1.clone()), input.t1.as_ref().map(|t| t.1.clone())]));
            c.Draw(3, 0);
            c.PSSetShaderResources(0, Some(&[None, None]));
            c.OMSetRenderTargets(None, None);
        }
        self.frame_srv = Some(target.srv.clone());
        self.frame_size = Some((w, h));
        Ok(())
    }

    /// Draws video + borders into the currently bound window target.
    pub fn draw(&self, gfx: &Gfx, win_w: u32, win_h: u32, look: &Look) {
        let (tw, th) = self.frame_size.unwrap_or((1, 1));
        let has_video = look.has_video && self.frame_srv.is_some();
        let cb = CompCb {
            win: [win_w as f32, win_h as f32],
            tex: [tw as f32, th as f32],
            vrect: if has_video { look.vrect } else { [0.0; 4] },
            frame_col: [look.frame_rgb[0], look.frame_rgb[1], look.frame_rgb[2], look.frame_px],
            zones: look.zones,
            img_rect: look.img_rect,
            time: look.time,
            filter: look.filter,
            has_video: has_video as u32,
            twinkle: look.twinkle as u32,
            img_dim: look.img_dim,
            star_density: look.star_density,
            has_img: self.img.is_some() as u32,
            pad0: 0.0,
        };
        gfx.write_buffer(&self.cb_comp, &[cb]);
        let frame = if has_video { self.frame_srv.clone() } else { Some(self.black.clone()) };
        let img = self.img.as_ref().map(|i| i.0.clone()).or_else(|| Some(self.black.clone()));
        unsafe {
            let c = &gfx.ctx;
            c.RSSetState(None);
            c.OMSetBlendState(None, None, 0xffff_ffff);
            c.IASetInputLayout(None);
            c.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            c.VSSetShader(&self.vs, None);
            c.PSSetShader(&self.ps_comp, None);
            c.PSSetConstantBuffers(0, Some(&[Some(self.cb_comp.clone())]));
            c.PSSetShaderResources(0, Some(&[frame, img]));
            c.PSSetSamplers(0, Some(&[Some(self.lin.clone()), Some(self.pt.clone())]));
            c.Draw(3, 0);
            c.PSSetShaderResources(0, Some(&[None, None]));
        }
    }
}

fn write_rows(gfx: &Gfx, tex: &ID3D11Texture2D, data: &[u8], row: usize, rows: usize) {
    unsafe {
        let mut m = D3D11_MAPPED_SUBRESOURCE::default();
        if gfx.ctx.Map(tex, 0, D3D11_MAP_WRITE_DISCARD, 0, Some(&mut m)).is_err() {
            return;
        }
        let pitch = m.RowPitch as usize;
        let dst = m.pData as *mut u8;
        if pitch == row {
            std::ptr::copy_nonoverlapping(data.as_ptr(), dst, row * rows);
        } else {
            for r in 0..rows {
                std::ptr::copy_nonoverlapping(data.as_ptr().add(r * row), dst.add(r * pitch), row);
            }
        }
        gfx.ctx.Unmap(tex, 0);
    }
}
