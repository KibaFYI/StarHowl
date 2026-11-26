//! The two windows (video + settings popup), their input, and the glue between
//! capture, audio, rendering and settings.

use crate::audio::{self, Audio};
use crate::discord::{self, Discord};
use crate::config::*;
use crate::egui_win::{self, EguiWindow};
use crate::gfx::{Gfx, Surface};
use crate::render::{Look, VideoRenderer};
use crate::ui::{self, Action, HotkeyTarget};
use crate::video::{self, Capture, WM_APP_FRAME};
use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use windows::core::{w, Result, HSTRING, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWINDOWATTRIBUTE};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForWindow};
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

const MAIN_CLASS: PCWSTR = w!("StarhowlMain");
const SETTINGS_CLASS: PCWSTR = w!("StarhowlSettings");

const TIMER_TICK: usize = 1;
const TIMER_ANIM: usize = 2;
const TIMER_UI: usize = 3;
const TIMER_UI_SOON: usize = 4;

const WM_APP_PICK_IMAGE: u32 = WM_APP + 2;
const IDM_SETTINGS: usize = 0x1010;
const IDM_FULLSCREEN: usize = 0x1020;

const MIN_W: i32 = 160;
const MIN_H: i32 = 90;
const COMMON_SIZES: &[(i32, i32)] =
    &[(1280, 720), (1600, 900), (1920, 1080), (2560, 1440), (3840, 2160), (960, 540), (640, 360)];

static APP: AtomicPtr<App> = AtomicPtr::new(std::ptr::null_mut());

fn app() -> Option<&'static mut App> {
    unsafe { APP.load(Ordering::Relaxed).as_mut() }
}

fn loword(v: isize) -> i32 {
    (v & 0xFFFF) as u16 as i32
}
fn hiword(v: isize) -> i32 {
    ((v >> 16) & 0xFFFF) as u16 as i32
}

fn key_down(vk: VIRTUAL_KEY) -> bool {
    unsafe { GetKeyState(vk.0 as i32) < 0 }
}

/// Ctrl+Shift+Alt held: the free-resize gesture.
fn resize_mods() -> bool {
    key_down(VK_CONTROL) && key_down(VK_SHIFT) && key_down(VK_MENU)
}

fn is_modifier(vk: u16) -> bool {
    matches!(vk, 0x10 | 0x11 | 0x12 | 0xA0..=0xA5 | 0x5B | 0x5C)
}

#[derive(Clone, Copy)]
struct Rect {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

struct ResizeDrag {
    ex: i32,
    ey: i32,
    start_pt: POINT,
    start: RECT,
    /// Window frame thickness around the client area: left, top, right, bottom.
    inset: [i32; 4],
}

pub struct App {
    main: HWND,
    settings: HWND,
    gfx: Gfx,
    main_surface: Surface,
    settings_surface: Option<Surface>,
    renderer: VideoRenderer,
    overlay: EguiWindow,
    ui: EguiWindow,
    logo: Option<crate::image::Rgba>,
    overlay_logo: Option<egui::TextureHandle>,
    ui_logo: Option<egui::TextureHandle>,

    cfg: Config,
    dirty_since: Option<Instant>,
    /// Testing aid: folder to drop main.png / settings.png into (STARHOWL_SNAPSHOT).
    snap_dir: Option<String>,
    snapped: [bool; 2],

    discord: Option<Discord>,
    /// Unix time Starhowl started: the presence's "elapsed" timer.
    discord_start: u64,
    discord_img: Option<crate::image::Rgba>,
    discord_art: Option<egui::TextureHandle>,
    discord_badge_img: Option<crate::image::Rgba>,
    discord_badge: Option<egui::TextureHandle>,

    vshared: Arc<video::Shared>,
    capture: Option<Capture>,
    ashared: Arc<audio::Shared>,
    audio: Option<Audio>,
    audio_link: String,
    resolved_input: Option<audio::Endpoint>,

    devices: Vec<video::DeviceInfo>,
    inputs: Vec<audio::Endpoint>,
    outputs: Vec<audio::Endpoint>,

    last_frame: Option<Instant>,
    fps: f32,
    fps_at: Instant,
    fullscreen: Option<Rect>,
    drag: Option<ResizeDrag>,
    readout_until: Option<Instant>,
    toast: Option<(String, Instant)>,
    cursor_hidden: bool,
    last_mouse: (Instant, POINT),
    capturing: Option<HotkeyTarget>,
    start: Instant,
    anim_on: bool,
    needs_render: bool,
    settings_placed: bool,
}

pub fn run(cfg: Config) -> Result<()> {
    unsafe {
        // One copy at a time: two would fight over the capture card.
        let _mutex = CreateMutexW(None, true, w!("Local\\Starhowl.SingleInstance"))?;
        if GetLastError() == ERROR_ALREADY_EXISTS {
            if let Ok(other) = FindWindowW(MAIN_CLASS, PCWSTR::null()) {
                let _ = ShowWindow(other, SW_RESTORE);
                let _ = SetForegroundWindow(other);
            }
            return Ok(());
        }

        let hinst: HINSTANCE = GetModuleHandleW(None)?.into();
        let icon_big = LoadImageW(Some(hinst), PCWSTR(1 as _), IMAGE_ICON, 0, 0, LR_DEFAULTSIZE)
            .map(|h| HICON(h.0))
            .unwrap_or_default();
        let icon_small = LoadImageW(
            Some(hinst),
            PCWSTR(1 as _),
            IMAGE_ICON,
            GetSystemMetrics(SM_CXSMICON),
            GetSystemMetrics(SM_CYSMICON),
            LR_DEFAULTCOLOR,
        )
        .map(|h| HICON(h.0))
        .unwrap_or_default();
        let arrow = LoadCursorW(None, IDC_ARROW)?;
        for (class, proc_) in [
            (MAIN_CLASS, main_proc as unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT),
            (SETTINGS_CLASS, settings_proc),
        ] {
            let wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                style: CS_DBLCLKS,
                lpfnWndProc: Some(proc_),
                hInstance: hinst,
                hIcon: icon_big,
                hIconSm: icon_small,
                hCursor: arrow,
                lpszClassName: class,
                ..Default::default()
            };
            RegisterClassExW(&wc);
        }

        let start_rect = initial_rect(&cfg);
        let main = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            MAIN_CLASS,
            w!("Starhowl"),
            WS_OVERLAPPEDWINDOW,
            start_rect.x,
            start_rect.y,
            start_rect.w,
            start_rect.h,
            None,
            None,
            Some(hinst),
            None,
        )?;
        let settings = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            SETTINGS_CLASS,
            w!("Starhowl Settings"),
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_THICKFRAME | WS_MINIMIZEBOX,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            480,
            800,
            Some(main),
            None,
            Some(hinst),
            None,
        )?;
        dark_title_bar(main);
        dark_title_bar(settings);

        let sys = GetSystemMenu(main, false);
        let _ = AppendMenuW(sys, MF_SEPARATOR, 0, None);
        let _ = AppendMenuW(sys, MF_STRING, IDM_SETTINGS, w!("Starhowl settings"));
        let _ = AppendMenuW(sys, MF_STRING, IDM_FULLSCREEN, w!("Toggle fullscreen"));

        let gfx = Gfx::new()?;
        let (cw, ch) = client_size(main);
        let main_surface = Surface::new(&gfx, main, cw, ch)?;
        let renderer = VideoRenderer::new(&gfx)?;
        let overlay = EguiWindow::new(&gfx)?;
        let ui_win = EguiWindow::new(&gfx)?;
        ui::apply_theme(&overlay.ctx);
        ui::apply_theme(&ui_win.ctx);
        let logo = crate::image::load_bytes(include_bytes!("../assets/logo.png"), 256).ok();

        let now = Instant::now();
        let boxed = Box::new(App {
            main,
            settings,
            gfx,
            main_surface,
            settings_surface: None,
            renderer,
            overlay,
            ui: ui_win,
            logo,
            overlay_logo: None,
            ui_logo: None,
            cfg,
            dirty_since: None,
            snap_dir: std::env::var("STARHOWL_SNAPSHOT").ok(),
            snapped: [false; 2],
            discord: None,
            discord_start: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            discord_img: crate::image::load_bytes(include_bytes!("../assets/discord-large.png"), 256).ok(),
            discord_art: None,
            discord_badge_img: crate::image::load_bytes(include_bytes!("../assets/discord-small.png"), 128).ok(),
            discord_badge: None,
            vshared: video::Shared::new(),
            capture: None,
            ashared: audio::Shared::new(),
            audio: None,
            audio_link: String::new(),
            resolved_input: None,
            devices: Vec::new(),
            inputs: Vec::new(),
            outputs: Vec::new(),
            last_frame: None,
            fps: 0.0,
            fps_at: now,
            fullscreen: None,
            drag: None,
            readout_until: None,
            toast: None,
            cursor_hidden: false,
            last_mouse: (now, POINT::default()),
            capturing: None,
            start: now,
            anim_on: false,
            needs_render: true,
            settings_placed: false,
        });
        APP.store(Box::into_raw(boxed), Ordering::Relaxed);
        let a = app().unwrap();

        a.overlay.ppp = GetDpiForWindow(main) as f32 / 96.0;
        a.apply_style();
        a.set_client_rect(start_rect);
        a.apply_topmost();
        a.sync_audio_controls();
        a.load_image();
        a.discord = Some(Discord::start());
        a.update_discord();
        let hint = format!("{} opens settings", a.cfg.key_settings);
        a.show_toast(&hint, 4.0);

        let _ = ShowWindow(main, SW_SHOW);
        a.restart_capture();
        a.restart_audio();
        SetTimer(Some(main), TIMER_TICK, 250, None);
        a.render_main();
        if std::env::var("STARHOWL_OPEN_SETTINGS").is_ok() {
            a.show_settings();
        }

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        crate::log!("message loop ended");

        let app = Box::from_raw(APP.swap(std::ptr::null_mut(), Ordering::Relaxed));
        app.shutdown();
    }
    Ok(())
}

