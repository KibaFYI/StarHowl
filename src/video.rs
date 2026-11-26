//! Video capture through Media Foundation.
//!
//! A background thread opens the capture device, picks a mode, and keeps only the
//! newest decoded frame in a shared slot. The UI thread is poked with a window
//! message whenever a fresh frame is waiting. Old frames are simply replaced, so a
//! slow display never builds up delay.

use crate::config::ModeSel;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;
use windows::core::{Interface, GUID, PWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{CoInitializeEx, CoTaskMemFree, CoUninitialize, COINIT_MULTITHREADED};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

pub const WM_APP_FRAME: u32 = WM_APP + 1;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PixFmt {
    Yuy2,
    Nv12,
    Bgra,
}

impl PixFmt {
    pub fn name(self) -> &'static str {
        match self {
            PixFmt::Yuy2 => "YUY2",
            PixFmt::Nv12 => "NV12",
            PixFmt::Bgra => "RGB32",
        }
    }
}

/// One decoded frame, rows packed tightly (no padding).
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub fmt: PixFmt,
    pub data: Vec<u8>,
    /// The device sends MJPEG (JPEG is full-range BT.601).
    pub src_mjpeg: bool,
    /// Range reported by the decoder, if any: Some(true) = full 0-255.
    pub range_full: Option<bool>,
    /// Matrix reported by the decoder, if any: Some(true) = BT.709.
    pub matrix_709: Option<bool>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Mode {
    pub width: u32,
    pub height: u32,
    pub fps_num: u32,
    pub fps_den: u32,
    pub format: String,
}

impl Mode {
    pub fn fps(&self) -> f32 {
        if self.fps_den == 0 {
            0.0
        } else {
            self.fps_num as f32 / self.fps_den as f32
        }
    }
    pub fn label(&self) -> String {
        format!("{}x{} · {:.0} fps · {}", self.width, self.height, self.fps(), self.format)
    }
    pub fn to_sel(&self) -> ModeSel {
        ModeSel {
            width: self.width,
            height: self.height,
            fps_num: self.fps_num,
            fps_den: self.fps_den,
            format: self.format.clone(),
        }
    }
    fn matches(&self, s: &ModeSel, check_format: bool) -> bool {
        self.width == s.width
            && self.height == s.height
            && (self.fps() - fps_of(s)).abs() < 0.5
            && (!check_format || self.format == s.format)
    }
}

fn fps_of(s: &ModeSel) -> f32 {
    if s.fps_den == 0 {
        0.0
    } else {
        s.fps_num as f32 / s.fps_den as f32
    }
}

#[derive(Clone, Debug)]
pub struct DeviceInfo {
    pub name: String,
    pub link: String,
}

#[derive(Clone, Default)]
pub struct VideoStatus {
    /// Name and device path of the device currently open.
    pub device: String,
    pub link: String,
    pub modes: Vec<Mode>,
    pub active: Option<Mode>,
    pub output: Option<PixFmt>,
    pub state: String,
    pub running: bool,
}

pub struct Shared {
    pub latest: Mutex<Option<Frame>>,
    spare: Mutex<Vec<Vec<u8>>>,
    pub pending: AtomicBool,
    pub status: Mutex<VideoStatus>,
    /// Frames received from the device (for the fps readout).
    pub frames: AtomicU32,
}

impl Shared {
    pub fn new() -> Arc<Shared> {
        Arc::new(Shared {
            latest: Mutex::new(None),
            spare: Mutex::new(Vec::new()),
            pending: AtomicBool::new(false),
            status: Mutex::new(VideoStatus::default()),
            frames: AtomicU32::new(0),
        })
    }

    /// Hand a frame's buffer back so the capture thread can reuse it.
    pub fn recycle(&self, data: Vec<u8>) {
        let mut spare = self.spare.lock().unwrap();
        if spare.len() < 3 {
            spare.push(data);
        }
    }

    fn set_state(&self, s: impl Into<String>, running: bool) {
        let mut st = self.status.lock().unwrap();
        st.state = s.into();
        st.running = running;
    }
}

/// USB vendors that make HDMI capture chips/cards: MacroSilicon (MS2109/MS2130),
/// Elgato, AVerMedia, Blackmagic, Magewell, YUAN, and a common MS2130 clone id.
const CAPTURE_VIDS: &[&str] = &["vid_534d", "vid_345f", "vid_0fd9", "vid_07ca", "vid_1edb", "vid_2935", "vid_1164", "vid_32ed"];
const NAME_HINTS: &[&str] = &["capture", "hdmi", "cam link", "elgato", "avermedia", "ms2109", "ms2130"];

