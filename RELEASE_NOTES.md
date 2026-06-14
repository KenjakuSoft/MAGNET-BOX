MagnetBox **v0.1.0** — first public release. 🎉

A self-hosted magnet/torrent + direct-link downloader and in-browser streamer, in a single binary.

## Install — download & run (no setup)
- **Windows:** `magnetbox-windows-x64.exe` — double-click; your browser opens automatically.
- **macOS (Apple Silicon):** `magnetbox-macos-arm64`
- **Linux:** `magnetbox-linux-x64`

Log in as `admin` with the password shown on first run (also saved to `FIRST-LOGIN.txt`), then change it under Admin.

## Or run on a server (one command)
```bash
curl -fsSL https://raw.githubusercontent.com/KenjakuSoft/MAGNET-BOX/main/install.sh | sudo bash
```

## What's included
- 🔗 Add magnets, `.torrent` files, or direct `http(s)` links — auto-detected
- ▶️ Stream video/audio in the browser **while it downloads** (seeking + subtitles)
- ✅ Pick which files to download; pause / resume / delete
- 👥 Multi-user with invite-only registration, **TOTP 2FA**, and a full admin console
- 🗑️ Retention / auto-expiry and per-user download limits
- 🔌 OpenAPI 3.1 spec + per-user API tokens
- 🔒 Security-hardened: SSRF protection, argon2, CSRF, security headers

Full documentation: [README](https://github.com/KenjakuSoft/MAGNET-BOX#readme).
