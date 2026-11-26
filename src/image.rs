//! Image loading with Windows Imaging Component (PNG, JPEG, BMP, GIF, TIFF,
//! WebP and more, with no extra libraries) and the "choose a file" dialog.

use windows::core::{w, Interface, Result, HSTRING};
use windows::Win32::Foundation::{GENERIC_READ, HWND};
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::{CoCreateInstance, CoTaskMemFree, CLSCTX_INPROC_SERVER};
use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
use windows::Win32::UI::Shell::{FileOpenDialog, IFileOpenDialog, SIGDN_FILESYSPATH};

/// Straight (unpremultiplied) RGBA8 pixels.
pub struct Rgba {
    pub w: u32,
    pub h: u32,
    pub px: Vec<u8>,
}

fn factory() -> Result<IWICImagingFactory> {
    unsafe { CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER) }
}

/// Loads an image file, shrinking it so its longest side is at most `max_side`.
pub fn load_file(path: &str, max_side: u32) -> Result<Rgba> {
    unsafe {
        let f = factory()?;
        let dec = f.CreateDecoderFromFilename(&HSTRING::from(path), None, GENERIC_READ, WICDecodeMetadataCacheOnDemand)?;
        decode(&f, &dec, max_side)
    }
}

pub fn load_bytes(bytes: &[u8], max_side: u32) -> Result<Rgba> {
    unsafe {
        let f = factory()?;
        let stream = f.CreateStream()?;
        stream.InitializeFromMemory(bytes)?;
        let dec = f.CreateDecoderFromStream(&stream, std::ptr::null(), WICDecodeMetadataCacheOnDemand)?;
        decode(&f, &dec, max_side)
    }
}

unsafe fn decode(f: &IWICImagingFactory, dec: &IWICBitmapDecoder, max_side: u32) -> Result<Rgba> {
    let frame = dec.GetFrame(0)?;
    let (mut w, mut h) = (0u32, 0u32);
    frame.GetSize(&mut w, &mut h)?;
    let mut src: IWICBitmapSource = frame.cast()?;
    let longest = w.max(h);
    if longest > max_side && longest > 0 {
        let nw = ((w as u64 * max_side as u64) / longest as u64).max(1) as u32;
        let nh = ((h as u64 * max_side as u64) / longest as u64).max(1) as u32;
        let scaler = f.CreateBitmapScaler()?;
        scaler.Initialize(&src, nw, nh, WICBitmapInterpolationModeFant)?;
        src = scaler.cast()?;
        w = nw;
        h = nh;
    }
    let conv = f.CreateFormatConverter()?;
    conv.Initialize(&src, &GUID_WICPixelFormat32bppRGBA, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeCustom)?;
    let mut px = vec![0u8; w as usize * h as usize * 4];
    conv.CopyPixels(std::ptr::null(), w * 4, &mut px)?;
    Ok(Rgba { w, h, px })
}

/// Shows the standard Windows "Open" dialog for pictures.
pub fn pick_image(owner: HWND) -> Option<String> {
    unsafe {
        let dlg: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let filters = [
            COMDLG_FILTERSPEC {
                pszName: w!("Images"),
                pszSpec: w!("*.png;*.jpg;*.jpeg;*.bmp;*.gif;*.webp;*.tif;*.tiff;*.jxr;*.heic"),
            },
            COMDLG_FILTERSPEC { pszName: w!("All files"), pszSpec: w!("*.*") },
        ];
        dlg.SetFileTypes(&filters).ok()?;
        let _ = dlg.SetTitle(w!("Choose a background image"));
        dlg.Show(Some(owner)).ok()?; // fails when cancelled
        let item = dlg.GetResult().ok()?;
        let name = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let s = name.to_string().ok();
        CoTaskMemFree(Some(name.0 as *const _));
        s
    }
}