/// How much a device looks like a capture card rather than a webcam.
fn capture_score(d: &DeviceInfo) -> u32 {
    let link = d.link.to_lowercase();
    let name = d.name.to_lowercase();
    if CAPTURE_VIDS.iter().any(|v| link.contains(v)) {
        3
    } else if NAME_HINTS.iter().any(|h| name.contains(h)) {
        2
    } else if name == "usb video" {
        1 // the MS2109's default name
    } else {
        0
    }
}

/// Lists video capture devices. Safe to call while capture is running.
pub fn list_devices() -> Vec<DeviceInfo> {
    unsafe { enum_activates() }
        .into_iter()
        .map(|(_, info)| info)
        .collect()
}

unsafe fn enum_activates() -> Vec<(IMFActivate, DeviceInfo)> {
    let mut out = Vec::new();
    let mut attrs: Option<IMFAttributes> = None;
    if MFCreateAttributes(&mut attrs, 1).is_err() {
        return out;
    }
    let Some(attrs) = attrs else { return out };
    if attrs
        .SetGUID(&MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE, &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID)
        .is_err()
    {
        return out;
    }
    let mut list: *mut Option<IMFActivate> = std::ptr::null_mut();
    let mut count: u32 = 0;
    if MFEnumDeviceSources(&attrs, &mut list, &mut count).is_err() || list.is_null() {
        return out;
    }
    let items = std::slice::from_raw_parts_mut(list, count as usize);
    for item in items.iter_mut() {
        if let Some(act) = item.take() {
            let name = alloc_string(&act, &MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME).unwrap_or_default();
            let link = alloc_string(&act, &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK)
                .unwrap_or_default();
            out.push((act, DeviceInfo { name, link }));
        }
    }
    CoTaskMemFree(Some(list as *const _));
    out
}

unsafe fn alloc_string(act: &IMFActivate, key: &GUID) -> Option<String> {
    let mut p = PWSTR::null();
    let mut len = 0u32;
    act.GetAllocatedString(key, &mut p, &mut len).ok()?;
    let s = p.to_string().ok();
    CoTaskMemFree(Some(p.0 as *const _));
    s
}

/// Picks the device the user asked for, or the most capture-card-looking one.
fn choose_device(
    mut devs: Vec<(IMFActivate, DeviceInfo)>,
    want_name: &str,
    want_link: &str,
) -> Option<(IMFActivate, DeviceInfo)> {
    if !want_link.is_empty() {
        if let Some(i) = devs.iter().position(|(_, d)| d.link.eq_ignore_ascii_case(want_link)) {
            return Some(devs.swap_remove(i));
        }
    }
    if !want_name.is_empty() {
        if let Some(i) = devs.iter().position(|(_, d)| d.name == want_name) {
            return Some(devs.swap_remove(i));
        }
        // The named device is not plugged in: don't silently grab a webcam instead.
        return None;
    }
    // Highest score wins; on a tie the first one Windows listed.
    let best = (0..devs.len()).rev().max_by_key(|&i| capture_score(&devs[i].1))?;
    Some(devs.swap_remove(best))
}

fn subtype_name(g: &GUID) -> String {
    if *g == MFVideoFormat_MJPG {
        "MJPG".into()
    } else if *g == MFVideoFormat_YUY2 {
        "YUY2".into()
    } else if *g == MFVideoFormat_NV12 {
        "NV12".into()
    } else if *g == MFVideoFormat_RGB32 {
        "RGB32".into()
    } else if *g == MFVideoFormat_H264 {
        "H264".into()
    } else {
        let b = g.data1.to_le_bytes();
        if b.iter().all(|c| c.is_ascii_graphic()) {
            String::from_utf8_lossy(&b).into_owned()
        } else {
            format!("{:08X}", g.data1)
        }
    }
}

fn unpack(v: u64) -> (u32, u32) {
    ((v >> 32) as u32, v as u32)
}

