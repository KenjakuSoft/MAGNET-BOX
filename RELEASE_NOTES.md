MagnetBox **v0.1.3** — a big quality-of-life release. ✨

Friendlier to set up, nicer to live in, and smarter at grabbing your stuff.

## New
- **🚀 First-run setup wizard** — no more hunting for a password in a console. A fresh install opens a guided page where you create your own account (protected by a one-time setup code shown at startup).
- **📱 Installable app (PWA)** — "Add to Home Screen" and run MagnetBox in its own window like a native app, with mobile status-bar theming.
- **🌗 Light / dark theme** — a header toggle, remembered per device, applied before first paint (no flash). Follows your system by default.
- **🔔 Completion notifications** — set `MAGNETBOX_NOTIFY_URL` and get pinged the instant a torrent or download finishes (Discord, ntfy, or any webhook).
- **📥 Quick-add bookmarklet** — drag the button from your Account page to your bookmarks bar; one click on any torrent page sends the magnet straight to your box.
- **📡 RSS auto-download** — subscribe to feeds in **Admin → RSS** with an optional keyword filter; new matching items download automatically (every 10 min, skipping the backlog).
- **📊 Download health** — each torrent shows its live **peer count** next to speed and ETA, so a stuck torrent is obvious at a glance.

> Tip: install **ffmpeg** on the server to play *any* media format in the browser, and set `MAGNETBOX_NOTIFY_URL` for completion pings.

## Install — download & run (no setup)
- **Windows:** `magnetbox-windows-x64.exe` — double-click; your browser opens to the setup wizard.
- **macOS (Apple Silicon):** `magnetbox-macos-arm64`
- **Linux:** `magnetbox-linux-x64`

## Or run on a server (one command)
```bash
curl -fsSL https://raw.githubusercontent.com/KenjakuSoft/MAGNET-BOX/main/install.sh | sudo bash
```

Full docs: [README](https://github.com/KenjakuSoft/MAGNET-BOX#readme) · [Changelog](https://github.com/KenjakuSoft/MAGNET-BOX/blob/main/CHANGELOG.md).
