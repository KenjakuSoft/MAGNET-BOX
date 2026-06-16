# Changelog

All notable changes to MagnetBox are documented here. This project follows
[Keep a Changelog](https://keepachangelog.com/) and [Semantic Versioning](https://semver.org/).

## [0.1.2] — 2026-06-16

### Added
- **Play (almost) any media type in the browser** — optional on-the-fly transcoding via `ffmpeg` (auto-detected on `PATH`). When the browser can't decode a file's video codec (e.g. MKV / H.265), the player automatically switches to a server-side H.264 + AAC fragmented-MP4 stream. It works **while the torrent is still downloading** (the file's piece stream is piped straight into ffmpeg). With no ffmpeg installed it falls back to the "open in an external player" hint.
- **Fullscreen** button (and `f` shortcut) in the in-app player.
- A notice in the player when the browser can't render the video, pointing to the transcode / external player.

### Security / limits
- Per-user cap on concurrent transcodes (each is a live ffmpeg encode) so one account can't exhaust the server's CPU.

### Changed
- Internal rustfmt / clippy cleanups (import ordering, extracted type aliases, line wrapping).

## [0.1.1] — 2026-06-16

Security hardening release (from a full audit — no Critical/High issues were found; these are the Medium/Low fixes).

### Security
- **SSRF / DNS-rebinding (TOCTOU) hardened** — the direct-link downloader now resolves the host once, verifies every IP is public, and **pins the connection** to those exact addresses, closing the window where a rebinding domain could pass the check and then resolve to `127.0.0.1` / `169.254.169.254`.
- **Stopped leaking internal error detail** — stream/seek/file errors now return generic client messages and log the detail server-side; other handlers no longer expose the full error context chain.
- **Content-Security-Policy** added (same-origin default, `object-src 'none'`, `frame-ancestors 'none'`, restricted `base-uri`/`form-action`).
- **Constant-time API-token comparison** (avoids byte-by-byte timing leakage).
- **Direct-download filenames re-sanitized on load** from `index.json` (defense-in-depth against a tampered data dir).

## [0.1.0] — 2026-06-15

First public release.

### Added
- Add content by magnet link, `.torrent` URL/upload, or direct `http(s)` link (auto-detected).
- In-browser video/audio player that streams and seeks **while downloading**, with subtitles pulled from the torrent.
- Per-file selection, pause / resume / delete, batch add, and drag-and-drop `.torrent` files.
- Multi-user accounts with invite-only registration, roles, and a maintenance mode.
- TOTP two-factor authentication with one-time recovery codes.
- Admin console: live CPU / RAM / disk metrics, per-user bandwidth, active sessions, audit log, and bulk controls.
- Retention / auto-expiry and per-user concurrent-download limits.
- OpenAPI 3.1 spec and per-user API tokens.
- Prebuilt binaries for Windows, macOS (Apple Silicon), and Linux.
- Desktop launch opens the browser automatically; first-run admin credentials saved to `FIRST-LOGIN.txt`.
- One-line server installer (`install.sh`).

### Security
- SSRF protection on the direct-link downloader — blocks private, loopback, link-local, CGNAT, and cloud-metadata addresses, including across redirects.
- argon2 password hashing, server-side sessions, CSRF `Origin` checks, login throttling, and security headers on every response.

[0.1.0]: https://github.com/KenjakuSoft/MAGNET-BOX/releases/tag/v0.1.0
