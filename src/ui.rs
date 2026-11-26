//! The settings popup and the small on-video overlays, drawn with egui.

use crate::audio::Endpoint;
use crate::config::*;
use crate::video::{DeviceInfo, VideoStatus};
use egui::{Align2, Color32, RichText, Stroke, TextureHandle};

pub const BG: Color32 = Color32::from_rgb(0x12, 0x0A, 0x20);
const SURFACE: Color32 = Color32::from_rgb(0x1A, 0x10, 0x2E);
const WIDGET: Color32 = Color32::from_rgb(0x24, 0x16, 0x3E);
const WIDGET_HOVER: Color32 = Color32::from_rgb(0x30, 0x1D, 0x55);
const ACCENT: Color32 = Color32::from_rgb(0x7B, 0x4F, 0xC9);
const LAVENDER: Color32 = Color32::from_rgb(0xCD, 0xB8, 0xFF);
const TEXT: Color32 = Color32::from_rgb(0xE9, 0xE3, 0xF5);
const WEAK: Color32 = Color32::from_rgb(0x9C, 0x8F, 0xB8);
const COMBO_W: f32 = 250.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HotkeyTarget {
    Settings,
    Fullscreen,
    Mute,
}

pub enum Action {
    CaptureHotkey(HotkeyTarget),
    WindowSize(u32, u32),
    PickImage,
    RefreshDevices,
}

/// Read-only facts the settings page shows.
pub struct Info<'a> {
    pub devices: &'a [DeviceInfo],
    pub video: &'a VideoStatus,
    pub inputs: &'a [Endpoint],
    pub outputs: &'a [Endpoint],
    pub audio_status: &'a str,
    /// The input "Auto" picked, if any.
    pub auto_input: Option<&'a str>,
    pub level: f32,
    pub fps: f32,
    pub frame_size: Option<(u32, u32)>,
    pub win_size: (u32, u32),
    pub capturing: Option<HotkeyTarget>,
    pub logo: Option<&'a TextureHandle>,
    pub discord_status: &'a str,
    /// The galaxy wolf picture, for the Discord preview card.
    pub discord_art: Option<&'a TextureHandle>,
    /// The small purple play badge, for the preview card.
    pub discord_badge: Option<&'a TextureHandle>,
    /// Seconds since Starhowl started (the presence timer).
    pub elapsed: u64,
}

pub fn apply_theme(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();
    v.panel_fill = BG;
    v.window_fill = SURFACE;
    v.extreme_bg_color = Color32::from_rgb(0x0B, 0x06, 0x14);
    v.faint_bg_color = SURFACE;
    v.selection.bg_fill = ACCENT;
    v.selection.stroke = Stroke::new(1.0_f32, Color32::WHITE);
    v.hyperlink_color = LAVENDER;
    v.slider_trailing_fill = true;
    let w = &mut v.widgets;
    w.noninteractive.fg_stroke = Stroke::new(1.0_f32, TEXT);
    w.noninteractive.bg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(0x2C, 0x1D, 0x4A));
    w.inactive.bg_fill = WIDGET;
    w.inactive.weak_bg_fill = WIDGET;
    w.inactive.fg_stroke = Stroke::new(1.0_f32, TEXT);
    w.hovered.bg_fill = WIDGET_HOVER;
    w.hovered.weak_bg_fill = WIDGET_HOVER;
    w.hovered.bg_stroke = Stroke::new(1.0_f32, ACCENT);
    w.hovered.fg_stroke = Stroke::new(1.5_f32, Color32::WHITE);
    w.active.bg_fill = ACCENT;
    w.active.weak_bg_fill = ACCENT;
    w.active.fg_stroke = Stroke::new(1.5_f32, Color32::WHITE);
    w.open.bg_fill = WIDGET_HOVER;
    w.open.weak_bg_fill = WIDGET_HOVER;
    w.open.fg_stroke = Stroke::new(1.0_f32, Color32::WHITE);
    ctx.set_visuals(v);
    ctx.style_mut(|s| {
        s.spacing.item_spacing = egui::vec2(8.0, 7.0);
        s.spacing.interact_size.y = 24.0;
        s.spacing.slider_width = 160.0;
        s.spacing.combo_width = COMBO_W;
    });
}

fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(12.0);
    ui.label(RichText::new(title).size(12.5).strong().color(LAVENDER));
    ui.add_space(2.0);
}

fn grid(ui: &mut egui::Ui, id: &str, body: impl FnOnce(&mut egui::Ui)) {
    egui::Grid::new(id)
        .num_columns(2)
        .spacing([14.0, 8.0])
        .min_col_width(96.0)
        .show(ui, body);
}

fn choice<T: Copy + PartialEq>(ui: &mut egui::Ui, id: &str, value: &mut T, all: &[T], label: fn(T) -> &'static str) {
    egui::ComboBox::from_id_salt(id)
        .width(COMBO_W)
        .selected_text(label(*value))
        .show_ui(ui, |ui| {
            for &v in all {
                ui.selectable_value(value, v, label(v));
            }
        });
}

/// Keeps long device names from stretching a dropdown.
fn short(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max - 1).collect();
        format!("{}…", cut.trim_end())
    }
}

fn weak(ui: &mut egui::Ui, text: impl Into<String>) {
    ui.label(RichText::new(text.into()).size(11.5).color(WEAK));
}

fn hotkey_button(ui: &mut egui::Ui, key: Hotkey, target: HotkeyTarget, info: &Info, actions: &mut Vec<Action>) {
    let text = if info.capturing == Some(target) {
        RichText::new("Press keys…  (Esc cancels)").color(LAVENDER)
    } else {
        RichText::new(key.to_string())
    };
    if ui.add(egui::Button::new(text).min_size(egui::vec2(120.0, 24.0))).clicked() {
        actions.push(Action::CaptureHotkey(target));
    }
}

pub fn settings(ctx: &egui::Context, cfg: &mut Config, info: &Info, actions: &mut Vec<Action>) {
    egui::CentralPanel::default()
        .frame(egui::Frame::default().fill(BG).inner_margin(egui::Margin::same(16)))
        .show(ctx, |ui| {
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                header(ui, info);
                video_section(ui, cfg, info, actions);
                audio_section(ui, cfg, info);
                window_section(ui, cfg, info, actions);
                look_section(ui, cfg, actions);
                discord_section(ui, cfg, info);
                keys_section(ui, cfg, info, actions);
                ui.add_space(14.0);
                weak(ui, format!("Settings save automatically  ·  Starhowl {}", env!("CARGO_PKG_VERSION")));
                ui.add_space(4.0);
            });
        });
}

fn header(ui: &mut egui::Ui, info: &Info) {
    ui.horizontal(|ui| {
        if let Some(logo) = info.logo {
            ui.add(egui::Image::from_texture(egui::load::SizedTexture::new(logo.id(), egui::vec2(44.0, 44.0))));
        }
        ui.vertical(|ui| {
            ui.add_space(4.0);
            ui.label(RichText::new("Starhowl").size(21.0).strong().color(Color32::WHITE));
            weak(ui, "Settings");
        });
    });
}