/// Best automatic mode: smooth (>= 29 fps) first, then biggest, then fastest,
/// then uncompressed over MJPEG.
fn auto_pick(modes: &[Mode]) -> usize {
    let key = |m: &Mode| {
        (
            m.fps() >= 29.0,
            m.width as u64 * m.height as u64,
            (m.fps().min(240.0) * 10.0) as u32,
            m.format != "MJPG",
        )
    };
    (0..modes.len()).max_by_key(|&i| key(&modes[i])).unwrap_or(0)
}

struct SendSource(IMFMediaSource);
unsafe impl Send for SendSource {}

/// A running capture thread. Call `stop` to end it.
pub struct Capture {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Capture {
    pub fn start(
        want_name: String,
        want_link: String,
        mode: Option<ModeSel>,
        hwnd: HWND,
        shared: Arc<Shared>,
    ) -> Capture {
        let stop = Arc::new(AtomicBool::new(false));
        let hwnd_raw = hwnd.0 as isize;
        let thread = {
            let stop = stop.clone();
            std::thread::Builder::new()
                .name("starhowl-video".into())
                .spawn(move || {
                    let hwnd = HWND(hwnd_raw as *mut _);
                    unsafe {
                        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
                    }
                    let source = Mutex::new(None);
                    run(&want_name, &want_link, mode.as_ref(), hwnd, &shared, &stop, &source);
                    unsafe { CoUninitialize() };
                })
                .ok()
        };
        Capture { stop, thread }
    }

    pub fn stop(mut self) {
        // The capture thread notices the flag after its next frame (a few ms at
        // 30-60 fps) and closes the device itself. Shutting the source down from
        // here instead leaves its blocking read stuck forever.
        self.stop.store(true, Ordering::SeqCst);
        let Some(t) = self.thread.take() else { return };
        let started = std::time::Instant::now();
        while !t.is_finished() && started.elapsed() < Duration::from_secs(2) {
            std::thread::sleep(Duration::from_millis(5));
        }
        if t.is_finished() {
            let _ = t.join();
        } else {
            // No frames are coming (device stalled). Don't freeze the window:
            // let the thread finish on its own whenever the device answers.
            crate::log!("video: capture thread still busy after 2 s, detaching it");
        }
    }
}

fn sleep_unless_stopped(stop: &AtomicBool, ms: u64) {
    let mut left = ms;
    while left > 0 && !stop.load(Ordering::SeqCst) {
        let step = left.min(50);
        std::thread::sleep(Duration::from_millis(step));
        left -= step;
    }
}

fn run(
    want_name: &str,
    want_link: &str,
    mode: Option<&ModeSel>,
    hwnd: HWND,
    shared: &Arc<Shared>,
    stop: &AtomicBool,
    source_slot: &Mutex<Option<SendSource>>,
) {
    shared.set_state("Looking for the capture device…", false);
    let mut announced_missing = false;
    // When another app (Camera, OBS…) already streams from the device, Windows lets
    // us join only if we keep the format that app picked.
    let mut follow_current = false;
    while !stop.load(Ordering::SeqCst) {
        let devs = unsafe { enum_activates() };
        match choose_device(devs, want_name, want_link) {
            None => {
                if !announced_missing {
                    crate::log!("video: no matching capture device (want '{want_name}')");
                    announced_missing = true;
                }
                let msg = if want_name.is_empty() {
                    "No capture device found. Plug it in and it will connect automatically.".to_string()
                } else {
                    format!("\"{want_name}\" is not connected. Waiting for it…")
                };
                shared.set_state(msg, false);
            }
            Some((act, info)) => {
                announced_missing = false;
                {
                    let mut st = shared.status.lock().unwrap();
                    st.device = info.name.clone();
                    st.link = info.link.clone();
                }
                crate::log!("video: opening '{}'{}", info.name, if follow_current { " (sharing)" } else { "" });
                let result = unsafe { stream(&act, mode, follow_current, hwnd, shared, stop, source_slot) };
                let src = source_slot.lock().unwrap().take();
                if let Some(src) = src {
                    unsafe {
                        let _ = src.0.Shutdown();
                    }
                }
                match result {
                    Ok(()) => follow_current = false,
                    Err(e) => {
                        crate::log!("video: {e}");
                        let busy = e.code().0 as u32 == 0xC00D3704 // MF_E_HW_MFT_FAILED_START_STREAMING
                            || e.code().0 as u32 == 0xC00D3EA3 // MF_E_VIDEO_RECORDING_DEVICE_LOCKED
                            || e.code().0 as u32 == 0x800700AA; // ERROR_BUSY
                        if busy && !follow_current && !stop.load(Ordering::SeqCst) {
                            // Try again right away, joining the other app's stream.
                            follow_current = true;
                            continue;
                        }
                        follow_current = false;
                        let msg = if busy {
                            "The capture device is busy. Close the Camera app or OBS if they are using it.".to_string()
                        } else if stop.load(Ordering::SeqCst) {
                            "Stopped".to_string()
                        } else {
                            format!("Capture stopped: {}. Reconnecting…", e.message())
                        };
                        shared.set_state(msg, false);
                    }
                }
            }
        }
        sleep_unless_stopped(stop, 1000);
    }
}

#[allow(clippy::too_many_arguments)]
unsafe fn stream(
    act: &IMFActivate,
    want: Option<&ModeSel>,
    follow_current: bool,
    hwnd: HWND,
    shared: &Arc<Shared>,
    stop: &AtomicBool,
    source_slot: &Mutex<Option<SendSource>>,
) -> windows::core::Result<()> {
    let source: IMFMediaSource = act.ActivateObject()?;
    *source_slot.lock().unwrap() = Some(SendSource(source.clone()));
    if stop.load(Ordering::SeqCst) {
        return Ok(());
    }

    let mut attrs: Option<IMFAttributes> = None;
    MFCreateAttributes(&mut attrs, 2)?;
    let attrs = attrs.unwrap();
    // Lets the reader add the MJPEG decoder and colour converters for us.
    attrs.SetUINT32(&MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, 1)?;
    let reader = MFCreateSourceReaderFromMediaSource(&source, &attrs)?;
    let stream_idx = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;

    let natives = native_modes(&reader, stream_idx);
    if natives.is_empty() {
        return Err(windows::core::Error::new(MF_E_INVALIDMEDIATYPE, "device reported no video modes"));
    }
    let modes: Vec<Mode> = natives.iter().map(|n| n.0.clone()).collect();

    reader.SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false)?;
    reader.SetStreamSelection(stream_idx, true)?;

