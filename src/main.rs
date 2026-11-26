//! Starhowl: a tiny, low-latency viewer for USB capture cards.

// No console window in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod audio;
mod config;
mod devinfo;
mod discord;
mod egui_win;
mod gfx;
mod image;
mod logging;
mod render;
mod snapshot;
mod ui;
mod video;

use windows::core::{w, HSTRING};
use windows::Win32::Media::MediaFoundation::{MFShutdown, MFStartup, MFSTARTUP_FULL};
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};
use windows::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};

/// MF_SDK_VERSION << 16 | MF_API_VERSION
const MF_VERSION: u32 = 0x0002_0070;

fn main() {
    logging::init();
    log!("Starhowl {} starting", env!("CARGO_PKG_VERSION"));
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        if let Err(e) = MFStartup(MF_VERSION, MFSTARTUP_FULL) {
            log!("MFStartup failed: {e}");
        }
    }
    if std::env::args().any(|a| a == "--probe") {
        // Diagnostics: list devices, modes and audio endpoints into the log.
        let mut report = video::probe();
        for (kind, capture) in [("AUDIO IN", true), ("AUDIO OUT", false)] {
            for e in audio::list_endpoints(capture) {
                let fmt = audio::mix_format(&e.id)
                    .map(|(ch, hz)| format!("{ch} ch, {hz} Hz"))
                    .unwrap_or_else(|| "?".into());
                report += &format!("{kind} \"{}\"\n  id: {}\n  container: {:?}\n  format: {fmt}\n", e.name, e.id, e.container);
            }
        }
        log!("probe:\n{report}");
        println!("{report}");
        return;
    }
    if let Some(path) = std::env::args().skip_while(|a| a != "--record-audio").nth(1) {
        // Diagnostics: 4 s of the capture card's audio, untouched, as a WAV.
        let inputs = audio::list_endpoints(true);
        let link = video::list_devices()
            .into_iter()
            .map(|d| d.link)
            .find(|l| l.to_lowercase().contains("vid_534d"))
            .unwrap_or_default();
        match audio::match_input(&link, &inputs) {
            Some(e) => match audio::record_raw(&e.id, 4, &path) {
                Ok((ch, hz)) => log!("recorded '{}' ({ch} ch, {hz} Hz) to {path}", e.name),
                Err(err) => log!("record failed: {err}"),
            },
            None => log!("record: no capture audio input found"),
        }
        return;
    }
    let cfg = config::Config::load();
    if let Err(e) = app::run(cfg) {
        log!("fatal: {e}");
        unsafe {
            MessageBoxW(
                None,
                &HSTRING::from(format!("Starhowl couldn't start:\n\n{}", e.message())),
                w!("Starhowl"),
                MB_ICONERROR | MB_OK,
            );
        }
    }
    unsafe {
        let _ = MFShutdown();
        CoUninitialize();
    }
    log!("bye");
}