fn video_section(ui: &mut egui::Ui, cfg: &mut Config, info: &Info, actions: &mut Vec<Action>) {
    section(ui, "VIDEO");
    grid(ui, "video", |ui| {
        ui.label("Device");
        ui.horizontal(|ui| {
            let selected = if cfg.video_device.is_empty() {
                if info.video.device.is_empty() {
                    "Auto".to_string()
                } else {
                    format!("Auto ({})", info.video.device)
                }
            } else {
                cfg.video_device.clone()
            };
            egui::ComboBox::from_id_salt("dev").width(COMBO_W - 72.0).selected_text(short(&selected, 26)).show_ui(ui, |ui| {
                if ui.selectable_label(cfg.video_device.is_empty(), "Auto (find the capture card)").clicked() {
                    cfg.video_device.clear();
                    cfg.video_link.clear();
                    cfg.mode = None;
                }
                for d in info.devices {
                    let on = !cfg.video_device.is_empty() && d.link == cfg.video_link;
                    if ui.selectable_label(on, &d.name).clicked() && !on {
                        cfg.video_device = d.name.clone();
                        cfg.video_link = d.link.clone();
                        cfg.mode = None;
                    }
                }
            });
            if ui.button("Rescan").on_hover_text("Look for devices again").clicked() {
                actions.push(Action::RefreshDevices);
            }
        });
        ui.end_row();

        ui.label("Mode");
        let auto_label = match &info.video.active {
            Some(m) if cfg.mode.is_none() => format!("Auto ({})", m.label()),
            _ => "Auto (best)".to_string(),
        };
        let selected = match &cfg.mode {
            None => auto_label.clone(),
            Some(s) => format!("{}x{} · {:.0} fps · {}", s.width, s.height, s.fps_num as f32 / s.fps_den.max(1) as f32, s.format),
        };
        egui::ComboBox::from_id_salt("mode").width(COMBO_W).selected_text(selected).show_ui(ui, |ui| {
            if ui.selectable_label(cfg.mode.is_none(), "Auto (best)").clicked() {
                cfg.mode = None;
            }
            for m in &info.video.modes {
                let sel = m.to_sel();
                let on = cfg.mode.as_ref() == Some(&sel);
                if ui.selectable_label(on, m.label()).clicked() {
                    cfg.mode = Some(sel);
                }
            }
        });
        ui.end_row();

        ui.label("");
        let status = if info.video.running {
            let out = info.video.output.map(|o| o.name()).unwrap_or("?");
            let line = format!("Receiving {:.0} fps  ·  decoded to {out}", info.fps);
            if info.video.state == "Running" {
                line
            } else {
                format!("{}\n{line}", info.video.state)
            }
        } else {
            info.video.state.clone()
        };
        ui.add(egui::Label::new(RichText::new(status).size(11.5).color(WEAK)).wrap());
        ui.end_row();

        ui.label("Scaling");
        choice(ui, "scaling", &mut cfg.scaling, Scaling::ALL, Scaling::label);
        ui.end_row();

        ui.label("Position");
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                for &v in AlignX::ALL {
                    ui.selectable_value(&mut cfg.align_x, v, v.label());
                }
            });
            ui.horizontal(|ui| {
                for &v in AlignY::ALL {
                    ui.selectable_value(&mut cfg.align_y, v, v.label());
                }
            });
        });
        ui.end_row();

        ui.label("");
        weak(ui, "Push the picture to a side to make room for overlays.");
        ui.end_row();

        ui.label("Filter");
        choice(ui, "filter", &mut cfg.filter, Filter::ALL, Filter::label);
        ui.end_row();

        ui.label("Color range");
        choice(ui, "range", &mut cfg.color_range, ColorRange::ALL, ColorRange::label);
        ui.end_row();

        ui.label("Color matrix");
        choice(ui, "matrix", &mut cfg.color_matrix, ColorMatrix::ALL, ColorMatrix::label);
        ui.end_row();
    });
}

fn endpoint_name<'a>(list: &'a [Endpoint], id: &str) -> Option<&'a str> {
    list.iter().find(|e| e.id == id).map(|e| e.name.as_str())
}

