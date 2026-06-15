MagnetBox **v0.1.1** — security hardening. 🔒

A full security audit found **no Critical or High issues**. This release ships the Medium/Low hardening fixes.

## Fixes
- **SSRF / DNS-rebinding hardened** — the link downloader resolves once, verifies every IP is public, and **pins the connection** to those addresses (closes the rebinding window to `127.0.0.1` / cloud-metadata).
- **No internal error leakage** — file/stream errors return generic messages and log details server-side.
- **Content-Security-Policy** added (same-origin, no plugins, no framing).
- **Constant-time API-token comparison**.
- **Direct-download filenames re-sanitized on load** (defense-in-depth).

Nothing here changes how you use MagnetBox — it's a drop-in upgrade.

## Install — download & run (no setup)
- **Windows:** `magnetbox-windows-x64.exe` — double-click; your browser opens automatically.
- **macOS (Apple Silicon):** `magnetbox-macos-arm64`
- **Linux:** `magnetbox-linux-x64`

Log in as `admin` with the password shown on first run (also saved to `FIRST-LOGIN.txt`), then change it under Admin.

## Or run on a server (one command)
```bash
curl -fsSL https://raw.githubusercontent.com/KenjakuSoft/MAGNET-BOX/main/install.sh | sudo bash
```

Full documentation: [README](https://github.com/KenjakuSoft/MAGNET-BOX#readme) · [Changelog](https://github.com/KenjakuSoft/MAGNET-BOX/blob/main/CHANGELOG.md).
