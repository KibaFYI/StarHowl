<p align="center">
  <img src="assets/discord-large.png" width="140" alt="Starhowl wolf">
</p>

<h1 align="center">Starhowl</h1>

<p align="center">
  <b>See your console on your PC. Hear it too. That's it.</b><br>
  A tiny capture-card viewer for Windows.
</p>

---

## TL;DR

- Plug in your HDMI capture card → run `Starhowl.exe` → done.
- **Alt+S** opens settings.
- 3 MB. No installer. About 0.2% CPU.

## Start

1. Console → HDMI → capture card → USB → PC.
2. Grab `Starhowl.exe` from **[Releases](https://github.com/KibaFYI/StarHowl/releases)**.
3. Run it. It finds the card and its sound on its own.

## Controls

| Do this | Press |
| --- | --- |
| Settings | `Alt+S` |
| Fullscreen | `F11` or double-click |
| Leave fullscreen | `Esc` |
| Mute | `Ctrl+M` |
| Volume | Mouse wheel |
| Move the window | Drag the picture |
| Resize to any size | Hold `Ctrl+Shift+Alt` + drag |

You can change the Settings, Fullscreen and Mute keys in settings.

## What you get

**Picture**
- Finds your capture card by itself (not your webcam).
- Pick any mode the card offers. It shows the real fps you're getting.
- Scaling: Fit · Pixel-perfect · 1× · ½× · ⅓× · 2× · Stretch.

**Sound**
- Card audio straight to your speakers. About 30 ms behind.
- Volume, mute, and a delay slider if the sound runs ahead of the picture.
- Fixes cheap MS2109 dongles that sound mono.

**Looks**
- Borderless window, always on top, mouse hides when idle.
- Around the picture: black, white, deep purple galaxy, or your own image.
- Left, right, top and bottom can each be different. Optional frame.

**Discord**
- Customizable top and bottom text.
- Shows **Capturing** whatever you type (like your console).
- Optional button with your own link.
- No login. No token. Turn it on in Settings → Discord.

## Something wrong?

| Problem | Fix |
| --- | --- |
| "Device is busy" | Another app won't share the card. Close the Camera app or OBS. |
| Wrong device | Settings → Video → Device |
| Washed out or too dark | Settings → Video → Color range |
| Sound before picture | Settings → Audio → Audio delay |
| 1080p says 30 fps | That's your dongle's limit. Pick 720p for 60. |
| Start fresh | Delete `%APPDATA%\Starhowl\settings.toml` |

---

<details>
<summary><b>Build it yourself</b></summary>

You need:
- Windows 10 or 11
- [Rust](https://rustup.rs) (stable)
- Visual Studio Build Tools with "Desktop development with C++"

```
cargo build --release
```

Your exe: `target\release\starhowl.exe`

Linker error `LNK1104`? Your folder path is too long. Move the project
somewhere short like `C:\dev\starhowl`.

</details>

<details>
<summary><b>Diagnostics</b></summary>

```
starhowl.exe --probe
```
Lists every video device, its modes, and every audio device into the log.

```
starhowl.exe --record-audio sample.wav
```
Saves 4 seconds of the card's raw audio.

Log file: `%APPDATA%\Starhowl\starhowl.log`

</details>

<details>
<summary><b>How it works</b></summary>

| Part | Uses |
| --- | --- |
| Window + input | Win32 (`windows` crate) |
| Video | Media Foundation, newest frame only |
| Drawing | Direct3D 11 + small HLSL shaders |
| Audio | WASAPI, ~30 ms buffer |
| Settings | [egui](https://github.com/emilk/egui) with a tiny D3D11 painter |
| Images | Windows Imaging Component |
| Discord | Discord's local pipe (no login) |

</details>

---

BSD 2-Clause License. See [LICENSE](LICENSE).