fn audio_section(ui: &mut egui::Ui, cfg: &mut Config, info: &Info) {
    section(ui, "AUDIO");
    grid(ui, "audio", |ui| {
        ui.label("Input");
        let selected = match cfg.audio_in.as_str() {
            "auto" => format!("Auto ({})", info.auto_input.unwrap_or("none found")),
            "off" => "Off".to_string(),
            id => endpoint_name(info.inputs, id).unwrap_or("(not connected)").to_string(),
        };
        egui::ComboBox::from_id_salt("ain").width(COMBO_W).selected_text(short(&selected, 34)).show_ui(ui, |ui| {
            ui.selectable_value(&mut cfg.audio_in, "auto".to_string(), "Auto (match the video device)");
            ui.selectable_value(&mut cfg.audio_in, "off".to_string(), "Off");
            for e in info.inputs {
                ui.selectable_value(&mut cfg.audio_in, e.id.clone(), &e.name);
            }
        });
        ui.end_row();

        ui.label("Output");
        let selected = if cfg.audio_out.is_empty() {
            "System default".to_string()
        } else {
            endpoint_name(info.outputs, &cfg.audio_out).unwrap_or("(not connected)").to_string()
        };
        egui::ComboBox::from_id_salt("aout").width(COMBO_W).selected_text(short(&selected, 34)).show_ui(ui, |ui| {
            ui.selectable_value(&mut cfg.audio_out, String::new(), "System default");
            for e in info.outputs {
                ui.selectable_value(&mut cfg.audio_out, e.id.clone(), &e.name);
            }
        });
        ui.end_row();

        ui.label("Volume");
        ui.horizontal(|ui| {
            let mut pct = (cfg.volume * 100.0).round();
            if ui.add(egui::Slider::new(&mut pct, 0.0..=100.0).suffix("%").fixed_decimals(0)).changed() {
                cfg.volume = pct / 100.0;
            }
            ui.checkbox(&mut cfg.muted, "Mute");
        });
        ui.end_row();

        ui.label("Input level");
        ui.add(egui::ProgressBar::new(info.level.clamp(0.0, 1.0)).desired_width(COMBO_W));
        ui.end_row();

        ui.label("Audio delay");
        ui.add(egui::Slider::new(&mut cfg.audio_delay_ms, 0..=500).suffix(" ms"))
            .on_hover_text("Delay the sound if it plays ahead of the picture.");
        ui.end_row();

        ui.label("");
        ui.checkbox(&mut cfg.ms2109_audio_fix, "Fix MS2109 stereo (when detected)")
            .on_hover_text(
                "Cheap MS2109 HDMI dongles send stereo 48 kHz sound but tell Windows it is\n\
                 mono 96 kHz, so it plays as mono. When an input reports exactly mono 96 kHz,\n\
                 Starhowl splits it back into left and right. Other inputs are not touched.",
            );
        ui.end_row();

        ui.label("");
        ui.add(egui::Label::new(RichText::new(info.audio_status).size(11.5).color(WEAK)).wrap());
        ui.end_row();
    });
}

fn window_section(ui: &mut egui::Ui, cfg: &mut Config, info: &Info, actions: &mut Vec<Action>) {
    section(ui, "WINDOW");
    grid(ui, "window", |ui| {
        ui.label("Style");
        ui.horizontal(|ui| {
            ui.checkbox(&mut cfg.borderless, "Borderless");
            ui.checkbox(&mut cfg.always_on_top, "Always on top");
        });
        ui.end_row();

        ui.label("");
        ui.checkbox(&mut cfg.hide_cursor, "Hide the mouse pointer when idle");
        ui.end_row();

        ui.label("Size");
        let (ww, wh) = info.win_size;
        egui::ComboBox::from_id_salt("size")
            .width(COMBO_W)
            .selected_text(format!("{ww} x {wh}"))
            .show_ui(ui, |ui| {
                let mut presets: Vec<(String, u32, u32)> = Vec::new();
                if let Some((fw, fh)) = info.frame_size {
                    presets.push((format!("Match video ({fw} x {fh})"), fw, fh));
                    presets.push((format!("Half video ({} x {})", fw / 2, fh / 2), fw / 2, fh / 2));
                }
                for (w, h) in [(1280, 720), (1600, 900), (1920, 1080), (2560, 1440), (3840, 2160)] {
                    presets.push((format!("{w} x {h}"), w, h));
                }
                for (label, w, h) in presets {
                    if ui.selectable_label(ww == w && wh == h, label).clicked() {
                        actions.push(Action::WindowSize(w, h));
                    }
                }
            });
        ui.end_row();

        ui.label("");
        weak(ui, "Any size: hold Ctrl+Shift+Alt and drag the window.");
        ui.end_row();
    });
}