unsafe fn dark_title_bar(hwnd: HWND) {
    let on: i32 = 1;
    let _ = DwmSetWindowAttribute(hwnd, DWMWINDOWATTRIBUTE(20), &on as *const _ as *const _, 4);
    let caption: u32 = 0x0020_0A12; // BGR of #120A20
    let _ = DwmSetWindowAttribute(hwnd, DWMWINDOWATTRIBUTE(35), &caption as *const _ as *const _, 4);
}

unsafe fn client_size(hwnd: HWND) -> (u32, u32) {
    let mut r = RECT::default();
    let _ = GetClientRect(hwnd, &mut r);
    ((r.right - r.left).max(1) as u32, (r.bottom - r.top).max(1) as u32)
}

unsafe fn work_area_near(r: Rect) -> RECT {
    let mon = MonitorFromRect(
        &RECT { left: r.x, top: r.y, right: r.x + r.w, bottom: r.y + r.h },
        MONITOR_DEFAULTTOPRIMARY,
    );
    let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
    let _ = GetMonitorInfoW(mon, &mut mi);
    mi.rcWork
}

/// Last saved position if it's still on a screen, else 1280x720 centred.
unsafe fn initial_rect(cfg: &Config) -> Rect {
    if let Some([x, y, w, h]) = cfg.window {
        let r = RECT { left: x, top: y, right: x + w, bottom: y + h };
        if w >= MIN_W && h >= MIN_H && !MonitorFromRect(&r, MONITOR_DEFAULTTONULL).is_invalid() {
            return Rect { x, y, w, h };
        }
    }
    let work = work_area_near(Rect { x: 0, y: 0, w: 1, h: 1 });
    let (aw, ah) = (work.right - work.left, work.bottom - work.top);
    let (mut w, mut h) = (1280, 720);
    if w > aw * 9 / 10 || h > ah * 9 / 10 {
        let s = ((aw as f32 * 0.8) / w as f32).min((ah as f32 * 0.8) / h as f32);
        w = (w as f32 * s) as i32;
        h = (h as f32 * s) as i32;
    }
    Rect { x: work.left + (aw - w) / 2, y: work.top + (ah - h) / 2, w, h }
}

/// Where the video sits inside a window of `ww` x `wh` pixels.
fn video_rect(s: Scaling, fw: u32, fh: u32, ww: u32, wh: u32) -> [f32; 4] {
    let (fw, fh, ww, wh) = (fw.max(1) as f32, fh.max(1) as f32, ww as f32, wh as f32);
    let k = (ww / fw).min(wh / fh);
    let scale = match s {
        Scaling::Fit => k,
        Scaling::Integer => {
            if k >= 1.0 {
                k.floor()
            } else {
                1.0 / (1.0 / k).ceil()
            }
        }
        Scaling::Native => 1.0,
        Scaling::Half => 0.5,
        Scaling::Third => 1.0 / 3.0,
        Scaling::Double => 2.0,
        Scaling::Stretch => return [0.0, 0.0, ww, wh],
    };
    let (vw, vh) = ((fw * scale).round(), (fh * scale).round());
    let x0 = ((ww - vw) / 2.0).floor();
    let y0 = ((wh - vh) / 2.0).floor();
    [x0, y0, x0 + vw, y0 + vh]
}

fn image_rect(fit: ImageFit, img: Option<(u32, u32)>, ww: u32, wh: u32) -> [f32; 4] {
    let (ww, wh) = (ww as f32, wh as f32);
    let Some((iw, ih)) = img else { return [0.0, 0.0, ww, wh] };
    let (iw, ih) = (iw.max(1) as f32, ih.max(1) as f32);
    let s = match fit {
        ImageFit::Stretch => return [0.0, 0.0, ww, wh],
        ImageFit::Cover => (ww / iw).max(wh / ih),
        ImageFit::Center => 1.0,
    };
    let (w, h) = (iw * s, ih * s);
    let x0 = ((ww - w) / 2.0).floor();
    let y0 = ((wh - h) / 2.0).floor();
    [x0, y0, x0 + w, y0 + h]
}

fn fill_id(f: Fill) -> u32 {
    match f {
        Fill::Black => 0,
        Fill::White => 1,
        Fill::Galaxy => 2,
        Fill::Image => 3,
    }
}

impl App {
    // ------------------------------------------------------------------ window geometry

    fn style_now(&self) -> WINDOW_STYLE {
        if self.fullscreen.is_some() || self.cfg.borderless {
            WS_POPUP | WS_SYSMENU | WS_MINIMIZEBOX
        } else {
            WS_OVERLAPPEDWINDOW
        }
    }

    /// The client area in screen coordinates.
    fn client_rect(&self) -> Rect {
        unsafe {
            let mut r = RECT::default();
            let _ = GetClientRect(self.main, &mut r);
            let mut p = POINT { x: 0, y: 0 };
            let _ = ClientToScreen(self.main, &mut p);
            Rect { x: p.x, y: p.y, w: r.right - r.left, h: r.bottom - r.top }
        }
    }

    /// Moves/resizes the window so its client area is exactly `r`.
    fn set_client_rect(&mut self, r: Rect) {
        unsafe {
            let style = self.style_now();
            let mut wr = RECT { left: r.x, top: r.y, right: r.x + r.w, bottom: r.y + r.h };
            if style != (WS_POPUP | WS_SYSMENU | WS_MINIMIZEBOX) {
                let _ = AdjustWindowRectExForDpi(&mut wr, style, false, WINDOW_EX_STYLE(0), GetDpiForWindow(self.main));
            }
            let _ = SetWindowPos(
                self.main,
                None,
                wr.left,
                wr.top,
                wr.right - wr.left,
                wr.bottom - wr.top,
                SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
            );
        }
    }

