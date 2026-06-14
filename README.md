# MagnetBox

A personal, self-hosted **magnet/torrent → direct HTTP download & streaming**
gateway in a single Rust binary. Paste a magnet or upload a `.torrent`, and get
clean HTTP links to **download** or **stream** each file in your browser — with
HTTP Range support, so video seeks and plays *while it's still downloading*.

It's built on [**librqbit**](https://github.com/ikatson/rqbit), a mature Rust
BitTorrent engine, so there's no external torrent client to install.

**Free & open · donation-supported.** MagnetBox is free to self-host — no license,
no subscription. If it's useful to you, [support development »](landing/index.html#support).
Full documentation lives in [`landing/docs.html`](landing/docs.html).

> ⚠️ **What this is — and isn't.** This is a self-hosted **seedbox with a
> direct-download front end**, not a clone of Real-Debrid. RD feels instant
> because it serves from a huge pre-filled cache; a single-user box has **no
> cache** and must fetch from the live swarm at seeder speed. Your machine's IP
> also joins the swarm. Use it only for content you have the right to download
> (your own files, Linux ISOs, Creative Commons / public-domain media, etc.).

## Features

- 🔐 **Login + multi-user** — argon2 passwords, sessions, and a full **admin
  console** (`/admin`): live overview/stats, all torrents & downloads with bulk
  controls, user management, active-session revocation, an audit/activity log,
  and global settings. Built to sit behind HTTPS on your own domain.
- 🎟 **Invite-only registration** (Demonoid-style) — a `/register` page gated by
  **invite codes** (single/multi-use, optional expiry) and a global
  **registration open/close** toggle you flip during an occasional window. Plus
  **ban/disable** users, **reset a user's API token**, last-seen/IP tracking, and
  a **maintenance mode** that locks out everyone but admins.
- 📈 **Host & usage metrics** — the admin Overview shows live **CPU / RAM / disk**
  of the server, and the Users table tracks **per-user bandwidth served**
  (cumulative, persisted across restarts) plus last-seen and IP.
- 🚦 **Per-user limits** — admin-configurable cap on simultaneous direct downloads
  per user (anti-abuse; 0 = unlimited), with download ownership shown in the
  admin Downloads view.
- 🗑️ **Retention / auto-expiry** — optionally auto-delete torrents & downloads
  (and their files) older than N days to reclaim disk; runs hourly with a
  "Run cleanup now" button. `0` = keep forever.
- ⚙️ **Settings** — user menu with a **Settings** page: change password, per-browser
  defaults (add-paused / public-trackers), and an admin-only **global speed limit**
  (persisted, re-applied on restart).
- 🔗 Add by **magnet**, **`.torrent` URL/upload**, or a plain **http(s) link** —
  the kind is auto-detected (torrents vs. a direct-link download manager).
- 🗂️ **Dashboard** with **Torrents** and **Downloads** tabs (direct-link history:
  name, size, progress, status, re-download).
- ▶️ **In-app video/audio player** — plays/seeks while downloading, with subtitle
  tracks auto-loaded from the torrent's `.srt` files (converted to WebVTT).
- 🔑 **Account page + API token** — per-user bearer token so scripts/apps can
  drive every endpoint with `Authorization: Bearer <token>`.
- 📘 **OpenAPI 3.1 spec** at `/api/openapi.json` + a self-contained reference page
  at `/docs` (no external scripts) — import into Postman/Insomnia or point your
  Kodi/Stremio-style tooling at it.
- 📊 **Live stats bar** (speed, active count, items, storage used), **name filter**,
  **drag-and-drop** `.torrent` files, **batch add** (paste several at once), and
  direct downloads that **persist across restarts**.
- ✅ **Pick what to download** — per-file checkboxes (live), select all/none, and an
  **"add paused"** option to choose files *before* anything downloads.
- ⏸ **Pause / Resume** and 🗑 **Delete** (optionally erasing files from disk).
- 📡 Optional **+ public trackers** toggle to find peers faster on trackerless
  magnets (leave off for private-tracker torrents).
