//! Direct3D 11 basics: one device shared by both windows, a swap chain per window,
//! and a runtime HLSL compiler (d3dcompiler_47.dll ships with Windows).

use std::ffi::CString;
use windows::core::{Interface, Result, PCSTR};
use windows::Win32::Foundation::{HMODULE, HWND};
use windows::Win32::Graphics::Direct3D::Fxc::{D3DCompile, D3DCOMPILE_OPTIMIZATION_LEVEL3};
use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;

pub struct Gfx {
    pub device: ID3D11Device,
    pub ctx: ID3D11DeviceContext,
    pub factory: IDXGIFactory2,
}

impl Gfx {
    pub fn new() -> Result<Gfx> {
        unsafe {
            let levels = [D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_10_1, D3D_FEATURE_LEVEL_10_0];
            let mut device = None;
            let mut ctx = None;
            let flags = D3D11_CREATE_DEVICE_BGRA_SUPPORT;
            let hw = D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                flags,
                Some(&levels),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut ctx),
            );
            if hw.is_err() {
                crate::log!("gfx: no hardware device ({hw:?}), using WARP");
                D3D11CreateDevice(
                    None,
                    D3D_DRIVER_TYPE_WARP,
                    HMODULE::default(),
                    flags,
                    Some(&levels),
                    D3D11_SDK_VERSION,
                    Some(&mut device),
                    None,
                    Some(&mut ctx),
                )?;
            }
            let device = device.unwrap();
            let ctx = ctx.unwrap();
            let dxgi: IDXGIDevice1 = device.cast()?;
            // Never queue more than one frame: lowest delay.
            let _ = dxgi.SetMaximumFrameLatency(1);
            let adapter = dxgi.GetAdapter()?;
            let factory: IDXGIFactory2 = adapter.GetParent()?;
            Ok(Gfx { device, ctx, factory })
        }
    }

    pub fn compile(&self, src: &str, entry: &str, target: &str) -> Result<Vec<u8>> {
        unsafe {
            let entry_c = CString::new(entry).unwrap();
            let target_c = CString::new(target).unwrap();
            let mut code = None;
            let mut errors = None;
            let r = D3DCompile(
                src.as_ptr() as *const _,
                src.len(),
                PCSTR::null(),
                None,
                None,
                PCSTR(entry_c.as_ptr() as *const u8),
                PCSTR(target_c.as_ptr() as *const u8),
                D3DCOMPILE_OPTIMIZATION_LEVEL3,
                0,
                &mut code,
                Some(&mut errors),
            );
            if let Err(e) = r {
                if let Some(errors) = errors {
                    let msg = std::slice::from_raw_parts(errors.GetBufferPointer() as *const u8, errors.GetBufferSize());
                    crate::log!("shader {entry}: {}", String::from_utf8_lossy(msg));
                }
                return Err(e);
            }
            let code = code.unwrap();
            Ok(std::slice::from_raw_parts(code.GetBufferPointer() as *const u8, code.GetBufferSize()).to_vec())
        }
    }

    pub fn vertex_shader(&self, bytes: &[u8]) -> Result<ID3D11VertexShader> {
        let mut vs = None;
        unsafe { self.device.CreateVertexShader(bytes, None, Some(&mut vs))? };
        Ok(vs.unwrap())
    }

    pub fn pixel_shader(&self, src: &str, entry: &str) -> Result<ID3D11PixelShader> {
        let bytes = self.compile(src, entry, "ps_4_0")?;
        let mut ps = None;
        unsafe { self.device.CreatePixelShader(&bytes, None, Some(&mut ps))? };
        Ok(ps.unwrap())
    }

    pub fn sampler(&self, filter: D3D11_FILTER) -> Result<ID3D11SamplerState> {
        let desc = D3D11_SAMPLER_DESC {
            Filter: filter,
            AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
            AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
            AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
            MipLODBias: 0.0,
            MaxAnisotropy: 1,
            ComparisonFunc: D3D11_COMPARISON_NEVER,
            BorderColor: [0.0; 4],
            MinLOD: 0.0,
            MaxLOD: f32::MAX,
        };
        let mut s = None;
        unsafe { self.device.CreateSamplerState(&desc, Some(&mut s))? };
        Ok(s.unwrap())
    }

    /// A small constant buffer the CPU rewrites every draw.
    pub fn dynamic_cbuffer(&self, size: usize) -> Result<ID3D11Buffer> {
        let desc = D3D11_BUFFER_DESC {
            ByteWidth: (size.div_ceil(16) * 16) as u32,
            Usage: D3D11_USAGE_DYNAMIC,
            BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
            CPUAccessFlags: D3D11_CPU_ACCESS_WRITE.0 as u32,
            MiscFlags: 0,
            StructureByteStride: 0,
        };
        let mut b = None;
        unsafe { self.device.CreateBuffer(&desc, None, Some(&mut b))? };
        Ok(b.unwrap())
    }

    /// Overwrites a dynamic buffer with `data`.
    pub fn write_buffer<T: Copy>(&self, buf: &ID3D11Buffer, data: &[T]) {
        unsafe {
            let mut m = D3D11_MAPPED_SUBRESOURCE::default();
            if self.ctx.Map(buf, 0, D3D11_MAP_WRITE_DISCARD, 0, Some(&mut m)).is_ok() {
                std::ptr::copy_nonoverlapping(data.as_ptr() as *const u8, m.pData as *mut u8, std::mem::size_of_val(data));
                self.ctx.Unmap(buf, 0);
            }
        }
    }

    /// A texture the CPU refills often (video frames).
    pub fn dynamic_texture(&self, w: u32, h: u32, fmt: DXGI_FORMAT) -> Result<(ID3D11Texture2D, ID3D11ShaderResourceView)> {
        self.texture(w, h, fmt, D3D11_USAGE_DYNAMIC, D3D11_BIND_SHADER_RESOURCE.0 as u32, D3D11_CPU_ACCESS_WRITE.0 as u32, None)
    }

    /// A texture filled once (or rarely) from memory.
    pub fn static_texture(&self, w: u32, h: u32, fmt: DXGI_FORMAT, pixels: &[u8], row: u32) -> Result<(ID3D11Texture2D, ID3D11ShaderResourceView)> {
        let init = D3D11_SUBRESOURCE_DATA { pSysMem: pixels.as_ptr() as *const _, SysMemPitch: row, SysMemSlicePitch: 0 };
        self.texture(w, h, fmt, D3D11_USAGE_DEFAULT, D3D11_BIND_SHADER_RESOURCE.0 as u32, 0, Some(&init))
    }

    #[allow(clippy::too_many_arguments)]
    pub fn texture(
        &self,
        w: u32,
        h: u32,
        fmt: DXGI_FORMAT,
        usage: D3D11_USAGE,
        bind: u32,
        cpu: u32,
        init: Option<&D3D11_SUBRESOURCE_DATA>,
    ) -> Result<(ID3D11Texture2D, ID3D11ShaderResourceView)> {
        let desc = D3D11_TEXTURE2D_DESC {
            Width: w.max(1),
            Height: h.max(1),
            MipLevels: 1,
            ArraySize: 1,
            Format: fmt,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: usage,
            BindFlags: bind,
            CPUAccessFlags: cpu,
            MiscFlags: 0,
        };
        unsafe {
            let mut tex = None;
            self.device.CreateTexture2D(&desc, init.map(|i| i as *const _), Some(&mut tex))?;
            let tex = tex.unwrap();
            let mut srv = None;
            self.device.CreateShaderResourceView(&tex, None, Some(&mut srv))?;
            Ok((tex, srv.unwrap()))
        }
    }
}

