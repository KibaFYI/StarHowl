//! Audio passthrough: capture-card input -> speakers, through WASAPI shared mode.
//!
//! Both sides run as 48 kHz stereo float and Windows resamples if a device uses
//! something else. A small jitter buffer (about 30 ms) sits in between, plus an
//! optional user delay for lining audio up with the picture.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;
use windows::core::{GUID, HSTRING, PWSTR};
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
use windows::Win32::Media::Audio::*;
use windows::Win32::System::Com::StructuredStorage::{PropVariantToGUID, PropVariantToStringAlloc};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL, COINIT_MULTITHREADED, STGM_READ,
};
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};

const RATE: u32 = 48_000;
/// Audio the output device should always have queued, in frames (30 ms).
const TARGET: usize = 1_440;
const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;

#[derive(Clone, Debug)]
pub struct Endpoint {
    pub id: String,
    pub name: String,
    pub container: Option<GUID>,
}

pub struct Shared {
    /// f32 bits, 0.0 ..= 1.0
    pub volume: AtomicU32,
    pub muted: AtomicBool,
    pub delay_ms: AtomicU32,
    /// f32 bits: recent input peak, for the level meter.
    pub peak: AtomicU32,
    pub status: Mutex<String>,
}

impl Shared {
    pub fn new() -> Arc<Shared> {
        Arc::new(Shared {
            volume: AtomicU32::new(1.0f32.to_bits()),
            muted: AtomicBool::new(false),
            delay_ms: AtomicU32::new(0),
            peak: AtomicU32::new(0),
            status: Mutex::new(String::new()),
        })
    }
    pub fn set_volume(&self, v: f32) {
        self.volume.store(v.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }
    pub fn peak(&self) -> f32 {
        f32::from_bits(self.peak.load(Ordering::Relaxed))
    }
    pub fn set_status(&self, s: impl Into<String>) {
        *self.status.lock().unwrap() = s.into();
    }
}

/// The container id groups every function of one physical device (the capture
/// card's video and its audio share one).
fn container_key() -> windows::Win32::Foundation::PROPERTYKEY {
    let mut k = PKEY_Device_FriendlyName;
    k.fmtid = GUID::from_u128(0x8c7ed206_3f8a_4827_b3ab_ae9e1faefc6c);
    k.pid = 2;
    k
}

pub fn list_endpoints(capture: bool) -> Vec<Endpoint> {
    unsafe { list_endpoints_inner(capture) }.unwrap_or_default()
}

unsafe fn list_endpoints_inner(capture: bool) -> windows::core::Result<Vec<Endpoint>> {
    let en: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
    let flow = if capture { eCapture } else { eRender };
    let col = en.EnumAudioEndpoints(flow, DEVICE_STATE_ACTIVE)?;
    let mut out = Vec::new();
    for i in 0..col.GetCount()? {
        let Ok(dev) = col.Item(i) else { continue };
        let Ok(idp) = dev.GetId() else { continue };
        let id = idp.to_string().unwrap_or_default();
        CoTaskMemFree(Some(idp.0 as *const _));
        let mut name = id.clone();
        let mut container = None;
        if let Ok(store) = dev.OpenPropertyStore(STGM_READ) {
            if let Ok(pv) = store.GetValue(&PKEY_Device_FriendlyName) {
                if let Ok(s) = PropVariantToStringAlloc(&pv) {
                    name = pwstr_take(s);
                }
            }
            if let Ok(pv) = store.GetValue(&container_key()) {
                container = PropVariantToGUID(&pv).ok();
            }
        }
        out.push(Endpoint { id, name, container });
    }
    out.sort_by_key(|e| e.name.to_lowercase());
    Ok(out)
}

/// The format Windows runs an endpoint at (channels, sample rate), for diagnostics.
pub fn mix_format(id: &str) -> Option<(u16, u32)> {
    unsafe {
        let en: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
        let dev = en.GetDevice(&HSTRING::from(id)).ok()?;
        let client: IAudioClient = dev.Activate(CLSCTX_ALL, None).ok()?;
        let f = client.GetMixFormat().ok()?;
        let r = ((*f).nChannels, (*f).nSamplesPerSec);
        CoTaskMemFree(Some(f as *const _));
        Some(r)
    }
}

/// Diagnostics (`--record-audio`): saves `secs` seconds of an input exactly as
/// Windows delivers it (its own format, 32-bit float) to a WAV file.
pub fn record_raw(id: &str, secs: u32, path: &str) -> windows::core::Result<(u16, u32)> {
    unsafe {
        let en: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let dev = en.GetDevice(&HSTRING::from(id))?;
        let client: IAudioClient = dev.Activate(CLSCTX_ALL, None)?;
        let fmt = client.GetMixFormat()?;
        let (ch, hz) = ((*fmt).nChannels, (*fmt).nSamplesPerSec);
        client.Initialize(AUDCLNT_SHAREMODE_SHARED, 0, 2_000_000, 0, fmt, None)?;
        CoTaskMemFree(Some(fmt as *const _));
        let cap: IAudioCaptureClient = client.GetService()?;
        client.Start()?;
        let want = (secs * hz * ch as u32) as usize;
        let mut samples: Vec<f32> = Vec::with_capacity(want);
        while samples.len() < want {
            std::thread::sleep(Duration::from_millis(10));
            while cap.GetNextPacketSize()? > 0 {
                let mut data: *mut u8 = std::ptr::null_mut();
                let (mut frames, mut flags) = (0u32, 0u32);
                cap.GetBuffer(&mut data, &mut frames, &mut flags, None, None)?;
                let n = frames as usize * ch as usize;
                if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 || data.is_null() {
                    samples.extend(std::iter::repeat_n(0.0, n));
                } else {
                    samples.extend_from_slice(std::slice::from_raw_parts(data as *const f32, n));
                }
                cap.ReleaseBuffer(frames)?;
            }
        }
        let _ = client.Stop();
        let mut wav = Vec::with_capacity(44 + samples.len() * 4);
        let data_len = (samples.len() * 4) as u32;
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + data_len).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&3u16.to_le_bytes()); // IEEE float
        wav.extend_from_slice(&ch.to_le_bytes());
        wav.extend_from_slice(&hz.to_le_bytes());
        wav.extend_from_slice(&(hz * 4 * ch as u32).to_le_bytes());
        wav.extend_from_slice(&(4 * ch).to_le_bytes());
        wav.extend_from_slice(&32u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data_len.to_le_bytes());
        for s in &samples {
            wav.extend_from_slice(&s.to_le_bytes());
        }
        let _ = std::fs::write(path, wav);
        Ok((ch, hz))
    }
}