- 📥 **Direct download** links per file (`Content-Disposition: attachment`).
- ▶ **Streaming** with HTTP Range — open a video link in the browser or VLC and
  seek before it's done. librqbit prioritizes the pieces you're reading.
- 📊 Live progress, speed, per-file readiness, polled into a clean web UI.
- 🦀 Single binary, embedded web UI. Binds to `127.0.0.1` by default.

## Run

```bash
git clone https://github.com/KenjakuSoft/MAGNET-BOX.git
cd MAGNET-BOX
cargo run --release
# then open the printed URL, e.g. http://127.0.0.1:8080
```

On first run it prints a generated **admin** username/password (or set your own
via env). Log in at `/login`; admins get an **Admin** link to manage users.

Config via env vars:

| Var                       | Default        | Purpose                                            |
|---------------------------|----------------|----------------------------------------------------|
| `MAGNETBOX_PORT`          | `8080`         | Listen port                                        |
| `MAGNETBOX_BIND`          | `127.0.0.1`    | Bind address (keep localhost; proxy in front)      |
| `MAGNETBOX_DIR`           | `./downloads`  | Where files are stored                             |
| `MAGNETBOX_DATA`          | `./magnetbox-data` | Where `users.json` lives                       |
| `MAGNETBOX_HTTPS`         | `0`            | `1` = mark session cookies `Secure` (set behind HTTPS) |
| `MAGNETBOX_ADMIN_USER`    | `admin`        | First-run admin username                           |
| `MAGNETBOX_ADMIN_PASSWORD`| *(generated)*  | First-run admin password (≥8 chars; else random)   |
| `RUST_LOG`                | `info`         | Log verbosity                                      |

## HTTP API

| Method | Path                       | Body / notes                          |
|--------|----------------------------|---------------------------------------|
| GET    | `/`                          | Web UI                                            |
| GET    | `/api/torrents`              | JSON: torrents + files (incl. `selected`/`paused`) + `adding`/`errors` |
| POST   | `/api/add`                   | `{ "source": "magnet / .torrent URL / http(s) link", "paused", "trackers" }` (auto-detected) |
| POST   | `/api/upload`                | raw `.torrent` body; `?paused=true&trackers=true` |
| GET    | `/api/links`                 | list direct-link downloads + progress             |
| POST   | `/api/links/{id}/delete`     | remove a direct download; `?files=true` erases it |
| GET    | `/dl/{id}`                   | serve a finished direct download (Range)          |
| GET    | `/subtitle/{id}/{file}`      | a torrent's `.srt` converted to WebVTT for the player |
| GET    | `/api/account`               | account info + API token + session count          |
| POST   | `/api/account/token`         | generate/regenerate the API token                 |
| POST   | `/api/account/logout-others` | end all of your other sessions                    |
| POST   | `/api/torrents/{id}/files`   | `{ "files": [indices] }` — set files to download  |
| POST   | `/api/torrents/{id}/pause`   | pause                                             |
| POST   | `/api/torrents/{id}/resume`  | resume                                            |
| POST   | `/api/torrents/{id}/delete`  | remove torrent; `?files=true` also erases data    |
| GET    | `/download/{id}/{file}`      | file as attachment (Range supported)              |
| GET    | `/stream/{id}/{file}`        | file inline for players (Range)                   |
| GET    | `/login` · POST `/api/login` | login page / `{username,password}` → session **or** `{twofa,challenge}` |
| POST   | `/api/login/2fa`             | `{challenge, code}` → session cookie (TOTP or recovery code) |
| POST   | `/api/account/2fa/start` · `/confirm` · `/disable` | enroll (QR+secret) / confirm (→ recovery codes) / disable |
| POST   | `/api/logout`                | end session                                       |
| GET    | `/api/me` · POST `/api/me/password` | current user / change own password         |
| GET/POST | `/api/users` *(admin)*     | list / create users                               |
| POST   | `/api/users/{name}/password` · `/delete` *(admin)* | reset password / delete user      |
| GET    | `/register` · POST `/api/register` · GET `/api/register/status` | invite-only sign-up (public) |
| GET/POST | `/api/admin/invites` *(admin)* · POST `…/{code}/delete` | list / create / delete invite codes |
| GET/POST | `/api/admin/config` *(admin)* | get/set registration-open + maintenance |
| POST   | `/api/users/{name}/disabled` · `/token` *(admin)* | ban/unban · rotate a user's API token |
| GET    | `/admin` *(admin)*           | the admin console (overview/torrents/downloads/users/invites/sessions/activity/access/settings) |
| GET    | `/api/admin/overview` *(admin)* | live system + engine + storage stats           |
| GET    | `/api/admin/activity` *(admin)* | audit log of state-changing actions            |
| GET    | `/api/admin/sessions` *(admin)* | active sessions; `POST …/{sid}/revoke`, `…/revoke-others` |
| POST   | `/api/admin/torrents/pause-all` · `resume-all` *(admin)* | bulk torrent control      |
| POST   | `/api/admin/downloads/clear-completed` *(admin)* | delete all finished direct downloads |
| GET    | `/settings`                  | settings page (account + preferences; admin extras) |
| GET/POST | `/api/settings` *(admin)*  | get / set global speed limits (bytes/sec; `0`/null = unlimited) |

