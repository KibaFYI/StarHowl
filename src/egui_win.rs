//! Runs egui inside a plain Win32 window and draws it with Direct3D 11.
//! (A small painter of our own keeps the app free of a second graphics stack.)

use crate::gfx::Gfx;
use egui::{epaint::Primitive, ClippedPrimitive, Pos2, TextureId, TexturesDelta};
use std::collections::HashMap;
use std::time::{Duration, Instant};
use windows::core::{Result, PCSTR};
use windows::Win32::Foundation::{HANDLE, HGLOBAL, HWND, LPARAM, RECT, WPARAM};
use windows::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard, SetClipboardData};
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::Graphics::Direct3D::D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

const EGUI_HLSL: &str = include_str!("shaders/egui.hlsl");

struct Tex {
    tex: ID3D11Texture2D,
    srv: ID3D11ShaderResourceView,
    nearest: bool,
}

pub struct Painter {
    vs: ID3D11VertexShader,
    ps: ID3D11PixelShader,
    layout: ID3D11InputLayout,
    cb: ID3D11Buffer,
    blend: ID3D11BlendState,
    raster: ID3D11RasterizerState,
    lin: ID3D11SamplerState,
    pt: ID3D11SamplerState,
    vb: Option<(ID3D11Buffer, usize)>,
    ib: Option<(ID3D11Buffer, usize)>,
    textures: HashMap<TextureId, Tex>,
}

impl Painter {
    pub fn new(gfx: &Gfx) -> Result<Painter> {
        let vs_bytes = gfx.compile(EGUI_HLSL, "vs_egui", "vs_4_0")?;
        let vs = gfx.vertex_shader(&vs_bytes)?;
        let ps = gfx.pixel_shader(EGUI_HLSL, "ps_egui")?;
        let elems = [
            input_elem(b"POSITION\0", DXGI_FORMAT_R32G32_FLOAT, 0),
            input_elem(b"TEXCOORD\0", DXGI_FORMAT_R32G32_FLOAT, 8),
            input_elem(b"COLOR\0", DXGI_FORMAT_R8G8B8A8_UNORM, 16),
        ];
        let mut layout = None;
        unsafe { gfx.device.CreateInputLayout(&elems, &vs_bytes, Some(&mut layout))? };

        // egui hands us premultiplied colours.
        let mut bd = D3D11_BLEND_DESC::default();
        bd.RenderTarget[0] = D3D11_RENDER_TARGET_BLEND_DESC {
            BlendEnable: true.into(),
            SrcBlend: D3D11_BLEND_ONE,
            DestBlend: D3D11_BLEND_INV_SRC_ALPHA,
            BlendOp: D3D11_BLEND_OP_ADD,
            SrcBlendAlpha: D3D11_BLEND_ONE,
            DestBlendAlpha: D3D11_BLEND_INV_SRC_ALPHA,
            BlendOpAlpha: D3D11_BLEND_OP_ADD,
            RenderTargetWriteMask: D3D11_COLOR_WRITE_ENABLE_ALL.0 as u8,
        };
        let mut blend = None;
        unsafe { gfx.device.CreateBlendState(&bd, Some(&mut blend))? };

        let rd = D3D11_RASTERIZER_DESC {
            FillMode: D3D11_FILL_SOLID,
            CullMode: D3D11_CULL_NONE,
            ScissorEnable: true.into(),
            DepthClipEnable: true.into(),
            ..Default::default()
        };
        let mut raster = None;
        unsafe { gfx.device.CreateRasterizerState(&rd, Some(&mut raster))? };

        Ok(Painter {
            vs,
            ps,
            layout: layout.unwrap(),
            cb: gfx.dynamic_cbuffer(16)?,
            blend: blend.unwrap(),
            raster: raster.unwrap(),
            lin: gfx.sampler(D3D11_FILTER_MIN_MAG_MIP_LINEAR)?,
            pt: gfx.sampler(D3D11_FILTER_MIN_MAG_MIP_POINT)?,
            vb: None,
            ib: None,
            textures: HashMap::new(),
        })
    }

