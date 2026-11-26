# Changelog

## v0.2.8
- Discord status hides itself after a minute with no picture (console off or
  unplugged) and comes back with the picture. Settings → Discord → No picture.
- "Quit Starhowl" button at the bottom of settings (the settings X only hides
  settings, which made it easy to think Starhowl had closed).
- Fixed: opening Starhowl a second time wiped the running copy's log file.

## v0.2.7
- Video position: Left / Center / Right and Top / Middle / Bottom (Settings →
  Video → Position), to leave room on one side for stream overlays.
- Discord on phones no longer shows the same line twice. Text that repeats a
  line already shown is left out (phones show hover texts as extra lines).
- `STARHOWL_CONFIG_DIR` environment variable: keep settings and log in another
  folder (portable use, or running a second copy).

## v0.2.6
- Fixed: the small purple badge didn't show on Discord (Discord's image cache
  held an old "not found"; the badge now uses a fresh link).

## v0.2.5
- Discord now shows "Watching Stream With StarHowl" by default.
- Fully customizable: top, middle and bottom text, big and small picture,
  big and small hover text, button text and button link.
- New small purple badge picture (assets/discord-small.png).
- Settings warn about anything Discord would ignore; bad picture links fall
  back to the built-in pictures instead of breaking the presence.
- Older settings carry over ("Capturing ___" moves into the new text boxes).

## v0.2.4
- Discord settings say "Top text" and "Bottom text".
- README: Discord section describes the customizable top and bottom text.

## v0.2.3
- README: no emojis, removed the purple Streaming badge section.

## v0.2.2
- Rewrote the README: shorter, straight to the point, easy to scan.

## v0.2.1
- Discord shows the galaxy wolf picture out of the box (served from the GitHub
  repository); the Image link box is only for using a different picture.
- Settings explain where the purple Streaming badge comes from (Discord's
  Twitch/YouTube connection, the same as with OBS).

## v0.2.0
- Discord Rich Presence: "Streaming With Starhowl", "Capturing ___", optional
  extra line, optional button (text + link), elapsed timer, galaxy wolf picture.
  Configured in Settings → Discord, with a live preview card.
- Text boxes in settings support copy, cut and paste.

## v0.1.0
- First release: capture-card video and audio passthrough, borderless window,
  free resize (Ctrl+Shift+Alt + drag), galaxy / black / white / custom-image
  borders, frame outline, MS2109 stereo fix, Alt+S settings popup.