All routes except `/login` and `/api/login` require a valid session **cookie**
— or an `Authorization: Bearer <token>` header (from the Account page) for
API/automation. Admin-only paths (`/admin`, `/api/users*`, `/api/settings`)
require the `admin` role.

## How streaming works

`/stream/{id}/{file}` calls librqbit's `ManagedTorrent::stream(file)`, which
returns a seekable, piece-aware reader. The handler maps the HTTP `Range` header
onto a `seek` + length-limited read, so a player requesting `bytes=…` gets a
`206 Partial Content` and the engine fetches exactly those pieces on demand.

## Security posture

Audited and hardened for an internet-facing private instance:

- **SSRF protection** — the direct-link downloader resolves each URL (and every
  redirect hop) and refuses private/loopback/link-local/CGNAT/cloud-metadata
  addresses, so it can't be used to reach `localhost`, internal hosts, or
  `169.254.169.254`.
- **Security headers** on every response: `X-Content-Type-Options: nosniff`,
  `X-Frame-Options: DENY`, `Referrer-Policy: no-referrer`, `Permissions-Policy`.
- **XSS-safe rendering** — all user-controlled values are escaped; usernames are
  charset-restricted; no user data is interpolated into inline handlers.
- **Two-factor auth (TOTP)** — optional per-account, set up via QR in the Account
  page (any authenticator app), enforced as a second step at login, with one-time
  **recovery codes**. Secrets are stored server-side only.
- **Login hardening** — argon2 verify, constant-ish timing (dummy verify for
  unknown usernames to prevent enumeration), and per-username attempt throttling;
  2FA login challenges expire in 5 min and cap attempts.
- **CSRF** — `SameSite=Strict` session cookie + cross-origin `Origin` check on
  mutating requests; API clients use bearer tokens (not CSRF-able).
- **Per-user limits** — admin-set cap on concurrent direct downloads (429 over cap).
- **Secrets at rest** — `users.json` / `invites.json` / `usage.json` are written
  `0600` (owner-only) automatically on Unix.
- No SQL (so no SQLi), no `unsafe`, file access is by index or sanitized name.

> Operational hardening: run as a dedicated non-root user, keep the app bound to
> `127.0.0.1` behind Caddy (so `X-Forwarded-For` is trustworthy), and run
> `cargo audit` on the server periodically for dependency CVEs.

## 🚀 Go-live checklist

1. **DNS** → point `magnetbox.example.com` at the VPS.
2. **Build on the VPS:** `cargo build --release` → install the binary to
   `/opt/magnetbox/magnetbox`; create dirs `downloads/` + `data/` owned by a
   dedicated `magnetbox` user.
3. **systemd:** install [`deploy/magnetbox.service`](deploy/magnetbox.service)
   (already sets `MAGNETBOX_HTTPS=1`, binds localhost, `ProtectSystem=strict`,
   `HOME=/opt/magnetbox`); `systemctl enable --now magnetbox`.
4. **First login:** grab the generated admin password from
   `journalctl -u magnetbox`, sign in, **change it**, and **enable 2FA** on the
   Account page.