    fn apply_style(&mut self) {
        if self.fullscreen.is_some() {
            return;
        }
        let keep = self.client_rect();
        unsafe {
            let mut style = self.style_now();
            if IsWindowVisible(self.main).as_bool() {
                style |= WS_VISIBLE;
            }
            SetWindowLongPtrW(self.main, GWL_STYLE, style.0 as isize);
            // Square corners when borderless (Windows 11 would otherwise round them).
            let pref: i32 = if self.cfg.borderless { 1 } else { 0 };
            let _ = DwmSetWindowAttribute(self.main, DWMWINDOWATTRIBUTE(33), &pref as *const _ as *const _, 4);
        }
        if keep.w > 0 && keep.h > 0 {
            self.set_client_rect(keep);
        }
    }

    fn apply_topmost(&self) {
        unsafe {
            let after = if self.cfg.always_on_top { HWND_TOPMOST } else { HWND_NOTOPMOST };
            let _ = SetWindowPos(self.main, Some(after), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        }
    }

    fn toggle_fullscreen(&mut self) {
        self.drag = None;
        if let Some(saved) = self.fullscreen.take() {
            unsafe {
                let mut style = self.style_now();
                style |= WS_VISIBLE;
                SetWindowLongPtrW(self.main, GWL_STYLE, style.0 as isize);
            }
            self.set_client_rect(saved);
            self.remember_window();
        } else {
            let saved = self.client_rect();
            unsafe {
                let mon = MonitorFromWindow(self.main, MONITOR_DEFAULTTONEAREST);
                let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
                let _ = GetMonitorInfoW(mon, &mut mi);
                self.fullscreen = Some(saved);
                SetWindowLongPtrW(self.main, GWL_STYLE, (WS_POPUP | WS_SYSMENU | WS_MINIMIZEBOX | WS_VISIBLE).0 as isize);
                let m = mi.rcMonitor;
                let _ = SetWindowPos(
                    self.main,
                    None,
                    m.left,
                    m.top,
                    m.right - m.left,
                    m.bottom - m.top,
                    SWP_NOZORDER | SWP_FRAMECHANGED,
                );
            }
        }
        self.needs_render = true;
    }

    fn remember_window(&mut self) {
        if self.fullscreen.is_some() || unsafe { IsIconic(self.main).as_bool() } {
            return;
        }
        let r = self.client_rect();
        if r.w >= MIN_W && r.h >= MIN_H {
            let v = Some([r.x, r.y, r.w, r.h]);
            if self.cfg.window != v {
                self.cfg.window = v;
                self.mark_dirty();
            }
        }
    }

    fn set_window_size(&mut self, w: u32, h: u32) {
        if self.fullscreen.is_some() {
            self.toggle_fullscreen();
        }
        let cur = self.client_rect();
        let (w, h) = (w as i32, h as i32);
        let mut x = cur.x + (cur.w - w) / 2;
        let mut y = cur.y + (cur.h - h) / 2;
        let work = unsafe { work_area_near(cur) };
        // Keep the top-left corner reachable.
        x = x.max(work.left - w + 80).min(work.right - 80);
        y = y.max(work.top).min(work.bottom - 80);
        self.set_client_rect(Rect { x, y, w, h });
        self.remember_window();
        self.show_toast(&format!("{w} x {h}"), 1.5);
    }

    // ------------------------------------------------------------------ resize gesture

    fn cursor_client(&self) -> Option<(i32, i32)> {
        unsafe {
            let mut p = POINT::default();
            GetCursorPos(&mut p).ok()?;
            let _ = ScreenToClient(self.main, &mut p);
            let (w, h) = client_size(self.main);
            (p.x >= 0 && p.y >= 0 && p.x < w as i32 && p.y < h as i32).then_some((p.x, p.y))
        }
    }

    fn edge_at_cursor(&self) -> (i32, i32) {
        let (w, h) = unsafe { client_size(self.main) };
        let (x, y) = self.cursor_client().unwrap_or((w as i32, h as i32));
        let third = |v: i32, size: u32| {
            if v < size as i32 / 3 {
                -1
            } else if v > size as i32 * 2 / 3 {
                1
            } else {
                0
            }
        };
        match (third(x, w), third(y, h)) {
            (0, 0) => (1, 1),
            e => e,
        }
    }

    fn begin_resize(&mut self) {
        let (ex, ey) = self.edge_at_cursor();
        unsafe {
            let mut pt = POINT::default();
            let _ = GetCursorPos(&mut pt);
            let mut wr = RECT::default();
            let _ = GetWindowRect(self.main, &mut wr);
            let c = self.client_rect();
            let inset = [c.x - wr.left, c.y - wr.top, wr.right - (c.x + c.w), wr.bottom - (c.y + c.h)];
            self.drag = Some(ResizeDrag { ex, ey, start_pt: pt, start: wr, inset });
            SetCapture(self.main);
        }
        self.readout_until = Some(Instant::now() + Duration::from_millis(1500));
    }

    fn drag_resize(&mut self) {
        let Some(d) = self.drag.as_ref() else { return };
        let mut pt = POINT::default();
        unsafe {
            let _ = GetCursorPos(&mut pt);
        }
        let (dx, dy) = (pt.x - d.start_pt.x, pt.y - d.start_pt.y);
        let mut r = d.start;
        match d.ex {
            -1 => r.left += dx,
            1 => r.right += dx,
            _ => {}
        }
        match d.ey {
            -1 => r.top += dy,
            1 => r.bottom += dy,
            _ => {}
        }
        let [il, it, ir, ib] = d.inset;
        let cw = (r.right - r.left - il - ir).max(MIN_W);
        let ch = (r.bottom - r.top - it - ib).max(MIN_H);
        let (cw, ch) = self.snap(cw, ch, d.ex, d.ey);
        let (ww, wh) = (cw + il + ir, ch + it + ib);
        if d.ex < 0 {
            r.left = r.right - ww;
        } else {
            r.right = r.left + ww;
        }
        if d.ey < 0 {
            r.top = r.bottom - wh;
        } else {
            r.bottom = r.top + wh;
        }
        unsafe {
            let _ = SetWindowPos(self.main, None, r.left, r.top, ww, wh, SWP_NOZORDER | SWP_NOACTIVATE);
        }
        self.readout_until = Some(Instant::now() + Duration::from_millis(1500));
    }

    /// Gently pulls the size to "no bars" and to common resolutions.
    fn snap(&self, cw: i32, ch: i32, ex: i32, ey: i32) -> (i32, i32) {
        const S: i32 = 12;
        for &(sw, sh) in COMMON_SIZES {
            if (cw - sw).abs() <= S && (ch - sh).abs() <= S {
                return (sw, sh);
            }
        }
        let (fw, fh) = self.renderer.frame_size.unwrap_or((16, 9));
        let aspect = fw as f32 / fh.max(1) as f32;
        if ey != 0 {
            let ih = (cw as f32 / aspect).round() as i32;
            if (ch - ih).abs() <= S {
                return (cw, ih);
            }
        } else if ex != 0 {
            let iw = (ch as f32 * aspect).round() as i32;
            if (cw - iw).abs() <= S {
                return (iw, ch);
            }
        }
        (cw, ch)
    }

    fn end_resize(&mut self) {
        if self.drag.take().is_some() {
            unsafe {
                let _ = ReleaseCapture();
            }
            self.remember_window();
        }
    }

    fn readout_text(&self) -> Option<String> {
        if self.readout_until.is_none_or(|t| Instant::now() > t) {
            return None;
        }
        let (w, h) = (self.main_surface.width, self.main_surface.height);
        let mut s = format!("{w} x {h}");
        if let Some((fw, fh)) = self.renderer.frame_size {
            if (w, h) == (fw, fh) {
                s.push_str("  ·  1:1");
            } else if (w as u64 * fh as u64).abs_diff(h as u64 * fw as u64) <= fw.max(fh) as u64 {
                s.push_str("  ·  no bars");
            }
        }
        Some(s)
    }

    fn set_cursor_for_edge(ex: i32, ey: i32) {
        let id = match (ex, ey) {
            (-1, -1) | (1, 1) => IDC_SIZENWSE,
            (1, -1) | (-1, 1) => IDC_SIZENESW,
            (_, 0) => IDC_SIZEWE,
            _ => IDC_SIZENS,
        };
        unsafe {
            if let Ok(c) = LoadCursorW(None, id) {
                SetCursor(Some(c));
            }
        }
    }

    /// Handles WM_SETCURSOR for the video window. Returns true if handled.
    fn update_cursor(&self) -> bool {
        if let Some(d) = &self.drag {
            Self::set_cursor_for_edge(d.ex, d.ey);
            return true;
        }
        if self.cursor_client().is_none() {
            return false;
        }
        if resize_mods() && self.fullscreen.is_none() {
            let (ex, ey) = self.edge_at_cursor();
            Self::set_cursor_for_edge(ex, ey);
        } else if self.cursor_hidden {
            unsafe { SetCursor(None) };
        } else {
            unsafe {
                if let Ok(c) = LoadCursorW(None, IDC_ARROW) {
                    SetCursor(Some(c));
                }
            }
        }
        true
    }

    fn on_mouse_move(&mut self) {
        let mut p = POINT::default();
        unsafe {
            let _ = GetCursorPos(&mut p);
        }
        if p.x != self.last_mouse.1.x || p.y != self.last_mouse.1.y {
            self.last_mouse = (Instant::now(), p);
            if self.cursor_hidden {
                self.cursor_hidden = false;
                self.update_cursor();
            }
        }
    }

    // ------------------------------------------------------------------ toasts & shortcuts

    fn show_toast(&mut self, text: &str, secs: f32) {
        self.toast = Some((text.to_string(), Instant::now() + Duration::from_secs_f32(secs)));
        self.needs_render = true;
        if self.no_signal() {
            self.render_main();
        }
    }

    fn handle_hotkey(&mut self, vk: u16) -> bool {
        let m = egui_win::modifiers();
        let hk = Hotkey::new(m.ctrl, m.shift, m.alt, vk);
        if hk == self.cfg.key_settings {
            self.toggle_settings();
        } else if hk == self.cfg.key_fullscreen || (m.alt && !m.ctrl && !m.shift && vk == VK_RETURN.0) {
            self.toggle_fullscreen();
        } else if hk == self.cfg.key_mute {
            self.cfg.muted = !self.cfg.muted;
            self.sync_audio_controls();
            self.mark_dirty();
            let msg = if self.cfg.muted { "Muted".to_string() } else { format!("Volume {:.0}%", self.cfg.volume * 100.0) };
            self.show_toast(&msg, 1.2);
            self.refresh_settings_soon();
        } else {
            return false;
        }
        true
    }

    fn nudge_volume(&mut self, wheel: i32) {
        let step = 0.05 * (wheel as f32 / 120.0);
        self.cfg.volume = ((self.cfg.volume + step) * 20.0).round().clamp(0.0, 20.0) / 20.0;
        self.cfg.muted = false;
        self.sync_audio_controls();
        self.mark_dirty();
        let msg = format!("Volume {:.0}%", self.cfg.volume * 100.0);
        self.show_toast(&msg, 1.2);
        self.refresh_settings_soon();
    }

    fn sync_audio_controls(&self) {
        self.ashared.set_volume(self.cfg.volume);
        self.ashared.muted.store(self.cfg.muted, Ordering::Relaxed);
        self.ashared.delay_ms.store(self.cfg.audio_delay_ms, Ordering::Relaxed);
    }

    fn mark_dirty(&mut self) {
        self.dirty_since.get_or_insert_with(Instant::now);
    }

    // ------------------------------------------------------------------ capture & audio

    fn restart_capture(&mut self) {
        if let Some(c) = self.capture.take() {
            c.stop();
        }
        let stale = self.vshared.latest.lock().unwrap().take();
        if let Some(f) = stale {
            self.vshared.recycle(f.data);
        }
        *self.vshared.status.lock().unwrap() = video::VideoStatus::default();
        self.last_frame = None;
        self.renderer.clear_frame();
        self.capture = Some(Capture::start(
            self.cfg.video_device.clone(),
            self.cfg.video_link.clone(),
            self.cfg.mode.clone(),
            self.main,
            self.vshared.clone(),
        ));
        self.needs_render = true;
    }

    /// Which audio input to use right now, following the "auto" rule.
    fn resolve_input(&mut self) -> Option<audio::Endpoint> {
        match self.cfg.audio_in.as_str() {
            "off" => None,
            "auto" => {
                if self.inputs.is_empty() {
                    self.inputs = audio::list_endpoints(true);
                }
                let link = self.vshared.status.lock().unwrap().link.clone();
                let link = if link.is_empty() { self.cfg.video_link.clone() } else { link };
                audio::match_input(&link, &self.inputs)
            }
            id => Some(
                self.inputs
                    .iter()
                    .find(|e| e.id == id)
                    .cloned()
                    .unwrap_or(audio::Endpoint { id: id.to_string(), name: id.to_string(), container: None }),
            ),
        }
    }

    fn restart_audio(&mut self) {
        if let Some(a) = self.audio.take() {
            a.stop();
        }
        self.audio_link = self.vshared.status.lock().unwrap().link.clone();
        self.ashared.peak.store(0, Ordering::Relaxed);
        let input = self.resolve_input();
        self.resolved_input = input.clone();
        match input {
            None if self.cfg.audio_in == "off" => self.ashared.set_status("Audio is off"),
            None => self.ashared.set_status("No matching audio input found. Pick one above."),
            Some(e) => {
                self.ashared.set_status(format!("Starting {}…", e.name));
                self.audio = Some(Audio::start(e.id, self.cfg.audio_out.clone(), self.cfg.ms2109_audio_fix, self.ashared.clone()));
            }
        }
    }

    /// What Discord should show right now (None = presence off).
    fn presence(&self) -> Option<discord::Presence> {
        let c = &self.cfg;
        if !c.discord_enabled {
            return None;
        }
        let app_id = if c.discord_app_id.trim().is_empty() { DISCORD_APP_ID } else { c.discord_app_id.trim() };
        // A blank or unusable picture link falls back to the built-in picture.
        let picture = |link: &str, fallback: &str| {
            let link = link.trim();
            if discord::is_picture(link) { link.to_string() } else { fallback.to_string() }
        };
        Some(discord::Presence {
            app_id: app_id.to_string(),
            title: c.discord_title.clone(),
            kind: c.discord_kind.id(),
            details: c.discord_details.clone(),
            state: c.discord_extra_on.then(|| c.discord_extra.clone()),
            large_image: picture(&c.discord_image, DISCORD_ART_URL),
            large_text: c.discord_large_text.clone(),
            small_image: c.discord_small_on.then(|| picture(&c.discord_small_image, DISCORD_BADGE_URL)),
            small_text: c.discord_small_text.clone(),
            button: c.discord_button_on.then(|| (c.discord_button_label.clone(), c.discord_button_url.clone())),
            start: c.discord_elapsed.then_some(self.discord_start),
        })
    }

    fn update_discord(&self) {
        if let Some(d) = &self.discord {
            d.set(self.presence());
        }
    }

    fn refresh_devices(&mut self) {
        self.devices = video::list_devices();
        self.inputs = audio::list_endpoints(true);
        self.outputs = audio::list_endpoints(false);
    }

    fn load_image(&mut self) {
        if self.cfg.image_path.is_empty() {
            self.renderer.set_image(&self.gfx, None);
            return;
        }
        match crate::image::load_file(&self.cfg.image_path, 4096) {
            Ok(img) => self.renderer.set_image(&self.gfx, Some((&img.px, img.w, img.h))),
            Err(e) => {
                crate::log!("image: {e}");
                self.renderer.set_image(&self.gfx, None);
                self.show_toast("Couldn't open that image", 2.5);
            }
        }
        self.needs_render = true;
    }

    fn pick_image(&mut self) {
        let owner = self.settings;
        // The dialog runs its own message loop; nothing here is borrowed across it.
        let picked = crate::image::pick_image(owner);
        let Some(path) = picked else { return };
        self.cfg.image_path = path;
        let uses_image = [self.cfg.left, self.cfg.right, self.cfg.top, self.cfg.bottom].contains(&Zone::Image)
            || self.cfg.background == Fill::Image;
        if !uses_image {
            self.cfg.background = Fill::Image;
        }
        self.load_image();
        self.mark_dirty();
        self.render_main();
        self.render_settings();
    }

    // ------------------------------------------------------------------ frames & drawing

    fn no_signal(&self) -> bool {
        !self.renderer.has_frame() || self.last_frame.is_none_or(|t| t.elapsed() > Duration::from_millis(1500))
    }

    fn on_frame(&mut self) {
        self.vshared.pending.store(false, Ordering::SeqCst);
        let Some(f) = self.vshared.latest.lock().unwrap().take() else { return };
        if unsafe { IsIconic(self.main).as_bool() } {
            self.vshared.recycle(f.data);
            self.last_frame = Some(Instant::now());
            return;
        }
        let full = match self.cfg.color_range {
            ColorRange::Auto => f.src_mjpeg || f.range_full.unwrap_or(false),
            ColorRange::Limited => false,
            ColorRange::Full => true,
        };
        let bt709 = match self.cfg.color_matrix {
            ColorMatrix::Auto => !f.src_mjpeg && f.matrix_709.unwrap_or(f.height >= 720),
            ColorMatrix::Bt601 => false,
            ColorMatrix::Bt709 => true,
        };
        if let Err(e) = self.renderer.upload(&self.gfx, &f, full, bt709) {
            crate::log!("upload: {e}");
        }
        self.vshared.recycle(f.data);
        self.last_frame = Some(Instant::now());
        if self.anim_on {
            unsafe {
                let _ = KillTimer(Some(self.main), TIMER_ANIM);
            }
            self.anim_on = false;
        }
        self.render_main();
    }

    fn look(&self, w: u32, h: u32, has_video: bool) -> Look {
        let c = &self.cfg;
        let (fw, fh) = self.renderer.frame_size.unwrap_or((1920, 1080));
        let bg = c.background;
        let (frame_rgb, frame_px) = match c.frame {
            FrameColor::None => ([0.0; 3], 0.0),
            FrameColor::Black => ([0.0; 3], c.frame_px as f32),
            FrameColor::White => ([1.0; 3], c.frame_px as f32),
            FrameColor::Purple => ([0.482, 0.310, 0.788], c.frame_px as f32),
        };
        let zones = if has_video {
            [c.left, c.right, c.top, c.bottom].map(|z| fill_id(z.resolve(bg)))
        } else {
            [fill_id(bg); 4]
        };
        Look {
            vrect: video_rect(c.scaling, fw, fh, w, h),
            has_video,
            zones,
            frame_rgb,
            frame_px,
            filter: match c.filter {
                Filter::Sharp => 0,
                Filter::Smooth => 1,
                Filter::Pixel => 2,
            },
            time: self.start.elapsed().as_secs_f32() % 3600.0,
            twinkle: c.twinkle,
            star_density: c.stars.density(),
            img_rect: image_rect(c.image_fit, self.renderer.image_size(), w, h),
            img_dim: c.image_dim,
        }
    }

    fn galaxy_visible(&self) -> bool {
        let c = &self.cfg;
        c.background == Fill::Galaxy
            || (c.background == Fill::Image && self.renderer.image_size().is_none())
            || [c.left, c.right, c.top, c.bottom].contains(&Zone::Galaxy)
    }

    fn render_main(&mut self) {
        self.needs_render = false;
        if unsafe { IsIconic(self.main).as_bool() } {
            return;
        }
        let (w, h) = (self.main_surface.width, self.main_surface.height);
        if self.main_surface.begin(&self.gfx).is_none() {
            return;
        }
        let no_signal = self.no_signal();
        let look = self.look(w, h, !no_signal);
        self.renderer.draw(&self.gfx, w, h, &look);

        let now = Instant::now();
        let readout = self.readout_text();
        let toast = self.toast.as_ref().filter(|(_, t)| now < *t).map(|(s, _)| s.clone());
        let status = no_signal.then(|| self.vshared.status.lock().unwrap().state.clone());
        if status.is_some() || readout.is_some() || toast.is_some() {
            if self.overlay_logo.is_none() {
                self.overlay_logo = self.logo.as_ref().map(|l| {
                    self.overlay.ctx.load_texture(
                        "logo",
                        egui::ColorImage::from_rgba_unmultiplied([l.w as usize, l.h as usize], &l.px),
                        egui::TextureOptions::LINEAR,
                    )
                });
            }
            let ov = ui::Overlay {
                no_signal: status.as_deref(),
                readout: readout.as_deref(),
                toast: toast.as_deref(),
                logo: self.overlay_logo.as_ref(),
                settings_key: self.cfg.key_settings.to_string(),
            };
            let frame = self.overlay.run([w, h], |ctx| ui::overlay(ctx, &ov));
            self.overlay.paint(&self.gfx, [w, h], &frame);
        }
        self.maybe_snapshot(0);
        self.main_surface.present();
    }

    /// Testing aid: after 5 s, save what window `which` (0 main, 1 settings) shows.
    fn maybe_snapshot(&mut self, which: usize) {
        let Some(dir) = &self.snap_dir else { return };
        if self.snapped[which] || self.start.elapsed() < Duration::from_secs(5) {
            return;
        }
        let (surface, name) = match which {
            0 => (Some(&self.main_surface), "main.png"),
            _ => (self.settings_surface.as_ref(), "settings.png"),
        };
        if let Some(s) = surface {
            let path = format!("{dir}\\{name}");
            match crate::snapshot::save(&self.gfx, s, &path) {
                Ok(()) => crate::log!("snapshot: {path}"),
                Err(e) => crate::log!("snapshot failed: {e}"),
            }
            self.snapped[which] = true;
        }
    }

    // ------------------------------------------------------------------ settings window

    fn toggle_settings(&mut self) {
        if unsafe { IsWindowVisible(self.settings).as_bool() } {
            self.hide_settings();
        } else {
            self.show_settings();
        }
    }

    fn show_settings(&mut self) {
        self.refresh_devices();
        unsafe {
            if !self.settings_placed {
                self.settings_placed = true;
                let dpi = GetDpiForWindow(self.main) as f32 / 96.0;
                let main = self.client_rect();
                let work = work_area_near(main);
                let sw = (470.0 * dpi) as i32;
                let sh = ((860.0 * dpi) as i32).min(work.bottom - work.top - 40);
                let mut x = main.x + main.w + 12;
                if x + sw > work.right {
                    x = main.x + (main.w - sw) / 2;
                }
                let x = x.clamp(work.left, (work.right - sw).max(work.left));
                let y = (main.y + (main.h - sh) / 2).clamp(work.top, (work.bottom - sh).max(work.top));
                let _ = SetWindowPos(self.settings, None, x, y, sw, sh, SWP_NOZORDER | SWP_NOACTIVATE);
            }
            self.ui.ppp = GetDpiForWindow(self.settings) as f32 / 96.0;
            let _ = ShowWindow(self.settings, SW_SHOW);
            let _ = SetForegroundWindow(self.settings);
            if self.settings_surface.is_none() {
                let (w, h) = client_size(self.settings);
                match Surface::new(&self.gfx, self.settings, w, h) {
                    Ok(s) => self.settings_surface = Some(s),
                    Err(e) => crate::log!("settings surface: {e}"),
                }
            }
            SetTimer(Some(self.settings), TIMER_UI, 250, None);
        }
        self.render_settings();
    }

    fn hide_settings(&mut self) {
        self.capturing = None;
        unsafe {
            let _ = KillTimer(Some(self.settings), TIMER_UI);
            let _ = ShowWindow(self.settings, SW_HIDE);
            let _ = SetForegroundWindow(self.main);
        }
    }

    fn refresh_settings_soon(&self) {
        unsafe {
            if IsWindowVisible(self.settings).as_bool() {
                SetTimer(Some(self.settings), TIMER_UI_SOON, 16, None);
            }
        }
    }

    fn render_settings(&mut self) {
        if !unsafe { IsWindowVisible(self.settings).as_bool() } {
            return;
        }
        let Some((w, h)) = self.settings_surface.as_ref().map(|s| (s.width, s.height)) else { return };
        if self.ui_logo.is_none() {
            self.ui_logo = self.logo.as_ref().map(|l| {
                self.ui.ctx.load_texture(
                    "logo",
                    egui::ColorImage::from_rgba_unmultiplied([l.w as usize, l.h as usize], &l.px),
                    egui::TextureOptions::LINEAR,
                )
            });
        }

        if self.discord_art.is_none() {
            self.discord_art = self.discord_img.as_ref().map(|l| {
                self.ui.ctx.load_texture(
                    "discord-art",
                    egui::ColorImage::from_rgba_unmultiplied([l.w as usize, l.h as usize], &l.px),
                    egui::TextureOptions::LINEAR,
                )
            });
        }
        if self.discord_badge.is_none() {
            self.discord_badge = self.discord_badge_img.as_ref().map(|l| {
                self.ui.ctx.load_texture(
                    "discord-badge",
                    egui::ColorImage::from_rgba_unmultiplied([l.w as usize, l.h as usize], &l.px),
                    egui::TextureOptions::LINEAR,
                )
            });
        }
        let discord_status = self.discord.as_ref().map(|d| d.status()).unwrap_or_default();
        let elapsed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs().saturating_sub(self.discord_start))
            .unwrap_or(0);

        let before = self.cfg.clone();
        let vstatus = self.vshared.status.lock().unwrap().clone();
        let audio_status = self.ashared.status.lock().unwrap().clone();
        let auto_input = self.resolved_input.as_ref().map(|e| e.name.clone());
        let info = ui::Info {
            devices: &self.devices,
            video: &vstatus,
            inputs: &self.inputs,
            outputs: &self.outputs,
            audio_status: &audio_status,
            auto_input: if self.cfg.audio_in == "auto" { auto_input.as_deref() } else { None },
            level: self.ashared.peak(),
            fps: self.fps,
            frame_size: self.renderer.frame_size,
            win_size: (self.main_surface.width, self.main_surface.height),
            capturing: self.capturing,
            logo: self.ui_logo.as_ref(),
            discord_status: &discord_status,
            discord_art: self.discord_art.as_ref(),
            discord_badge: self.discord_badge.as_ref(),
            elapsed,
        };
        let mut actions = Vec::new();
        let cfg = &mut self.cfg;
        let frame = self.ui.run([w, h], |ctx| ui::settings(ctx, cfg, &info, &mut actions));

        if let Some(rtv) = self.settings_surface.as_ref().and_then(|s| s.begin(&self.gfx)).cloned() {
            let bg = ui::BG.to_normalized_gamma_f32();
            unsafe { self.gfx.ctx.ClearRenderTargetView(&rtv, &bg) };
            self.ui.paint(&self.gfx, [w, h], &frame);
            self.maybe_snapshot(1);
            if let Some(s) = self.settings_surface.as_ref() {
                s.present();
            }
        }
        if let Some(text) = &frame.copied {
            egui_win::clipboard_set(self.settings, text);
        }
        if frame.repaint_after < Duration::from_secs(1) {
            let ms = frame.repaint_after.as_millis().clamp(8, 1000) as u32;
            unsafe {
                SetTimer(Some(self.settings), TIMER_UI_SOON, ms, None);
            }
        }

        if self.cfg != before {
            self.apply_changes(&before);
        }
        for a in actions {
            match a {
                Action::CaptureHotkey(t) => {
                    self.capturing = Some(t);
                    self.refresh_settings_soon();
                }
                Action::WindowSize(w, h) => self.set_window_size(w, h),
                Action::PickImage => unsafe {
                    let _ = PostMessageW(Some(self.settings), WM_APP_PICK_IMAGE, WPARAM(0), LPARAM(0));
                },
                Action::RefreshDevices => {
                    self.refresh_devices();
                    self.refresh_settings_soon();
                }
            }
        }
    }