fn look_section(ui: &mut egui::Ui, cfg: &mut Config, actions: &mut Vec<Action>) {
    section(ui, "LOOK");
    grid(ui, "look", |ui| {
        ui.label("Background");
        choice(ui, "bg", &mut cfg.background, Fill::ALL, Fill::label);
        ui.end_row();

        for (name, id, zone) in [
            ("Left side", "zl", &mut cfg.left),
            ("Right side", "zr", &mut cfg.right),
            ("Top", "zt", &mut cfg.top),
            ("Bottom", "zb", &mut cfg.bottom),
        ] {
            ui.label(name);
            choice(ui, id, zone, Zone::ALL, Zone::label);
            ui.end_row();
        }

        ui.label("Frame");
        choice(ui, "frame", &mut cfg.frame, FrameColor::ALL, FrameColor::label);
        ui.end_row();

        if cfg.frame != FrameColor::None {
            ui.label("Frame size");
            ui.add(egui::Slider::new(&mut cfg.frame_px, 1..=32).suffix(" px"));
            ui.end_row();
        }

        ui.label("Stars");
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("stars")
                .width(120.0)
                .selected_text(cfg.stars.label())
                .show_ui(ui, |ui| {
                    for &s in Stars::ALL {
                        ui.selectable_value(&mut cfg.stars, s, s.label());
                    }
                });
            ui.checkbox(&mut cfg.twinkle, "Twinkle");
        });
        ui.end_row();

        ui.label("Custom image");
        ui.horizontal(|ui| {
            if ui.button("Choose…").clicked() {
                actions.push(Action::PickImage);
            }
            if !cfg.image_path.is_empty() && ui.button("Clear").clicked() {
                cfg.image_path.clear();
            }
        });
        ui.end_row();

        if !cfg.image_path.is_empty() {
            ui.label("");
            let name = std::path::Path::new(&cfg.image_path)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            ui.add(egui::Label::new(RichText::new(name).size(11.5).color(WEAK)).truncate())
                .on_hover_text(&cfg.image_path);
            ui.end_row();

            ui.label("Image fit");
            choice(ui, "imgfit", &mut cfg.image_fit, ImageFit::ALL, ImageFit::label);
            ui.end_row();

            ui.label("Dim image");
            let mut pct = (cfg.image_dim * 100.0).round();
            if ui.add(egui::Slider::new(&mut pct, 0.0..=90.0).suffix("%").fixed_decimals(0)).changed() {
                cfg.image_dim = pct / 100.0;
            }
            ui.end_row();
        }
    });
}

fn text_box(ui: &mut egui::Ui, value: &mut String, hint: &str, width: f32, enabled: bool, limit: usize) {
    ui.add_enabled(
        enabled,
        egui::TextEdit::singleline(value).hint_text(hint).desired_width(width).char_limit(limit),
    );
}

const WARN: Color32 = Color32::from_rgb(0xF0, 0xB2, 0x5A);

/// A short note under a field that Discord would ignore as typed.
fn warn(ui: &mut egui::Ui, text: &str) {
    ui.label("");
    ui.add(egui::Label::new(RichText::new(text).size(11.5).color(WARN)).wrap());
    ui.end_row();
}

/// Discord silently drops 1-character text.
fn too_short(s: &str) -> bool {
    s.trim().chars().count() == 1
}

/// A grey explanation under a field (not a problem, just how it works).
fn note(ui: &mut egui::Ui, text: &str) {
    ui.label("");
    ui.add(egui::Label::new(RichText::new(text).size(11.5).color(WEAK)).wrap());
    ui.end_row();
}

/// True when `t` would repeat one of the lines already shown (so Starhowl
/// leaves it out: phones show hover texts as extra lines).
fn repeats(t: &str, others: &[&str]) -> bool {
    let t = t.trim();
    t.chars().count() >= 2 && others.iter().any(|o| o.trim().eq_ignore_ascii_case(t))
}

const REPEAT_NOTE: &str = "Same words as a line above, so it's left out. Phones would show it twice.";