5. **HTTPS:** install Caddy with [`deploy/Caddyfile`](deploy/Caddyfile) (your
   domain) → automatic Let's Encrypt cert + reverse proxy to `127.0.0.1:8080`.
6. **Firewall:** `ufw allow OpenSSH && ufw allow 80 && ufw allow 443 && ufw enable`
   — never expose the app port directly. (Optionally open your BitTorrent port for
   better peer connectivity.)
7. **Access policy:** keep **registration closed**; open it briefly only when
   handing out invite codes, then close it again.
8. **Audit:** run `cargo audit` for dependency CVEs before and periodically after.

## Authentication

- **argon2**-hashed passwords stored in `users.json`.
- Server-side sessions; **HttpOnly + SameSite=Strict** cookie (`Secure` when
  `MAGNETBOX_HTTPS=1`).
- **CSRF**: cross-origin mutating requests are rejected (Origin check) on top of
  SameSite=Strict.
- **Login throttling**: 5 failed attempts locks that username for ~15 min.
- **Roles**: `admin` (manage users + everything) and `user` (use the app; shared
  torrent list). The last admin can't be deleted.

## Deploy to a VPS (public domain + login)

> ⚠️ Your VPS provider gets the DMCA notices. Keep this to content you have the
> right to download, or they'll suspend the box.

**1. DNS** — point an `A` record (e.g. `magnetbox.example.com`) at the VPS IP.

**2. Build on the VPS** (Ubuntu/Debian shown):
```bash
sudo apt update && sudo apt install -y build-essential pkg-config curl
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
source "$HOME/.cargo/env"
git clone https://github.com/KenjakuSoft/MAGNET-BOX.git magnetbox && cd magnetbox
cargo build --release
sudo useradd --system --create-home --home-dir /opt/magnetbox magnetbox
sudo install -m755 target/release/magnetbox /opt/magnetbox/magnetbox
sudo mkdir -p /opt/magnetbox/{downloads,data} && sudo chown -R magnetbox:magnetbox /opt/magnetbox
```

**3. systemd** — copy [`deploy/magnetbox.service`](deploy/magnetbox.service) to
`/etc/systemd/system/`, then:
```bash
sudo systemctl daemon-reload && sudo systemctl enable --now magnetbox
journalctl -u magnetbox -f          # grab the first-run admin password here
```
The unit already sets `MAGNETBOX_HTTPS=1` and binds localhost. (Or set
`MAGNETBOX_ADMIN_PASSWORD` in the unit to pick your own.)

**4. Caddy (HTTPS + reverse proxy)** — install Caddy, drop in
[`deploy/Caddyfile`](deploy/Caddyfile) with your domain, then `sudo systemctl reload caddy`.
Caddy fetches and renews the TLS cert automatically.

**5. Firewall** — only expose web ports; never the app port:
```bash
sudo ufw allow OpenSSH && sudo ufw allow 80 && sudo ufw allow 443
sudo ufw enable
```
(Optional: open your BitTorrent listen port for better peer connectivity.)

**6. Log in** at `https://magnetbox.example.com/login`, change the admin
password, and add your users from the **Admin** panel.

### Hardening checklist
- [ ] Strong admin password; rotate the generated one immediately.
- [ ] `MAGNETBOX_BIND=127.0.0.1` (default) — the app is never directly exposed.
- [ ] HTTPS only via Caddy; `MAGNETBOX_HTTPS=1` so cookies are `Secure`.
- [ ] Firewall closed except 80/443 (+SSH).
- [ ] Keep it invite-only — only create accounts for people you trust.
- [ ] Consider a second factor at the proxy (Cloudflare Access / Authelia) if it
      faces the open internet.

## Stack

Rust 2021 · librqbit 8.1 (BitTorrent engine) · axum 0.7 (HTTP) · tokio ·
tokio-util `ReaderStream` (range streaming) · embedded vanilla-JS UI.

## License

Licensed under the **GNU Affero General Public License v3.0** — see
[`LICENSE`](LICENSE). In short: it's free and open, and if you run a **modified**
version as a network service you must make your source available under the same
license. Contributions are welcome.