    fn update_textures(&mut self, gfx: &Gfx, delta: &TexturesDelta) {
        for (id, d) in &delta.set {
            let (size, pixels): ([usize; 2], Vec<u8>) = match &d.image {
                egui::ImageData::Color(img) => {
                    (img.size, img.pixels.iter().flat_map(|c| c.to_array()).collect())
                }
                egui::ImageData::Font(font) => {
                    (font.size, font.srgba_pixels(None).flat_map(|c| c.to_array()).collect())
                }
            };
            let nearest = d.options.magnification == egui::TextureFilter::Nearest;
            match (d.pos, self.textures.get_mut(id)) {
                (Some([x, y]), Some(t)) => unsafe {
                    let b = D3D11_BOX {
                        left: x as u32,
                        top: y as u32,
                        front: 0,
                        right: (x + size[0]) as u32,
                        bottom: (y + size[1]) as u32,
                        back: 1,
                    };
                    gfx.ctx.UpdateSubresource(&t.tex, 0, Some(&b), pixels.as_ptr() as *const _, size[0] as u32 * 4, 0);
                    t.nearest = nearest;
                },
                _ => {
                    if let Ok((tex, srv)) = gfx.texture(
                        size[0] as u32,
                        size[1] as u32,
                        DXGI_FORMAT_R8G8B8A8_UNORM,
                        D3D11_USAGE_DEFAULT,
                        D3D11_BIND_SHADER_RESOURCE.0 as u32,
                        0,
                        Some(&D3D11_SUBRESOURCE_DATA {
                            pSysMem: pixels.as_ptr() as *const _,
                            SysMemPitch: size[0] as u32 * 4,
                            SysMemSlicePitch: 0,
                        }),
                    ) {
                        self.textures.insert(*id, Tex { tex, srv, nearest });
                    }
                }
            }
        }
    }

    fn ensure_buffer(gfx: &Gfx, slot: &mut Option<(ID3D11Buffer, usize)>, bytes: usize, bind: D3D11_BIND_FLAG) -> Option<ID3D11Buffer> {
        if !matches!(slot, Some((_, cap)) if *cap >= bytes) {
            let cap = bytes.next_power_of_two().max(4096);
            let desc = D3D11_BUFFER_DESC {
                ByteWidth: cap as u32,
                Usage: D3D11_USAGE_DYNAMIC,
                BindFlags: bind.0 as u32,
                CPUAccessFlags: D3D11_CPU_ACCESS_WRITE.0 as u32,
                MiscFlags: 0,
                StructureByteStride: 0,
            };
            let mut b = None;
            unsafe { gfx.device.CreateBuffer(&desc, None, Some(&mut b)).ok()? };
            *slot = Some((b?, cap));
        }
        slot.as_ref().map(|(b, _)| b.clone())
    }

    /// Draws egui's output on top of whatever is in the bound render target.
    pub fn paint(&mut self, gfx: &Gfx, size_px: [u32; 2], ppp: f32, prims: &[ClippedPrimitive], delta: &TexturesDelta) {
        self.update_textures(gfx, delta);

        let meshes: Vec<(&egui::Rect, &egui::Mesh)> = prims
            .iter()
            .filter_map(|p| match &p.primitive {
                Primitive::Mesh(m) if !m.indices.is_empty() => Some((&p.clip_rect, m)),
                _ => None,
            })
            .collect();
        if !meshes.is_empty() {
            self.draw_meshes(gfx, size_px, ppp, &meshes);
        }

        for id in &delta.free {
            self.textures.remove(id);
        }
    }

