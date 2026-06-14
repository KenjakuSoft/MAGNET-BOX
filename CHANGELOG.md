# Changelog

All notable changes to MagnetBox are documented here. This project follows
[Keep a Changelog](https://keepachangelog.com/) and [Semantic Versioning](https://semver.org/).

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