    fn apply_changes(&mut self, before: &Config) {
        let c = self.cfg.clone();
        if c.video_device != before.video_device || c.video_link != before.video_link || c.mode != before.mode {
            self.restart_capture();
        }
        if c.audio_in != before.audio_in || c.audio_out != before.audio_out || c.ms2109_audio_fix != before.ms2109_audio_fix {
            self.restart_audio();
        }
        self.sync_audio_controls();
        if c.borderless != before.borderless {
            self.apply_style();
        }
        if c.always_on_top != before.always_on_top {
            self.apply_topmost();
        }
        if c.image_path != before.image_path {
            self.load_image();
        }
        self.update_discord();
        self.mark_dirty();
        self.render_main();
    }

    fn set_captured_hotkey(&mut self, vk: u16) {
        let Some(t) = self.capturing.take() else { return };
        if vk == VK_ESCAPE.0 {
            return;
        }
        let m = egui_win::modifiers();
        let hk = Hotkey::new(m.ctrl, m.shift, m.alt, vk);
        match t {
            HotkeyTarget::Settings => self.cfg.key_settings = hk,
            HotkeyTarget::Fullscreen => self.cfg.key_fullscreen = hk,
            HotkeyTarget::Mute => self.cfg.key_mute = hk,
        }
        self.mark_dirty();
    }