fn discord_section(ui: &mut egui::Ui, cfg: &mut Config, info: &Info) {
    use crate::discord::{is_link, is_picture, MAX_BUTTON, MAX_TEXT};
    section(ui, "DISCORD");
    grid(ui, "discord", |ui| {
        ui.label("Rich Presence");
        ui.checkbox(&mut cfg.discord_enabled, "Show on my Discord profile");
        ui.end_row();

        ui.label("");
        ui.add(egui::Label::new(RichText::new(info.discord_status).size(11.5).color(WEAK)).wrap());
        ui.end_row();

        let on = cfg.discord_enabled;

        ui.label("Shown as");
        ui.add_enabled_ui(on, |ui| choice(ui, "dkind", &mut cfg.discord_kind, DiscordKind::ALL, DiscordKind::label));
        ui.end_row();

        ui.label("Top text");
        text_box(ui, &mut cfg.discord_title, DEFAULT_TITLE, COMBO_W, on, MAX_TEXT);
        ui.end_row();
        if on && too_short(&cfg.discord_title) {
            warn(ui, "Needs 2+ characters. Until then Discord shows \"StarHowl\".");
        }

        ui.label("Middle text");
        text_box(ui, &mut cfg.discord_details, "Capturing Xbox", COMBO_W, on, MAX_TEXT);
        ui.end_row();
        if on && too_short(&cfg.discord_details) {
            warn(ui, "Needs 2+ characters to show.");
        }

        ui.label("Bottom text");
        ui.horizontal(|ui| {
            ui.add_enabled(on, egui::Checkbox::without_text(&mut cfg.discord_extra_on));
            text_box(ui, &mut cfg.discord_extra, "Your text", COMBO_W - 30.0, on && cfg.discord_extra_on, MAX_TEXT);
        });
        ui.end_row();
        if on && cfg.discord_extra_on && too_short(&cfg.discord_extra) {
            warn(ui, "Needs 2+ characters to show.");
        }

        ui.label("Big picture");
        text_box(ui, &mut cfg.discord_image, "blank = galaxy wolf", COMBO_W, on, 512);
        ui.end_row();
        if on && !cfg.discord_image.trim().is_empty() && !is_picture(&cfg.discord_image) {
            warn(ui, "Use an https:// image link. Showing the galaxy wolf until then.");
        }

        ui.label("Big text");
        text_box(ui, &mut cfg.discord_large_text, "Shows when hovering the big picture", COMBO_W, on, MAX_TEXT);
        ui.end_row();
        let bottom = if cfg.discord_extra_on { cfg.discord_extra.as_str() } else { "" };
        if on && too_short(&cfg.discord_large_text) {
            warn(ui, "Needs 2+ characters to show.");
        } else if on && repeats(&cfg.discord_large_text, &[&cfg.discord_title, &cfg.discord_details, bottom]) {
            note(ui, REPEAT_NOTE);
        }

        let small_on = on && cfg.discord_small_on;
        ui.label("Small picture");
        ui.horizontal(|ui| {
            ui.add_enabled(on, egui::Checkbox::without_text(&mut cfg.discord_small_on));
            text_box(ui, &mut cfg.discord_small_image, "blank = StarHowl badge", COMBO_W - 30.0, small_on, 512);
        });
        ui.end_row();
        if small_on && !cfg.discord_small_image.trim().is_empty() && !is_picture(&cfg.discord_small_image) {
            warn(ui, "Use an https:// image link. Showing the StarHowl badge until then.");
        }

        ui.label("Small text");
        text_box(ui, &mut cfg.discord_small_text, "Shows when hovering the small picture", COMBO_W, small_on, MAX_TEXT);
        ui.end_row();
        if small_on && too_short(&cfg.discord_small_text) {
            warn(ui, "Needs 2+ characters to show.");
        } else if small_on
            && repeats(
                &cfg.discord_small_text,
                &[&cfg.discord_title, &cfg.discord_details, bottom, &cfg.discord_large_text],
            )
        {
            note(ui, REPEAT_NOTE);
        }

        ui.label("Button");
        ui.horizontal(|ui| {
            ui.add_enabled(on, egui::Checkbox::without_text(&mut cfg.discord_button_on));
            text_box(
                ui,
                &mut cfg.discord_button_label,
                "Button text",
                COMBO_W - 30.0,
                on && cfg.discord_button_on,
                MAX_BUTTON,
            );
        });
        ui.end_row();

        if cfg.discord_button_on {
            ui.label("Button link");
            text_box(ui, &mut cfg.discord_button_url, "https://twitch.tv/you", COMBO_W, on, 512);
            ui.end_row();
            if on && cfg.discord_button_label.trim().is_empty() {
                warn(ui, "The button needs text to show.");
            } else if on && !is_link(&cfg.discord_button_url) {
                warn(ui, "Needs a full link like https://twitch.tv/you. The button stays hidden until then.");
            }
        }

        ui.label("Timer");
        ui.add_enabled(on, egui::Checkbox::new(&mut cfg.discord_elapsed, "Show time elapsed"));
        ui.end_row();

        ui.label("App ID");
        text_box(ui, &mut cfg.discord_app_id, DISCORD_APP_ID, COMBO_W, on, 24);
        ui.end_row();
        if on && !cfg.discord_app_id.trim().chars().all(|c| c.is_ascii_digit()) {
            warn(ui, "Must be numbers only.");
        }
    });
    discord_preview(ui, cfg, info);
}