    fn draw_meshes(&mut self, gfx: &Gfx, size_px: [u32; 2], ppp: f32, meshes: &[(&egui::Rect, &egui::Mesh)]) {
        let vcount: usize = meshes.iter().map(|(_, m)| m.vertices.len()).sum();
        let icount: usize = meshes.iter().map(|(_, m)| m.indices.len()).sum();
        let vstride = std::mem::size_of::<egui::epaint::Vertex>();
        let Some(vb) = Self::ensure_buffer(gfx, &mut self.vb, vcount * vstride, D3D11_BIND_VERTEX_BUFFER) else { return };
        let Some(ib) = Self::ensure_buffer(gfx, &mut self.ib, icount * 4, D3D11_BIND_INDEX_BUFFER) else { return };

        unsafe {
            let c = &gfx.ctx;
            let mut m = D3D11_MAPPED_SUBRESOURCE::default();
            if c.Map(&vb, 0, D3D11_MAP_WRITE_DISCARD, 0, Some(&mut m)).is_err() {
                return;
            }
            let mut dst = m.pData as *mut u8;
            for (_, mesh) in meshes {
                let n = mesh.vertices.len() * vstride;
                std::ptr::copy_nonoverlapping(mesh.vertices.as_ptr() as *const u8, dst, n);
                dst = dst.add(n);
            }
            c.Unmap(&vb, 0);
            if c.Map(&ib, 0, D3D11_MAP_WRITE_DISCARD, 0, Some(&mut m)).is_err() {
                return;
            }
            let mut dst = m.pData as *mut u32;
            for (_, mesh) in meshes {
                std::ptr::copy_nonoverlapping(mesh.indices.as_ptr(), dst, mesh.indices.len());
                dst = dst.add(mesh.indices.len());
            }
            c.Unmap(&ib, 0);

            let pts = [size_px[0] as f32 / ppp, size_px[1] as f32 / ppp, 0.0, 0.0];
            gfx.write_buffer(&self.cb, &pts);

            c.IASetInputLayout(&self.layout);
            c.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            let stride = vstride as u32;
            let offset = 0u32;
            c.IASetVertexBuffers(0, 1, Some(&Some(vb.clone())), Some(&stride), Some(&offset));
            c.IASetIndexBuffer(&ib, DXGI_FORMAT_R32_UINT, 0);
            c.VSSetShader(&self.vs, None);
            c.VSSetConstantBuffers(0, Some(&[Some(self.cb.clone())]));
            c.PSSetShader(&self.ps, None);
            c.OMSetBlendState(&self.blend, None, 0xffff_ffff);
            c.RSSetState(&self.raster);

            let (mut vbase, mut ibase) = (0i32, 0u32);
            for (clip, mesh) in meshes {
                let x0 = (clip.min.x * ppp).round().clamp(0.0, size_px[0] as f32) as i32;
                let y0 = (clip.min.y * ppp).round().clamp(0.0, size_px[1] as f32) as i32;
                let x1 = (clip.max.x * ppp).round().clamp(0.0, size_px[0] as f32) as i32;
                let y1 = (clip.max.y * ppp).round().clamp(0.0, size_px[1] as f32) as i32;
                if x1 > x0 && y1 > y0 {
                    if let Some(t) = self.textures.get(&mesh.texture_id) {
                        c.RSSetScissorRects(Some(&[RECT { left: x0, top: y0, right: x1, bottom: y1 }]));
                        c.PSSetShaderResources(0, Some(&[Some(t.srv.clone())]));
                        let samp = if t.nearest { &self.pt } else { &self.lin };
                        c.PSSetSamplers(0, Some(&[Some(samp.clone())]));
                        c.DrawIndexed(mesh.indices.len() as u32, ibase, vbase);
                    }
                }
                vbase += mesh.vertices.len() as i32;
                ibase += mesh.indices.len() as u32;
            }
            c.RSSetState(None);
            c.OMSetBlendState(None, None, 0xffff_ffff);
            c.IASetInputLayout(None);
        }
    }
}

fn input_elem(name: &'static [u8], fmt: DXGI_FORMAT, offset: u32) -> D3D11_INPUT_ELEMENT_DESC {
    D3D11_INPUT_ELEMENT_DESC {
        SemanticName: PCSTR(name.as_ptr()),
        SemanticIndex: 0,
        Format: fmt,
        InputSlot: 0,
        AlignedByteOffset: offset,
        InputSlotClass: D3D11_INPUT_PER_VERTEX_DATA,
        InstanceDataStepRate: 0,
    }
}

/// egui state for one window: the context, pending input, and its painter.
pub struct EguiWindow {
    pub ctx: egui::Context,
    pub painter: Painter,
    events: Vec<egui::Event>,
    start: Instant,
    pub ppp: f32,
    pub cursor: egui::CursorIcon,
    pub tracking_leave: bool,
}

pub struct EguiFrame {
    pub prims: Vec<ClippedPrimitive>,
    pub delta: TexturesDelta,
    pub repaint_after: Duration,
    /// Text egui wants on the clipboard (Ctrl+C / Ctrl+X in a text box).
    pub copied: Option<String>,
}

const CF_UNICODETEXT: u32 = 13;