unsafe fn pwstr_take(p: PWSTR) -> String {
    let s = p.to_string().unwrap_or_default();
    CoTaskMemFree(Some(p.0 as *const _));
    s
}

const AUDIO_HINTS: &[&str] = &[
    "usb digital audio",
    "digital audio interface",
    "capture",
    "hdmi",
    "cam link",
    "elgato",
    "avermedia",
    "usb video",
    "ms2109",
];

/// Finds the audio input that belongs to the given video device.
pub fn match_input(video_link: &str, inputs: &[Endpoint]) -> Option<Endpoint> {
    if let Some(cid) = crate::devinfo::container_of_video(video_link) {
        if let Some(e) = inputs.iter().find(|e| e.container == Some(cid)) {
            return Some(e.clone());
        }
    }
    for hint in AUDIO_HINTS {
        if let Some(e) = inputs.iter().find(|e| e.name.to_lowercase().contains(hint)) {
            return Some(e.clone());
        }
    }
    None
}

pub struct Audio {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    pub input_id: String,
}

impl Audio {
    pub fn start(input_id: String, output_id: String, ms2109_fix: bool, shared: Arc<Shared>) -> Audio {
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let stop = stop.clone();
            let input_id = input_id.clone();
            std::thread::Builder::new()
                .name("starhowl-audio".into())
                .spawn(move || {
                    unsafe {
                        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
                    }
                    while !stop.load(Ordering::SeqCst) {
                        match unsafe { passthrough(&input_id, &output_id, ms2109_fix, &shared, &stop) } {
                            Ok(()) => break,
                            Err(e) => {
                                crate::log!("audio: {e}");
                                shared.set_status(format!("Audio stopped: {}. Retrying…", e.message()));
                                shared.peak.store(0, Ordering::Relaxed);
                                for _ in 0..20 {
                                    if stop.load(Ordering::SeqCst) {
                                        break;
                                    }
                                    std::thread::sleep(Duration::from_millis(50));
                                }
                            }
                        }
                    }
                    unsafe { CoUninitialize() };
                })
                .ok()
        };
        Audio { stop, thread, input_id }
    }

    pub fn stop(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn float_format(channels: u16, rate: u32) -> WAVEFORMATEX {
    WAVEFORMATEX {
        wFormatTag: WAVE_FORMAT_IEEE_FLOAT,
        nChannels: channels,
        nSamplesPerSec: rate,
        nAvgBytesPerSec: rate * 4 * channels as u32,
        nBlockAlign: 4 * channels,
        wBitsPerSample: 32,
        cbSize: 0,
    }
}

unsafe fn device_name(dev: &IMMDevice) -> String {
    dev.OpenPropertyStore(STGM_READ)
        .and_then(|s| s.GetValue(&PKEY_Device_FriendlyName))
        .and_then(|pv| PropVariantToStringAlloc(&pv))
        .map(|p| pwstr_take(p))
        .unwrap_or_else(|_| "audio device".into())
}

unsafe fn passthrough(
    input_id: &str,
    output_id: &str,
    ms2109_fix: bool,
    shared: &Shared,
    stop: &AtomicBool,
) -> windows::core::Result<()> {
    let en: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
    let in_dev = en.GetDevice(&HSTRING::from(input_id))?;
    let out_dev = if output_id.is_empty() {
        en.GetDefaultAudioEndpoint(eRender, eConsole)?
    } else {
        en.GetDevice(&HSTRING::from(output_id))?
    };
    let in_name = device_name(&in_dev);
    let out_name = device_name(&out_dev);

    let convert = AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY;

    // Input. MS2109 dongles send stereo 48 kHz but tell Windows it is mono 96 kHz.
    // When an input reports exactly that, read it raw and split the samples back
    // into left/right pairs.
    let in_client: IAudioClient = in_dev.Activate(CLSCTX_ALL, None)?;
    let reported = in_client.GetMixFormat().ok().map(|f| {
        let r = ((*f).nChannels, (*f).nSamplesPerSec);
        CoTaskMemFree(Some(f as *const _));
        r
    });
    let ms2109_fix = ms2109_fix && reported == Some((1, RATE * 2));
    let in_fmt = if ms2109_fix { float_format(1, RATE * 2) } else { float_format(2, RATE) };
    in_client.Initialize(
        AUDCLNT_SHAREMODE_SHARED,
        AUDCLNT_STREAMFLAGS_EVENTCALLBACK | convert,
        200_000, // 20 ms
        0,
        &in_fmt,
        None,
    )?;
    let event = CreateEventW(None, false, false, None)?;
    let result = (|| -> windows::core::Result<()> {
        in_client.SetEventHandle(event)?;
        let capture: IAudioCaptureClient = in_client.GetService()?;

        // Output.
        let out_fmt = float_format(2, RATE);
        let out_client: IAudioClient = out_dev.Activate(CLSCTX_ALL, None)?;
        out_client.Initialize(AUDCLNT_SHAREMODE_SHARED, convert, 1_000_000, 0, &out_fmt, None)?;
        let out_size = out_client.GetBufferSize()? as usize;
        let render: IAudioRenderClient = out_client.GetService()?;

        // Start with a cushion of silence so small timing wobbles never click.
        let prefill = TARGET.min(out_size);
        render.GetBuffer(prefill as u32)?;
        render.ReleaseBuffer(prefill as u32, AUDCLNT_BUFFERFLAGS_SILENT.0 as u32)?;

        in_client.Start()?;
        out_client.Start()?;
        let fix_note = if ms2109_fix { "  (MS2109 stereo fix on)" } else { "" };
        shared.set_status(format!("{in_name}  ->  {out_name}{fix_note}"));
        crate::log!("audio: {in_name} -> {out_name} (fix={ms2109_fix})");

        let mut ring: VecDeque<f32> = VecDeque::with_capacity(RATE as usize * 2);
        let mut gain = 0.0f32;
        let mut peak = 0.0f32;

        while !stop.load(Ordering::SeqCst) {
            if WaitForSingleObject(event, 200) != WAIT_OBJECT_0 {
                continue;
            }

            // 1. Pull everything the input has.
            loop {
                let n = capture.GetNextPacketSize()?;
                if n == 0 {
                    break;
                }
                let mut data: *mut u8 = std::ptr::null_mut();
                let mut frames = 0u32;
                let mut flags = 0u32;
                capture.GetBuffer(&mut data, &mut frames, &mut flags, None, None)?;
                let samples = frames as usize * in_fmt.nChannels as usize;
                if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 || data.is_null() {
                    ring.extend(std::iter::repeat_n(0.0, samples));
                } else {
                    let s = std::slice::from_raw_parts(data as *const f32, samples);
                    // Both layouts end up as interleaved L,R pairs.
                    ring.extend(s.iter().copied());
                    for v in s {
                        peak = peak.max(v.abs());
                    }
                }
                capture.ReleaseBuffer(frames)?;
            }
            // Keep the ring an even number of samples (L,R pairs).
            if ring.len() % 2 == 1 {
                ring.pop_back();
            }
            peak *= 0.92;
            shared.peak.store(peak.to_bits(), Ordering::Relaxed);

            // 2. Hold back the user's delay; trim if we've drifted too far ahead.
            let delay = shared.delay_ms.load(Ordering::Relaxed) as usize * RATE as usize / 1000;
            let have = ring.len() / 2;
            let mut playable = have.saturating_sub(delay);
            if playable > TARGET * 3 {
                let drop = playable - TARGET;
                ring.drain(..drop * 2);
                playable = TARGET;
            }

            // 3. Top the output up to about two targets' worth.
            let padding = out_client.GetCurrentPadding()? as usize;
            let room = (TARGET * 2).saturating_sub(padding).min(out_size - padding);
            let n = playable.min(room);
            if n == 0 {
                continue;
            }
            let want = if shared.muted.load(Ordering::Relaxed) {
                0.0
            } else {
                let v = f32::from_bits(shared.volume.load(Ordering::Relaxed));
                v * v // feels more even than a straight line
            };
            let p = render.GetBuffer(n as u32)?;
            let out = std::slice::from_raw_parts_mut(p as *mut f32, n * 2);
            let step = (want - gain) / n as f32;
            for frame in out.chunks_exact_mut(2) {
                gain += step;
                frame[0] = ring.pop_front().unwrap_or(0.0) * gain;
                frame[1] = ring.pop_front().unwrap_or(0.0) * gain;
            }
            gain = want;
            render.ReleaseBuffer(n as u32, 0)?;
        }
        let _ = in_client.Stop();
        let _ = out_client.Stop();
        Ok(())
    })();
    let _ = CloseHandle(event);
    if result.is_ok() {
        shared.set_status("Stopped");
    }
    result
}