    let (mode, native_sub) = if follow_current {
        // Keep whatever format the device is already running at.
        let cur = reader.GetNativeMediaType(stream_idx, 0xFFFF_FFFF)?;
        mode_of(&cur).ok_or_else(|| windows::core::Error::new(MF_E_INVALIDMEDIATYPE, "unknown current format"))?
    } else {
        let pick = want
            .and_then(|w| {
                modes
                    .iter()
                    .position(|m| m.matches(w, true))
                    .or_else(|| modes.iter().position(|m| m.matches(w, false)))
            })
            .unwrap_or_else(|| auto_pick(&modes));
        let (mode, native_mt, native_sub) = &natives[pick];
        reader.SetCurrentMediaType(stream_idx, None, native_mt)?;
        (mode.clone(), *native_sub)
    };
    let native_sub = &native_sub;
    crate::log!("video: mode {}", mode.label());

    // Ask for a format the GPU can turn into RGB cheaply.
    let candidates: &[(GUID, PixFmt)] = if *native_sub == MFVideoFormat_NV12 {
        &[(MFVideoFormat_NV12, PixFmt::Nv12), (MFVideoFormat_YUY2, PixFmt::Yuy2), (MFVideoFormat_RGB32, PixFmt::Bgra)]
    } else {
        &[(MFVideoFormat_YUY2, PixFmt::Yuy2), (MFVideoFormat_NV12, PixFmt::Nv12), (MFVideoFormat_RGB32, PixFmt::Bgra)]
    };
    let mut out_fmt = None;
    for (sub, fmt) in candidates {
        let out = MFCreateMediaType()?;
        out.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        out.SetGUID(&MF_MT_SUBTYPE, sub)?;
        if reader.SetCurrentMediaType(stream_idx, None, &out).is_ok() {
            out_fmt = Some(*fmt);
            break;
        }
    }
    let Some(fmt) = out_fmt else {
        return Err(windows::core::Error::new(MF_E_INVALIDMEDIATYPE, "no usable output format"));
    };
    let src_mjpeg = *native_sub == MFVideoFormat_MJPG;

    let mut layout = Layout::read(&reader, stream_idx, fmt)?;
    crate::log!("video: output {} {}x{} stride {}", fmt.name(), layout.w, layout.h, layout.stride);