/// Reads text from the Windows clipboard.
pub fn clipboard_get() -> Option<String> {
    unsafe {
        OpenClipboard(None).ok()?;
        let text = (|| {
            let h = GetClipboardData(CF_UNICODETEXT).ok()?;
            let g = HGLOBAL(h.0);
            let p = GlobalLock(g) as *const u16;
            if p.is_null() {
                return None;
            }
            let mut n = 0;
            while *p.add(n) != 0 {
                n += 1;
            }
            let s = String::from_utf16_lossy(std::slice::from_raw_parts(p, n));
            let _ = GlobalUnlock(g);
            Some(s)
        })();
        let _ = CloseClipboard();
        text
    }
}

/// Puts text on the Windows clipboard (`owner` must be one of our windows).
pub fn clipboard_set(owner: HWND, text: &str) {
    unsafe {
        if OpenClipboard(Some(owner)).is_err() {
            return;
        }
        let _ = EmptyClipboard();
        let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
        if let Ok(g) = GlobalAlloc(GMEM_MOVEABLE, wide.len() * 2) {
            let p = GlobalLock(g) as *mut u16;
            if !p.is_null() {
                std::ptr::copy_nonoverlapping(wide.as_ptr(), p, wide.len());
                let _ = GlobalUnlock(g);
                let _ = SetClipboardData(CF_UNICODETEXT, Some(HANDLE(g.0)));
            }
        }
        let _ = CloseClipboard();
    }
}

pub fn modifiers() -> egui::Modifiers {
    let down = |vk: VIRTUAL_KEY| unsafe { GetKeyState(vk.0 as i32) } < 0;
    let ctrl = down(VK_CONTROL);
    egui::Modifiers { alt: down(VK_MENU), ctrl, shift: down(VK_SHIFT), mac_cmd: false, command: ctrl }
}

fn lparam_pos(l: LPARAM, ppp: f32) -> Pos2 {
    let x = (l.0 & 0xFFFF) as u16 as i16 as f32;
    let y = ((l.0 >> 16) & 0xFFFF) as u16 as i16 as f32;
    Pos2::new(x / ppp, y / ppp)
}

fn vk_to_key(vk: u16) -> Option<egui::Key> {
    use egui::Key;
    Some(match vk {
        0x08 => Key::Backspace,
        0x09 => Key::Tab,
        0x0D => Key::Enter,
        0x1B => Key::Escape,
        0x20 => Key::Space,
        0x21 => Key::PageUp,
        0x22 => Key::PageDown,
        0x23 => Key::End,
        0x24 => Key::Home,
        0x25 => Key::ArrowLeft,
        0x26 => Key::ArrowUp,
        0x27 => Key::ArrowRight,
        0x28 => Key::ArrowDown,
        0x2E => Key::Delete,
        0x41 => Key::A,
        0x43 => Key::C,
        0x56 => Key::V,
        0x58 => Key::X,
        0x5A => Key::Z,
        _ => return None,
    })
}

impl EguiWindow {
    pub fn new(gfx: &Gfx) -> Result<EguiWindow> {
        Ok(EguiWindow {
            ctx: egui::Context::default(),
            painter: Painter::new(gfx)?,
            events: Vec::new(),
            start: Instant::now(),
            ppp: 1.0,
            cursor: egui::CursorIcon::Default,
            tracking_leave: false,
        })
    }

