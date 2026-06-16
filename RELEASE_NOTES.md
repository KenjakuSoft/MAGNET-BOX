MagnetBox **v0.1.2** — play any media file in the browser. 🎬

## New
- **Universal playback (optional)** — install `ffmpeg` on the server and the player transcodes on the fly (H.264 / AAC) for anything your browser can't decode natively — **MKV, H.265, AVI**, and more. It even works **while the file is still downloading**. No ffmpeg? It falls back to the "open in VLC / Infuse" hint via the ⧉ Copy link button.
- **Fullscreen** button + `f` shortcut in the player.
- A **per-user cap** on simultaneous conversions, so one account can't peg the server's CPU.

> **To enable transcoding:** `sudo apt install ffmpeg` (Linux server) or `winget install ffmpeg` (Windows), then restart MagnetBox. On startup it logs *"ffmpeg detected — in-browser transcoding enabled."*

## Install — download & run (no setup)
- **Windows:** `magnetbox-windows-x64.exe` — double-click; your browser opens automatically.
- **macOS (Apple Silicon):** `magnetbox-macos-arm64`
- **Linux:** `magnetbox-linux-x64`

Log in as `admin` with the password shown on first run (also saved to `FIRST-LOGIN.txt`).

## Or run on a server (one command)
```bash
curl -fsSL https://raw.githubusercontent.com/KenjakuSoft/MAGNET-BOX/main/install.sh | sudo bash
```

Full docs: [README](https://github.com/KenjakuSoft/MAGNET-BOX#readme) · [Changelog](https://github.com/KenjakuSoft/MAGNET-BOX/blob/main/CHANGELOG.md).