/// A mock-up of how the activity looks on a Discord profile.
fn discord_preview(ui: &mut egui::Ui, cfg: &Config, info: &Info) {
    use crate::discord::{is_link, MAX_BUTTON, MAX_TEXT};
    const CARD: Color32 = Color32::from_rgb(0x23, 0x24, 0x28);
    const MUTED: Color32 = Color32::from_rgb(0xB5, 0xBA, 0xC1);
    const BODY: Color32 = Color32::from_rgb(0xDB, 0xDE, 0xE1);
    // Exactly what Discord would show for a field (or nothing).
    let shown = |s: &str| {
        let t = s.trim();
        (t.chars().count() >= 2).then(|| t.chars().take(MAX_TEXT).collect::<String>())
    };
    let bottom = if cfg.discord_extra_on { cfg.discord_extra.as_str() } else { "" };
    ui.add_space(8.0);
    weak(ui, "Preview (how friends will see it)");
    egui::Frame::default()
        .fill(CARD)
        .corner_radius(8)
        .inner_margin(egui::Margin::same(12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(cfg.discord_kind.label().to_uppercase()).size(10.5).strong().color(MUTED));
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                if let Some(art) = info.discord_art {
                    let big = ui.add(
                        egui::Image::from_texture(egui::load::SizedTexture::new(art.id(), egui::vec2(72.0, 72.0)))
                            .corner_radius(8),
                    );
                    let mut on_badge = false;
                    if let (true, Some(badge)) = (cfg.discord_small_on, info.discord_badge) {
                        let c = big.rect.right_bottom() - egui::vec2(4.0, 4.0);
                        let r = egui::Rect::from_center_size(c, egui::vec2(24.0, 24.0));
                        ui.painter().circle_filled(c, 14.0, CARD);
                        ui.painter().image(
                            badge.id(),
                            r,
                            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                            Color32::WHITE,
                        );
                        let hover = ui.interact(r, ui.id().with("discord-badge"), egui::Sense::hover());
                        on_badge = hover.hovered();
                        if let Some(t) = shown(&cfg.discord_small_text).filter(|t| !repeats(t, &[&cfg.discord_title, &cfg.discord_details, bottom, &cfg.discord_large_text])) {
                            hover.on_hover_text(t);
                        }
                    }
                    if !on_badge {
                        if let Some(t) = shown(&cfg.discord_large_text).filter(|t| !repeats(t, &[&cfg.discord_title, &cfg.discord_details, bottom])) {
                            big.on_hover_text(t);
                        }
                    }
                }
                ui.vertical(|ui| {
                    ui.add_space(2.0);
                    let title = shown(&cfg.discord_title).unwrap_or_else(|| "StarHowl".into());
                    ui.label(RichText::new(title).size(14.0).strong().color(Color32::WHITE));
                    if let Some(t) = shown(&cfg.discord_details) {
                        ui.label(RichText::new(t).size(12.5).color(BODY));
                    }
                    if cfg.discord_extra_on {
                        if let Some(t) = shown(&cfg.discord_extra) {
                            ui.label(RichText::new(t).size(12.5).color(BODY));
                        }
                    }
                    if cfg.discord_elapsed {
                        let s = info.elapsed;
                        let t = format!("{:02}:{:02}:{:02} elapsed", s / 3600, s / 60 % 60, s % 60);
                        ui.label(RichText::new(t).size(12.0).color(MUTED));
                    }
                });
            });
            let label: String = cfg.discord_button_label.trim().chars().take(MAX_BUTTON).collect();
            if cfg.discord_button_on && !label.is_empty() && is_link(&cfg.discord_button_url) {
                ui.add_space(6.0);
                let b = egui::Button::new(RichText::new(label).color(Color32::WHITE))
                    .fill(Color32::from_rgb(0x4E, 0x50, 0x58));
                ui.add_sized([ui.available_width(), 28.0], b);
            }
        });
    weak(ui, "Hover the pictures to see the big and small text.");
    weak(ui, "Friends see the button. Discord hides it on your own profile.");
}