/// The swap chain behind one window.
pub struct Surface {
    pub swap: IDXGISwapChain1,
    pub rtv: Option<ID3D11RenderTargetView>,
    pub width: u32,
    pub height: u32,
}

impl Surface {
    pub fn new(gfx: &Gfx, hwnd: HWND, width: u32, height: u32) -> Result<Surface> {
        let desc = DXGI_SWAP_CHAIN_DESC1 {
            Width: width.max(1),
            Height: height.max(1),
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            Stereo: false.into(),
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: 2,
            Scaling: DXGI_SCALING_STRETCH,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
            AlphaMode: DXGI_ALPHA_MODE_IGNORE,
            Flags: 0,
        };
        unsafe {
            let swap = gfx.factory.CreateSwapChainForHwnd(&gfx.device, hwnd, &desc, None, None)?;
            // We handle Alt+Enter ourselves.
            let _ = gfx.factory.MakeWindowAssociation(hwnd, DXGI_MWA_NO_ALT_ENTER);
            let mut s = Surface { swap, rtv: None, width: width.max(1), height: height.max(1) };
            s.make_rtv(gfx)?;
            Ok(s)
        }
    }

    fn make_rtv(&mut self, gfx: &Gfx) -> Result<()> {
        unsafe {
            let back: ID3D11Texture2D = self.swap.GetBuffer(0)?;
            let mut rtv = None;
            gfx.device.CreateRenderTargetView(&back, None, Some(&mut rtv))?;
            self.rtv = rtv;
        }
        Ok(())
    }

    pub fn resize(&mut self, gfx: &Gfx, width: u32, height: u32) {
        let (width, height) = (width.max(1), height.max(1));
        if width == self.width && height == self.height && self.rtv.is_some() {
            return;
        }
        self.rtv = None;
        unsafe {
            gfx.ctx.OMSetRenderTargets(None, None);
            gfx.ctx.Flush();
            if let Err(e) = self.swap.ResizeBuffers(0, width, height, DXGI_FORMAT_UNKNOWN, DXGI_SWAP_CHAIN_FLAG(0)) {
                crate::log!("gfx: resize failed: {e}");
            }
        }
        self.width = width;
        self.height = height;
        let _ = self.make_rtv(gfx);
    }

    pub fn present(&self) {
        unsafe {
            let _ = self.swap.Present(1, DXGI_PRESENT(0));
        }
    }

    pub fn begin(&self, gfx: &Gfx) -> Option<&ID3D11RenderTargetView> {
        let rtv = self.rtv.as_ref()?;
        unsafe {
            gfx.ctx.OMSetRenderTargets(Some(&[Some(rtv.clone())]), None);
            gfx.ctx.RSSetViewports(Some(&[viewport(self.width, self.height)]));
        }
        Some(rtv)
    }
}

pub fn viewport(w: u32, h: u32) -> D3D11_VIEWPORT {
    D3D11_VIEWPORT { TopLeftX: 0.0, TopLeftY: 0.0, Width: w as f32, Height: h as f32, MinDepth: 0.0, MaxDepth: 1.0 }
}