    // ------------------------------------------------------------------ timers

    /// Debug builds only: STARHOWL_SELFTEST=1 drives the app through its window
    /// messages (scroll, wheel, fullscreen, borderless, resize, close) and logs
    /// what happened.
    #[cfg(debug_assertions)]
    fn selftest(&mut self) {
        if std::env::var("STARHOWL_SELFTEST").is_err() {
            return;
        }
        let t = self.start.elapsed().as_secs_f32();
        let step = (t * 4.0) as u32; // one step per tick
        let post = |h: HWND, m: u32, w: usize, l: isize| unsafe {
            let _ = PostMessageW(Some(h), m, WPARAM(w), LPARAM(l));
        };
        let wheel_down = 0xFF88_0000usize;
        let r = self.client_rect();
        match step {
            8 => {
                post(self.settings, WM_MOUSEMOVE, 0, 200 | (500 << 16));
                for _ in 0..40 {
                    post(self.settings, WM_MOUSEWHEEL, wheel_down, 0);
                }
            }
            12 => {
                for _ in 0..3 {
                    post(self.main, WM_MOUSEWHEEL, wheel_down, 0);
                }
            }
            14 => crate::log!("selftest: volume now {:.0}%", self.cfg.volume * 100.0),
            16 => {
                // Same path as picking another mode in settings.
                self.restart_capture();
                crate::log!("selftest: capture restarted");
            }
            21 => {
                let live = self.last_frame.is_some_and(|t| t.elapsed() < Duration::from_millis(500));
                crate::log!("selftest: video live again after restart = {live}");
            }
            23 => {
                crate::log!("selftest: windowed client {},{} {}x{}", r.x, r.y, r.w, r.h);
                post(self.main, WM_SYSCOMMAND, IDM_FULLSCREEN, 0);
            }
            25 => {
                crate::log!("selftest: fullscreen client {},{} {}x{}", r.x, r.y, r.w, r.h);
                post(self.main, WM_SYSCOMMAND, IDM_FULLSCREEN, 0);
            }
            27 => {
                crate::log!("selftest: restored client {},{} {}x{}", r.x, r.y, r.w, r.h);
                self.cfg.borderless = !self.cfg.borderless;
                self.apply_style();
            }
            28 => {
                crate::log!("selftest: borderless={} client {},{} {}x{}", self.cfg.borderless, r.x, r.y, r.w, r.h);
                self.set_window_size(960, 540);
            }
            29 => {
                crate::log!("selftest: after size preset client {}x{} surface {}x{}", r.w, r.h, self.main_surface.width, self.main_surface.height);
                post(self.main, WM_SYSCOMMAND, IDM_SETTINGS, 0);
            }
            30 => {
                let vis = unsafe { IsWindowVisible(self.settings).as_bool() };
                crate::log!("selftest: settings visible after toggle = {vis}");
            }
            s if s >= 34 => post(self.main, WM_CLOSE, 0, 0),
            _ => {}
        }
    }