fn keys_section(ui: &mut egui::Ui, cfg: &mut Config, info: &Info, actions: &mut Vec<Action>) {
    section(ui, "SHORTCUTS");
    grid(ui, "keys", |ui| {
        ui.label("Settings");
        hotkey_button(ui, cfg.key_settings, HotkeyTarget::Settings, info, actions);
        ui.end_row();
        ui.label("Fullscreen");
        hotkey_button(ui, cfg.key_fullscreen, HotkeyTarget::Fullscreen, info, actions);
        ui.end_row();
        ui.label("Mute");
        hotkey_button(ui, cfg.key_mute, HotkeyTarget::Mute, info, actions);
        ui.end_row();
        ui.label("Resize");
        weak(ui, "Hold Ctrl+Shift+Alt and drag");
        ui.end_row();
        ui.label("Move");
        weak(ui, "Drag anywhere on the video");
        ui.end_row();
        ui.label("Also");
        weak(ui, "Double-click: fullscreen  ·  Wheel: volume");
        ui.end_row();
    });
}

/// Things drawn on top of the video.
pub struct Overlay<'a> {
    pub no_signal: Option<&'a str>,
    pub readout: Option<&'a str>,
    pub toast: Option<&'a str>,
    pub logo: Option<&'a TextureHandle>,
    pub settings_key: String,
}

fn pill(ui: &mut egui::Ui, text: &str) {
    egui::Frame::default()
        .fill(Color32::from_black_alpha(170))
        .corner_radius(10)
        .inner_margin(egui::Margin::symmetric(14, 7))
        .show(ui, |ui| {
            ui.label(RichText::new(text).size(17.0).color(Color32::WHITE));
        });
}

pub fn overlay(ctx: &egui::Context, o: &Overlay) {
    let screen = ctx.screen_rect();
    if let Some(status) = o.no_signal {
        egui::Area::new(egui::Id::new("nosignal"))
            .anchor(Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .interactable(false)
            .show(ctx, |ui| {
                ui.set_max_width((screen.width() * 0.8).min(560.0));
                ui.vertical_centered(|ui| {
                    if let Some(logo) = o.logo {
                        let s = (screen.height() * 0.22).clamp(56.0, 200.0);
                        ui.add(egui::Image::from_texture(egui::load::SizedTexture::new(logo.id(), egui::vec2(s, s))));
                        ui.add_space(10.0);
                    }
                    ui.label(RichText::new("No signal").size(24.0).color(Color32::WHITE));
                    ui.add_space(4.0);
                    ui.label(RichText::new(status).size(13.5).color(LAVENDER));
                    ui.add_space(10.0);
                    ui.label(RichText::new(format!("{} opens settings", o.settings_key)).size(12.5).color(WEAK));
                });
            });
    }
    if let Some(text) = o.readout {
        egui::Area::new(egui::Id::new("readout"))
            .anchor(Align2::CENTER_TOP, egui::vec2(0.0, 18.0))
            .interactable(false)
            .show(ctx, |ui| pill(ui, text));
    }
    if let Some(text) = o.toast {
        egui::Area::new(egui::Id::new("toast"))
            .anchor(Align2::CENTER_BOTTOM, egui::vec2(0.0, -28.0))
            .interactable(false)
            .show(ctx, |ui| pill(ui, text));
    }
}
