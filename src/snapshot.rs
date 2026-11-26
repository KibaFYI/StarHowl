//! Saves what a window is showing to a PNG. Used for testing: set the
//! environment variable STARHOWL_SNAPSHOT to a folder and the app writes
//! main.png / settings.png there a few seconds after starting.

use crate::gfx::{Gfx, Surface};
use windows::core::{Result, HSTRING};
use windows::Win32::Foundation::GENERIC_WRITE;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC;
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};

/// Copies the back buffer (call before Present) and writes it as a PNG.
pub fn save(gfx: &Gfx, surface: &Surface, path: &str) -> Result<()> {
    unsafe {
        let back: ID3D11Texture2D = surface.swap.GetBuffer(0)?;
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        back.GetDesc(&mut desc);
        let sdesc = D3D11_TEXTURE2D_DESC {
            Usage: D3D11_USAGE_STAGING,
            BindFlags: 0,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            MiscFlags: 0,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            ..desc
        };
        let mut staging = None;
        gfx.device.CreateTexture2D(&sdesc, None, Some(&mut staging))?;
        let staging = staging.unwrap();
        gfx.ctx.CopyResource(&staging, &back);
        let mut m = D3D11_MAPPED_SUBRESOURCE::default();
        gfx.ctx.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut m))?;
        let (w, h) = (desc.Width, desc.Height);
        let mut px = vec![0u8; (w * h * 4) as usize];
        for y in 0..h as usize {
            let src = (m.pData as *const u8).add(y * m.RowPitch as usize);
            std::ptr::copy_nonoverlapping(src, px.as_mut_ptr().add(y * w as usize * 4), w as usize * 4);
        }
        gfx.ctx.Unmap(&staging, 0);
        for p in px.chunks_exact_mut(4) {
            p[3] = 255;
        }

        let f: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
        let stream = f.CreateStream()?;
        stream.InitializeFromFilename(&HSTRING::from(path), GENERIC_WRITE.0)?;
        let enc = f.CreateEncoder(&GUID_ContainerFormatPng, std::ptr::null())?;
        enc.Initialize(&stream, WICBitmapEncoderNoCache)?;
        let mut frame = None;
        enc.CreateNewFrame(&mut frame, std::ptr::null_mut())?;
        let frame = frame.unwrap();
        frame.Initialize(None)?;
        frame.SetSize(w, h)?;
        let mut fmt = GUID_WICPixelFormat32bppBGRA;
        frame.SetPixelFormat(&mut fmt)?;
        frame.WritePixels(h, w * 4, &px)?;
        frame.Commit()?;
        enc.Commit()?;
    }
    Ok(())
}