    {
        let mut st = shared.status.lock().unwrap();
        st.modes = modes.clone();
        st.active = Some(mode.clone());
        st.output = Some(fmt);
        st.state = if follow_current {
            "Sharing the device with another app, so that app picks the mode.".into()
        } else {
            "Running".into()
        };
        st.running = true;
    }

    while !stop.load(Ordering::SeqCst) {
        let mut flags = 0u32;
        let mut sample: Option<IMFSample> = None;
        let mut actual = 0u32;
        let mut ts = 0i64;
        reader.ReadSample(stream_idx, 0, Some(&mut actual), Some(&mut flags), Some(&mut ts), Some(&mut sample))?;
        if flags & MF_SOURCE_READERF_ERROR.0 as u32 != 0 {
            return Err(windows::core::Error::new(MF_E_INVALIDREQUEST, "device reported an error"));
        }
        if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
            return Err(windows::core::Error::new(MF_E_END_OF_STREAM, "device stopped sending video"));
        }
        if flags & MF_SOURCE_READERF_CURRENTMEDIATYPECHANGED.0 as u32 != 0 {
            layout = Layout::read(&reader, stream_idx, fmt)?;
            crate::log!("video: format changed to {}x{}", layout.w, layout.h);
        }
        let Some(sample) = sample else { continue };
        let Ok(buf) = sample.ConvertToContiguousBuffer() else { continue };

        let mut data = shared.spare.lock().unwrap().pop().unwrap_or_default();
        data.clear();
        let copied = copy_frame(&buf, &layout, &mut data);
        if !copied {
            shared.recycle(data);
            continue;
        }
        shared.frames.fetch_add(1, Ordering::Relaxed);
        let frame = Frame {
            width: layout.w,
            height: layout.h,
            fmt,
            data,
            src_mjpeg,
            range_full: layout.range_full,
            matrix_709: layout.matrix_709,
        };
        let old = shared.latest.lock().unwrap().replace(frame);
        if let Some(old) = old {
            shared.recycle(old.data);
        }
        if !shared.pending.swap(true, Ordering::SeqCst) {
            let _ = PostMessageW(Some(hwnd), WM_APP_FRAME, WPARAM(0), LPARAM(0));
        }
    }
    Ok(())
}

unsafe fn mode_of(mt: &IMFMediaType) -> Option<(Mode, GUID)> {
    let sub = mt.GetGUID(&MF_MT_SUBTYPE).ok()?;
    let (w, h) = unpack(mt.GetUINT64(&MF_MT_FRAME_SIZE).ok()?);
    let (num, den) = mt.GetUINT64(&MF_MT_FRAME_RATE).map(unpack).unwrap_or((0, 1));
    Some((Mode { width: w, height: h, fps_num: num, fps_den: den.max(1), format: subtype_name(&sub) }, sub))
}

/// Every mode the device reports, biggest and fastest first.
unsafe fn native_modes(reader: &IMFSourceReader, stream_idx: u32) -> Vec<(Mode, IMFMediaType, GUID)> {
    let mut natives: Vec<(Mode, IMFMediaType, GUID)> = Vec::new();
    let mut i = 0;
    while let Ok(mt) = reader.GetNativeMediaType(stream_idx, i) {
        i += 1;
        let Some((m, sub)) = mode_of(&mt) else { continue };
        if !natives.iter().any(|(n, _, _)| *n == m) {
            natives.push((m, mt, sub));
        }
    }
    natives.sort_by(|a, b| {
        let (a, b) = (&a.0, &b.0);
        (b.width * b.height)
            .cmp(&(a.width * a.height))
            .then(b.fps().partial_cmp(&a.fps()).unwrap_or(std::cmp::Ordering::Equal))
            .then(a.format.cmp(&b.format))
    });
    natives
}

/// Diagnostic dump of every capture device and its modes (`starhowl --probe`).
pub fn probe() -> String {
    let mut out = String::new();
    for (act, info) in unsafe { enum_activates() } {
        let cid = crate::devinfo::container_of_video(&info.link);
        out += &format!("VIDEO \"{}\"\n  link: {}\n  container: {:?}\n", info.name, info.link, cid);
        let modes = unsafe {
            (|| -> windows::core::Result<Vec<Mode>> {
                let source: IMFMediaSource = act.ActivateObject()?;
                let reader = MFCreateSourceReaderFromMediaSource(&source, None)?;
                let m = native_modes(&reader, MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32)
                    .into_iter()
                    .map(|n| n.0)
                    .collect();
                let _ = source.Shutdown();
                Ok(m)
            })()
        };
        match modes {
            Ok(modes) => {
                for m in modes {
                    out += &format!("    {}\n", m.label());
                }
            }
            Err(e) => out += &format!("    (could not open: {e})\n"),
        }
    }
    out
}