    fn on_tick(&mut self) {
        let now = Instant::now();
        #[cfg(debug_assertions)]
        self.selftest();

        let dt = now.duration_since(self.fps_at).as_secs_f32();
        if dt >= 1.0 {
            self.fps = self.vshared.frames.swap(0, Ordering::Relaxed) as f32 / dt;
            self.fps_at = now;
        }

        // Pick up the matching audio once the video device is known.
        if self.cfg.audio_in == "auto" {
            let link = self.vshared.status.lock().unwrap().link.clone();
            if !link.is_empty() && link != self.audio_link {
                self.audio_link = link;
                let input = self.resolve_input();
                let current = self.audio.as_ref().map(|a| a.input_id.clone());
                if input.as_ref().map(|e| e.id.clone()) != current {
                    self.restart_audio();
                } else {
                    self.resolved_input = input;
                }
            }
        }

        // No signal: show the waiting screen, animated if stars twinkle.
        let no_signal = self.no_signal();
        let want_anim = no_signal && self.cfg.twinkle && self.galaxy_visible();
        if want_anim != self.anim_on {
            unsafe {
                if want_anim {
                    SetTimer(Some(self.main), TIMER_ANIM, 33, None);
                } else {
                    let _ = KillTimer(Some(self.main), TIMER_ANIM);
                }
            }
            self.anim_on = want_anim;
        }
        if let Some((_, until)) = &self.toast {
            if now > *until {
                self.toast = None;
                self.needs_render = true;
            }
        }
        if self.readout_until.is_some_and(|t| now > t) && self.drag.is_none() {
            self.readout_until = None;
            self.needs_render = true;
        }
        if no_signal && !self.anim_on {
            // Refresh the status line on the waiting screen now and then.
            self.needs_render = true;
        }
        if self.needs_render && no_signal && !self.anim_on {
            self.render_main();
        }

        // Hide an idle pointer over the video.
        if self.cfg.hide_cursor
            && !self.cursor_hidden
            && self.drag.is_none()
            && now.duration_since(self.last_mouse.0) > Duration::from_millis(2500)
            && self.cursor_client().is_some()
            && unsafe { GetForegroundWindow() } == self.main
        {
            self.cursor_hidden = true;
            unsafe { SetCursor(None) };
        }

        if self.dirty_since.is_some_and(|t| now.duration_since(t) > Duration::from_secs(1)) {
            self.dirty_since = None;
            self.cfg.save();
        }
    }