    /// Feeds a window message to egui. Returns true if egui cares about it
    /// (so the window should repaint).
    pub fn on_message(&mut self, msg: u32, w: WPARAM, l: LPARAM) -> bool {
        let mods = modifiers();
        let ppp = self.ppp;
        let button = |b: egui::PointerButton, pressed: bool| egui::Event::PointerButton {
            pos: lparam_pos(l, ppp),
            button: b,
            pressed,
            modifiers: mods,
        };
        let ev = match msg {
            WM_MOUSEMOVE => egui::Event::PointerMoved(lparam_pos(l, ppp)),
            WM_MOUSELEAVE => {
                self.tracking_leave = false;
                egui::Event::PointerGone
            }
            WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => button(egui::PointerButton::Primary, true),
            WM_LBUTTONUP => button(egui::PointerButton::Primary, false),
            WM_RBUTTONDOWN | WM_RBUTTONDBLCLK => button(egui::PointerButton::Secondary, true),
            WM_RBUTTONUP => button(egui::PointerButton::Secondary, false),
            WM_MBUTTONDOWN => button(egui::PointerButton::Middle, true),
            WM_MBUTTONUP => button(egui::PointerButton::Middle, false),
            WM_MOUSEWHEEL | WM_MOUSEHWHEEL => {
                let d = ((w.0 >> 16) & 0xFFFF) as u16 as i16 as f32 / 120.0;
                let delta = if msg == WM_MOUSEWHEEL { egui::vec2(0.0, d) } else { egui::vec2(-d, 0.0) };
                egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Line, delta, modifiers: mods }
            }
            WM_CHAR => {
                let c = char::from_u32(w.0 as u32).unwrap_or('\0');
                if c.is_control() {
                    return false;
                }
                egui::Event::Text(c.to_string())
            }
            WM_KEYDOWN if mods.ctrl && !mods.alt && matches!(w.0 as u8, b'C' | b'X' | b'V') => match w.0 as u8 {
                b'C' => egui::Event::Copy,
                b'X' => egui::Event::Cut,
                _ => match clipboard_get() {
                    Some(t) => egui::Event::Paste(t),
                    None => return false,
                },
            },
            WM_KEYDOWN | WM_KEYUP | WM_SYSKEYDOWN | WM_SYSKEYUP => {
                let Some(key) = vk_to_key(w.0 as u16) else { return false };
                egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed: msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN,
                    repeat: false,
                    modifiers: mods,
                }
            }
            WM_KILLFOCUS => egui::Event::WindowFocused(false),
            WM_SETFOCUS => egui::Event::WindowFocused(true),
            _ => return false,
        };
        self.events.push(ev);
        true
    }

    /// Runs one egui frame for a window of `size_px` physical pixels.
    pub fn run(&mut self, size_px: [u32; 2], ui: impl FnMut(&egui::Context)) -> EguiFrame {
        let mut raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                Pos2::ZERO,
                egui::vec2(size_px[0] as f32 / self.ppp, size_px[1] as f32 / self.ppp),
            )),
            time: Some(self.start.elapsed().as_secs_f64()),
            modifiers: modifiers(),
            events: std::mem::take(&mut self.events),
            focused: true,
            max_texture_side: Some(8192),
            ..Default::default()
        };
        raw.viewports.entry(egui::ViewportId::ROOT).or_default().native_pixels_per_point = Some(self.ppp);
        let out = self.ctx.run(raw, ui);
        self.cursor = out.platform_output.cursor_icon;
        let repaint_after = out
            .viewport_output
            .get(&egui::ViewportId::ROOT)
            .map(|v| v.repaint_delay)
            .unwrap_or(Duration::MAX);
        let copied = out.platform_output.commands.iter().find_map(|c| match c {
            egui::OutputCommand::CopyText(t) => Some(t.clone()),
            _ => None,
        });
        let prims = self.ctx.tessellate(out.shapes, out.pixels_per_point);
        EguiFrame { prims, delta: out.textures_delta, repaint_after, copied }
    }

    pub fn paint(&mut self, gfx: &Gfx, size_px: [u32; 2], frame: &EguiFrame) {
        let ppp = self.ctx.pixels_per_point();
        self.painter.paint(gfx, size_px, ppp, &frame.prims, &frame.delta);
    }
}

/// The Windows cursor that matches what egui wants.
pub fn cursor_for(icon: egui::CursorIcon) -> PCWSTR16 {
    use egui::CursorIcon as C;
    match icon {
        C::PointingHand => IDC_HAND,
        C::Text => IDC_IBEAM,
        C::ResizeHorizontal | C::ResizeColumn | C::ResizeEast | C::ResizeWest => IDC_SIZEWE,
        C::ResizeVertical | C::ResizeRow | C::ResizeNorth | C::ResizeSouth => IDC_SIZENS,
        C::NotAllowed | C::NoDrop => IDC_NO,
        C::Wait | C::Progress => IDC_WAIT,
        C::Move | C::Grab | C::Grabbing | C::AllScroll => IDC_SIZEALL,
        _ => IDC_ARROW,
    }
}

pub type PCWSTR16 = windows::core::PCWSTR;