struct Layout {
    w: u32,
    h: u32,
    fmt: PixFmt,
    /// Bytes per row of the first plane as the decoder lays it out (may be negative).
    stride: i32,
    range_full: Option<bool>,
    matrix_709: Option<bool>,
}

impl Layout {
    unsafe fn read(reader: &IMFSourceReader, stream_idx: u32, fmt: PixFmt) -> windows::core::Result<Layout> {
        let cur = reader.GetCurrentMediaType(stream_idx)?;
        let (w, h) = unpack(cur.GetUINT64(&MF_MT_FRAME_SIZE)?);
        let row = match fmt {
            PixFmt::Yuy2 => w as i32 * 2,
            PixFmt::Nv12 => w as i32,
            PixFmt::Bgra => w as i32 * 4,
        };
        let stride = cur.GetUINT32(&MF_MT_DEFAULT_STRIDE).map(|s| s as i32).unwrap_or(row);
        let range_full = match cur.GetUINT32(&MF_MT_VIDEO_NOMINAL_RANGE) {
            Ok(1) => Some(true),  // MFNominalRange_0_255
            Ok(2) => Some(false), // MFNominalRange_16_235
            _ => None,
        };
        let matrix_709 = match cur.GetUINT32(&MF_MT_YUV_MATRIX) {
            Ok(1) => Some(true),  // BT.709
            Ok(2) => Some(false), // BT.601
            _ => None,
        };
        Ok(Layout { w, h, fmt, stride, range_full, matrix_709 })
    }

    fn row_bytes(&self) -> usize {
        match self.fmt {
            PixFmt::Yuy2 => self.w as usize * 2,
            PixFmt::Nv12 => self.w as usize,
            PixFmt::Bgra => self.w as usize * 4,
        }
    }
}

/// Copies one frame out of the decoder's buffer into `out`, removing row padding.
unsafe fn copy_frame(buf: &IMFMediaBuffer, l: &Layout, out: &mut Vec<u8>) -> bool {
    let row = l.row_bytes();
    let h = l.h as usize;
    let chroma_rows = if l.fmt == PixFmt::Nv12 { h / 2 } else { 0 };
    out.reserve(row * (h + chroma_rows));

    // Prefer the 2D lock: it tells us the real pitch.
    if let Ok(b2) = buf.cast::<IMF2DBuffer>() {
        let mut scan0: *mut u8 = std::ptr::null_mut();
        let mut pitch: i32 = 0;
        if b2.Lock2D(&mut scan0, &mut pitch).is_ok() {
            copy_planes(scan0, pitch as isize, row, h, chroma_rows, out);
            let _ = b2.Unlock2D();
            return true;
        }
    }

    let mut p: *mut u8 = std::ptr::null_mut();
    let mut len = 0u32;
    if buf.Lock(&mut p, None, Some(&mut len)).is_err() {
        return false;
    }
    let pitch = if l.stride == 0 { row as isize } else { l.stride as isize };
    let need = pitch.unsigned_abs() * (h + chroma_rows);
    let ok = (len as usize) >= need.saturating_sub(pitch.unsigned_abs() - row);
    if ok {
        let scan0 = if pitch < 0 { p.offset((h as isize - 1) * -pitch) } else { p };
        copy_planes(scan0, pitch, row, h, chroma_rows, out);
    }
    let _ = buf.Unlock();
    ok
}

unsafe fn copy_planes(scan0: *const u8, pitch: isize, row: usize, h: usize, chroma_rows: usize, out: &mut Vec<u8>) {
    for r in 0..h {
        let src = scan0.offset(r as isize * pitch);
        out.extend_from_slice(std::slice::from_raw_parts(src, row));
    }
    if chroma_rows > 0 {
        // NV12: interleaved UV plane follows the Y plane.
        let uv0 = scan0.offset(h as isize * pitch);
        for r in 0..chroma_rows {
            let src = uv0.offset(r as isize * pitch);
            out.extend_from_slice(std::slice::from_raw_parts(src, row));
        }
    }
}