    fn shutdown(mut self: Box<Self>) {
        // Clear the Discord activity first so it doesn't linger after we quit.
        if let Some(d) = self.discord.take() {
            d.stop();
        }
        crate::log!("shutdown: saving settings");
        if let Some(saved) = self.fullscreen {
            self.cfg.window = Some([saved.x, saved.y, saved.w, saved.h]);
        }
        self.cfg.save();
        if let Some(c) = self.capture.take() {
            c.stop();
        }
        crate::log!("shutdown: video stopped");
        if let Some(a) = self.audio.take() {
            a.stop();
        }
        crate::log!("shutdown: audio stopped");
    }
}

// ---------------------------------------------------------------------- window procedures

unsafe extern "system" fn main_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let Some(a) = app() else { return DefWindowProcW(hwnd, msg, wp, lp) };
    if hwnd != a.main {
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
    match msg {
        WM_APP_FRAME => {
            a.on_frame();
            LRESULT(0)
        }
        WM_SIZE => {
            let (w, h) = (loword(lp.0), hiword(lp.0));
            if w > 0 && h > 0 {
                a.main_surface.resize(&a.gfx, w as u32, h as u32);
                a.render_main();
                a.refresh_settings_soon();
            }
            LRESULT(0)
        }
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            BeginPaint(hwnd, &mut ps);
            let _ = EndPaint(hwnd, &ps);
            a.render_main();
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_TIMER => {
            match wp.0 {
                TIMER_TICK => a.on_tick(),
                TIMER_ANIM => a.render_main(),
                _ => {}
            }
            LRESULT(0)
        }
        WM_NCHITTEST => {
            let hit = DefWindowProcW(hwnd, msg, wp, lp);
            // The whole video acts like a title bar (drag to move) unless resizing.
            if hit.0 == HTCLIENT as isize && a.fullscreen.is_none() && a.drag.is_none() && !resize_mods() {
                LRESULT(HTCAPTION as isize)
            } else {
                hit
            }
        }
        WM_NCLBUTTONDBLCLK if wp.0 == HTCAPTION as usize && a.cursor_client().is_some() => {
            a.toggle_fullscreen();
            LRESULT(0)
        }
        WM_LBUTTONDBLCLK => {
            a.toggle_fullscreen();
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            if resize_mods() && a.fullscreen.is_none() {
                a.begin_resize();
            }
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            a.on_mouse_move();
            if a.drag.is_some() {
                a.drag_resize();
            }
            LRESULT(0)
        }
        WM_NCMOUSEMOVE => {
            a.on_mouse_move();
            DefWindowProcW(hwnd, msg, wp, lp)
        }
        WM_LBUTTONUP => {
            a.end_resize();
            LRESULT(0)
        }
        WM_CAPTURECHANGED => {
            if a.drag.take().is_some() {
                a.remember_window();
            }
            LRESULT(0)
        }
        WM_SETCURSOR => {
            if a.update_cursor() {
                LRESULT(1)
            } else {
                DefWindowProcW(hwnd, msg, wp, lp)
            }
        }
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            let vk = wp.0 as u16;
            if is_modifier(vk) {
                a.update_cursor();
            } else if a.handle_hotkey(vk) {
                return LRESULT(0);
            } else if vk == VK_ESCAPE.0 && a.fullscreen.is_some() {
                a.toggle_fullscreen();
                return LRESULT(0);
            }
            DefWindowProcW(hwnd, msg, wp, lp)
        }
        WM_KEYUP | WM_SYSKEYUP => {
            if is_modifier(wp.0 as u16) {
                a.update_cursor();
            }
            DefWindowProcW(hwnd, msg, wp, lp)
        }
        // Stop the "ding" for Alt+letter shortcuts, but keep Alt+Space.
        WM_SYSCHAR if wp.0 != ' ' as usize => LRESULT(0),
        WM_SYSCOMMAND => {
            let cmd = wp.0 & 0xFFF0;
            if cmd == SC_KEYMENU as usize && lp.0 == 0 {
                LRESULT(0)
            } else if cmd == IDM_SETTINGS {
                a.toggle_settings();
                LRESULT(0)
            } else if cmd == IDM_FULLSCREEN {
                a.toggle_fullscreen();
                LRESULT(0)
            } else {
                DefWindowProcW(hwnd, msg, wp, lp)
            }
        }
        WM_MOUSEWHEEL => {
            a.nudge_volume(hiword(wp.0 as isize) as i16 as i32);
            LRESULT(0)
        }
        WM_EXITSIZEMOVE => {
            a.remember_window();
            LRESULT(0)
        }
        WM_GETMINMAXINFO => {
            let mmi = &mut *(lp.0 as *mut MINMAXINFO);
            mmi.ptMinTrackSize = POINT { x: MIN_W, y: MIN_H };
            LRESULT(0)
        }
        WM_DPICHANGED => {
            // Keep the window's pixel size: resolutions are pixels.
            a.overlay.ppp = hiword(wp.0 as isize) as f32 / 96.0;
            LRESULT(0)
        }
        WM_CLOSE => {
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }
        WM_DESTROY => {
            // Still alive here, so this is the last chance to note where it was.
            a.remember_window();
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

unsafe extern "system" fn settings_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let Some(a) = app() else { return DefWindowProcW(hwnd, msg, wp, lp) };
    if hwnd != a.settings {
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
    match msg {
        WM_CLOSE => {
            a.hide_settings();
            LRESULT(0)
        }
        WM_SIZE => {
            let (w, h) = (loword(lp.0), hiword(lp.0));
            if w > 0 && h > 0 {
                if let Some(s) = a.settings_surface.as_mut() {
                    s.resize(&a.gfx, w as u32, h as u32);
                }
                a.render_settings();
            }
            LRESULT(0)
        }
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            BeginPaint(hwnd, &mut ps);
            let _ = EndPaint(hwnd, &ps);
            a.render_settings();
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_TIMER => {
            if wp.0 == TIMER_UI_SOON {
                let _ = KillTimer(Some(hwnd), TIMER_UI_SOON);
            }
            a.render_settings();
            LRESULT(0)
        }
        WM_SETCURSOR if loword(lp.0) as u32 == HTCLIENT => {
            if let Ok(c) = LoadCursorW(None, egui_win::cursor_for(a.ui.cursor)) {
                SetCursor(Some(c));
            }
            LRESULT(1)
        }
        WM_DPICHANGED => {
            a.ui.ppp = hiword(wp.0 as isize) as f32 / 96.0;
            let r = &*(lp.0 as *const RECT);
            let _ = SetWindowPos(hwnd, None, r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOZORDER | SWP_NOACTIVATE);
            LRESULT(0)
        }
        WM_GETMINMAXINFO => {
            let mmi = &mut *(lp.0 as *mut MINMAXINFO);
            mmi.ptMinTrackSize = POINT { x: 380, y: 360 };
            LRESULT(0)
        }
        WM_APP_PICK_IMAGE => {
            a.pick_image();
            LRESULT(0)
        }
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            let vk = wp.0 as u16;
            if a.capturing.is_some() {
                if !is_modifier(vk) {
                    a.set_captured_hotkey(vk);
                    a.render_settings();
                }
                return LRESULT(0);
            }
            if !is_modifier(vk) && a.handle_hotkey(vk) {
                return LRESULT(0);
            }
            if vk == VK_ESCAPE.0 {
                a.hide_settings();
                return LRESULT(0);
            }
            if a.ui.on_message(msg, wp, lp) {
                a.render_settings();
            }
            if msg == WM_SYSKEYDOWN {
                DefWindowProcW(hwnd, msg, wp, lp)
            } else {
                LRESULT(0)
            }
        }
        WM_SYSCHAR if wp.0 != ' ' as usize => LRESULT(0),
        WM_SYSCOMMAND if (wp.0 & 0xFFF0) == SC_KEYMENU as usize && lp.0 == 0 => LRESULT(0),
        WM_MOUSEMOVE => {
            if !a.ui.tracking_leave {
                let mut t = TRACKMOUSEEVENT {
                    cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: hwnd,
                    dwHoverTime: 0,
                };
                if TrackMouseEvent(&mut t).is_ok() {
                    a.ui.tracking_leave = true;
                }
            }
            a.ui.on_message(msg, wp, lp);
            a.render_settings();
            LRESULT(0)
        }
        WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN => {
            SetCapture(hwnd);
            a.ui.on_message(msg, wp, lp);
            a.render_settings();
            LRESULT(0)
        }
        WM_LBUTTONUP | WM_RBUTTONUP | WM_MBUTTONUP => {
            let _ = ReleaseCapture();
            a.ui.on_message(msg, wp, lp);
            a.render_settings();
            LRESULT(0)
        }
        WM_MOUSEWHEEL | WM_MOUSEHWHEEL => {
            // Wheel messages carry screen coordinates; egui only needs the delta.
            a.ui.on_message(msg, wp, lp);
            a.render_settings();
            LRESULT(0)
        }
        WM_LBUTTONDBLCLK | WM_RBUTTONDBLCLK | WM_MOUSELEAVE | WM_CHAR | WM_KEYUP | WM_SYSKEYUP | WM_SETFOCUS
        | WM_KILLFOCUS => {
            if a.ui.on_message(msg, wp, lp) {
                a.render_settings();
            }
            DefWindowProcW(hwnd, msg, wp, lp)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

#[allow(dead_code)]
fn message_box(text: &str) {
    unsafe {
        MessageBoxW(None, &HSTRING::from(text), w!("Starhowl"), MB_ICONERROR | MB_OK);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_pillarboxes_a_wide_window() {
        // 1080p video in a 1700x720 window: 1280x720 centred, 210px bars.
        assert_eq!(video_rect(Scaling::Fit, 1920, 1080, 1700, 720), [210.0, 0.0, 1490.0, 720.0]);
    }

    #[test]
    fn fit_letterboxes_a_tall_window() {
        assert_eq!(video_rect(Scaling::Fit, 1920, 1080, 1280, 1000), [0.0, 140.0, 1280.0, 860.0]);
    }

    #[test]
    fn integer_scaling_uses_whole_steps() {
        // 2.4x fits -> 2x; 0.7x fits -> 1/2x.
        assert_eq!(video_rect(Scaling::Integer, 640, 480, 1600, 1200), [160.0, 120.0, 1440.0, 1080.0]);
        assert_eq!(video_rect(Scaling::Integer, 1920, 1080, 1344, 756), [192.0, 108.0, 1152.0, 648.0]);
    }

    #[test]
    fn fixed_scales_and_stretch() {
        assert_eq!(video_rect(Scaling::Half, 1920, 1080, 1920, 1080), [480.0, 270.0, 1440.0, 810.0]);
        assert_eq!(video_rect(Scaling::Third, 1920, 1080, 640, 360), [0.0, 0.0, 640.0, 360.0]);
        assert_eq!(video_rect(Scaling::Stretch, 1920, 1080, 800, 800), [0.0, 0.0, 800.0, 800.0]);
    }

    #[test]
    fn image_cover_fills_and_centre_keeps_size() {
        assert_eq!(image_rect(ImageFit::Cover, Some((1000, 1000)), 1600, 900), [0.0, -350.0, 1600.0, 1250.0]);
        assert_eq!(image_rect(ImageFit::Center, Some((200, 100)), 1000, 500), [400.0, 200.0, 600.0, 300.0]);
    }
}
