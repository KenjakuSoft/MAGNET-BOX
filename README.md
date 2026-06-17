<div align="center">

# 🧲 MagnetBox

### Your own self-hosted download box — on your server.

Paste a **magnet**, a **`.torrent`**, or any **http(s) link** and get clean HTTP
links to **download** or **stream** every file right in your browser — with HTTP
Range support, so video **seeks and plays while it's still downloading**.
One Rust binary. Your server, your data.

<br>

[![License: AGPL v3](https://img.shields.io/badge/License-AGPL_v3-41d6a3.svg?style=flat-square)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-2021-CE412B?style=flat-square&logo=rust&logoColor=white)](https://www.rust-lang.org)
[![Engine: librqbit](https://img.shields.io/badge/engine-librqbit-38bdf8?style=flat-square)](https://github.com/ikatson/rqbit)
[![Single binary](https://img.shields.io/badge/deploy-single_binary-8b5cf6?style=flat-square)](#install)
<br>
[![Ko-fi](https://img.shields.io/badge/Ko--fi-Support_development-FF5E5B?style=flat-square&logo=ko-fi&logoColor=white)](https://ko-fi.com/lidaf)
[![PayPal](https://img.shields.io/badge/PayPal-Donate-00457C?style=flat-square&logo=paypal&logoColor=white)](https://www.paypal.com/donate/?business=JG7J2JMQ8DP38)

**[Features](#-features) · [Install](#install) · [Configuration](#-configuration) · [HTTP API](#-http-api) · [Security](#-security-posture) · [Deploy](#-deploy-to-a-vps) · [Donate](#-support)**

</div>

---

> **Free & open · donation-supported.** MagnetBox is free to self-host — no
> license, no subscription, no telemetry. It runs entirely on hardware you
> control. If it's useful to you, [support development](#-support) keeps it going.

> [!NOTE]
> **What this is.** A self-hosted **seedbox with a direct-download & streaming
> front end**. There's no global cache — your box fetches from the live
> BitTorrent swarm at seeder speed, and your server's IP participates in the
> swarm. You run your own private instance on your own hardware.

Built on [**librqbit**](https://github.com/ikatson/rqbit), a mature Rust
BitTorrent engine, so there's no external torrent client to install.

---

## ✨ Features

| | |
|---|---|
| 🔗 **Add anything** | Magnet, `.torrent` URL/upload, or a plain http(s) link — auto-detected (torrent engine vs. direct-link downloader). |
| ▶️ **Stream while downloading** | In-app video/audio player seeks before the file finishes; subtitles auto-loaded from the torrent's `.srt` (→ WebVTT). |
| ✅ **Pick what to download** | Per-file checkboxes (live), select all/none, and an **add-paused** option to choose files *before* anything downloads. |
| 🔐 **Login + multi-user** | argon2 passwords, server-side sessions, roles, and a full **admin console** at `/admin`. |
| 🎟 **Invite-only registration** | Closed sign-up — invite codes (single/multi-use, optional expiry) + an open/close toggle and **maintenance mode**. |
| 🛡️ **2FA (TOTP)** | Optional per-account, QR enrollment in any authenticator app, with one-time **recovery codes**. |
| 📈 **Host & usage metrics** | Live **CPU / RAM / disk** in the admin Overview; **per-user bandwidth**, last-seen and IP in the Users table. |
| 🚦 **Per-user limits** | Admin-set cap on simultaneous direct downloads per user (`0` = unlimited). |
| 🗑️ **Retention / auto-expiry** | Optionally auto-delete torrents & downloads (and files) older than N days; hourly sweep + "Run cleanup now". |
| ⚙️ **Settings** | Change password, per-browser defaults, and an admin-only **global speed limit** (persisted). |
| 🔑 **API + tokens** | Per-user bearer token + **OpenAPI 3.1** spec at `/api/openapi.json` and a reference page at `/docs`. |
| ⏸ **Full control** | Pause / resume / delete (optionally erasing files), drag-and-drop `.torrent`, batch add, live stats, name filter. |
| 🦀 **Single binary** | Embedded web UI, binds `127.0.0.1` by default. No database server, no container stack required. |

---

## Install

No technical knowledge required — pick the path that fits you.

### 🖥️ Easiest — run it on your own computer

1. Open the **[Releases](https://github.com/KenjakuSoft/MAGNET-BOX/releases/latest)** page.
2. Download the file for your system:
   - **Windows** → `magnetbox-windows-x64.exe`
   - **macOS (Apple Silicon)** → `magnetbox-macos-arm64`
   - **Linux** → `magnetbox-linux-x64`
3. **Open it.** A small window appears and **your browser opens automatically** to a quick setup page.
4. **Create your account** — paste the one-time setup code shown in that window, pick a username and
   password, and you're in. (Turn on two-factor auth and add users afterwards, any time.)

That's the whole thing — no commands, no Rust, no build.

> **Windows** may show a "Windows protected your PC" notice for a new app → click **More info → Run anyway**.
> **macOS/Linux**: it's a plain program, not an installer — open a terminal in your downloads folder and run
> `chmod +x magnetbox-*` then `./magnetbox-*`. (macOS: first time, right-click the file → **Open**.)
> Keep the window open while you use it — closing it stops MagnetBox.

### 🌐 Always-on — run it on a server (one command)

On a fresh **Ubuntu/Debian** server, paste this one line:

```bash
curl -fsSL https://raw.githubusercontent.com/KenjakuSoft/MAGNET-BOX/main/install.sh | sudo bash
```

It downloads MagnetBox, installs it as a service that **starts on boot**, and prints your
admin login. To use it from anywhere with HTTPS, point a domain at the server and add Caddy —
see [Deploy to a VPS](#-deploy-to-a-vps).

### 🛠️ Build from source (advanced)

```bash
git clone https://github.com/KenjakuSoft/MAGNET-BOX.git
cd MAGNET-BOX
cargo run --release      # needs the Rust toolchain — https://rustup.rs
```

📖 **Full documentation:** [`landing/docs.html`](landing/docs.html).

---

## 🔧 Configuration

Everything is configured with environment variables — no config file to edit.

| Var | Default | Purpose |
|---|---|---|
| `MAGNETBOX_PORT` | `8080` | Listen port |
| `MAGNETBOX_BIND` | `127.0.0.1` | Bind address (keep localhost; put a proxy in front) |
| `MAGNETBOX_DIR` | `./downloads` | Where downloaded files are stored |
| `MAGNETBOX_DATA` | `./magnetbox-data` | Where accounts/settings live |
| `MAGNETBOX_HTTPS` | `0` | `1` marks session cookies `Secure` (set when behind HTTPS) |
| `MAGNETBOX_ADMIN_USER` | `admin` | First-run admin username |
| `MAGNETBOX_ADMIN_PASSWORD` | *(generated)* | First-run admin password (≥8 chars). Set it to reset a lost password. |
| `RUST_LOG` | `info` | Log verbosity |

---

## 🔌 HTTP API

All routes except `/login` and `/api/login` require a valid session **cookie** —
or an `Authorization: Bearer <token>` header (from the Account page) for
automation. Admin-only paths (`/admin`, `/api/users*`, `/api/settings`) require
the `admin` role. The full machine-readable spec is at `/api/openapi.json`.

<details>
<summary><b>Full endpoint reference</b> (click to expand)</summary>

<br>

| Method | Path | Body / notes |
|--------|------|--------------|
| GET    | `/`                          | Web UI |
| GET    | `/api/torrents`              | torrents + files (incl. `selected`/`paused`) + `adding`/`errors` |
| POST   | `/api/add`                   | `{ "source": "magnet / .torrent URL / http(s) link", "paused", "trackers" }` (auto-detected) |
| POST   | `/api/upload`                | raw `.torrent` body; `?paused=true&trackers=true` |
| GET    | `/api/links`                 | list direct-link downloads + progress |
| POST   | `/api/links/{id}/delete`     | remove a direct download; `?files=true` erases it |
| GET    | `/dl/{id}`                   | serve a finished direct download (Range) |
| GET    | `/subtitle/{id}/{file}`      | a torrent's `.srt` converted to WebVTT for the player |
| GET    | `/api/account`               | account info + API token + session count |
| POST   | `/api/account/token`         | generate/regenerate the API token |
| POST   | `/api/account/logout-others` | end all of your other sessions |
| POST   | `/api/torrents/{id}/files`   | `{ "files": [indices] }` — set files to download |
| POST   | `/api/torrents/{id}/pause` · `/resume` · `/delete` | pause / resume / remove (`?files=true` erases data) |
| GET    | `/download/{id}/{file}`      | file as attachment (Range supported) |
| GET    | `/stream/{id}/{file}`        | file inline for players (Range) |
| GET    | `/login` · POST `/api/login` | login page / `{username,password}` → session **or** `{twofa,challenge}` |
| POST   | `/api/login/2fa`             | `{challenge, code}` → session cookie (TOTP or recovery code) |
| POST   | `/api/account/2fa/start` · `/confirm` · `/disable` | enroll (QR+secret) / confirm (→ recovery codes) / disable |
| POST   | `/api/logout`                | end session |
| GET    | `/api/me` · POST `/api/me/password` | current user / change own password |
| GET/POST | `/api/users` *(admin)*     | list / create users |
| POST   | `/api/users/{name}/password` · `/delete` *(admin)* | reset password / delete user |
| GET    | `/register` · POST `/api/register` · GET `/api/register/status` | invite-only sign-up (public) |
| GET/POST | `/api/admin/invites` *(admin)* · POST `…/{code}/delete` | list / create / delete invite codes |
| GET/POST | `/api/admin/config` *(admin)* | get/set registration-open + maintenance |
| POST   | `/api/users/{name}/disabled` · `/token` *(admin)* | ban/unban · rotate a user's API token |
| GET    | `/admin` *(admin)*           | the admin console |
| GET    | `/api/admin/overview` *(admin)* | live system + engine + storage stats |
| GET    | `/api/admin/activity` *(admin)* | audit log of state-changing actions |
| GET    | `/api/admin/sessions` *(admin)* | active sessions; `POST …/{sid}/revoke`, `…/revoke-others` |
| POST   | `/api/admin/torrents/pause-all` · `resume-all` *(admin)* | bulk torrent control |
| POST   | `/api/admin/downloads/clear-completed` *(admin)* | delete all finished direct downloads |
| GET    | `/settings`                  | settings page (account + preferences; admin extras) |
| GET/POST | `/api/settings` *(admin)*  | get / set global speed limits (bytes/sec; `0`/null = unlimited) |

</details>

#### How streaming works

`/stream/{id}/{file}` calls librqbit's `ManagedTorrent::stream(file)`, which
returns a seekable, piece-aware reader. The handler maps the HTTP `Range` header
onto a `seek` + length-limited read, so a player requesting `bytes=…` gets a
`206 Partial Content` and the engine fetches exactly those pieces on demand.

---

## 🔒 Security posture

Audited and hardened for an internet-facing private instance:

- **SSRF protection** — the direct-link downloader resolves each URL (and every
  redirect hop) and refuses private/loopback/link-local/CGNAT/cloud-metadata
  addresses, so it can't reach `localhost`, internal hosts, or `169.254.169.254`.
- **Two-factor auth (TOTP)** — optional per-account via QR, enforced as a second
  login step, with one-time **recovery codes**; secrets stored server-side only.
- **Login hardening** — argon2 verify, constant-ish timing (dummy verify for
  unknown usernames to prevent enumeration), per-username throttling (5 fails →
  ~15 min lock); 2FA challenges expire in 5 min and cap attempts.
- **CSRF** — `SameSite=Strict` session cookie + cross-origin `Origin` check on
  mutating requests; API clients use bearer tokens (not CSRF-able).
- **Security headers** on every response — `X-Content-Type-Options: nosniff`,
  `X-Frame-Options: DENY`, `Referrer-Policy: no-referrer`, `Permissions-Policy`.
- **XSS-safe rendering** — all user-controlled values escaped; usernames
  charset-restricted; no user data in inline handlers.
- **Per-user limits** — admin-set cap on concurrent direct downloads (429 over cap).
- **Secrets at rest** — `users.json` / `invites.json` / `usage.json` written
  `0600` (owner-only) on Unix.
- **No SQL** (no SQLi), **no `unsafe`**, file access by index or sanitized name.

> Operational hardening: run as a dedicated non-root user, keep the app bound to
> `127.0.0.1` behind Caddy (so `X-Forwarded-For` is trustworthy), and run
> `cargo audit` on the server periodically for dependency CVEs.

---

## 🚀 Deploy to a VPS

**Fastest:** on a fresh Ubuntu/Debian server, the one-line installer downloads the
prebuilt binary and sets up the systemd service for you:

```bash
curl -fsSL https://raw.githubusercontent.com/KenjakuSoft/MAGNET-BOX/main/install.sh | sudo bash
```

Then just add a domain + HTTPS (steps 1, 4–6 below). The manual route is here if you
prefer to build from source:

<details>
<summary><b>Manual step-by-step (build from source + HTTPS + login)</b></summary>

<br>

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
The unit already sets `MAGNETBOX_HTTPS=1`, binds localhost, and uses
`ProtectSystem=strict`. (Or set `MAGNETBOX_ADMIN_PASSWORD` in the unit.)

**4. Caddy (HTTPS + reverse proxy)** — install Caddy, drop in
[`deploy/Caddyfile`](deploy/Caddyfile) with your domain, then
`sudo systemctl reload caddy`. Caddy fetches and renews the TLS cert automatically.

**5. Firewall** — only expose web ports; never the app port:
```bash
sudo ufw allow OpenSSH && sudo ufw allow 80 && sudo ufw allow 443 && sudo ufw enable
```
(Optional: open your BitTorrent listen port for better peer connectivity.)

**6. Log in** at `https://magnetbox.example.com/login`, change the admin
password, enable 2FA, and add your users from the **Admin** panel.

</details>

**Hardening checklist**
- [ ] Strong admin password; rotate the generated one immediately.
- [ ] `MAGNETBOX_BIND=127.0.0.1` (default) — the app is never directly exposed.
- [ ] HTTPS only via Caddy; `MAGNETBOX_HTTPS=1` so cookies are `Secure`.
- [ ] Firewall closed except 80/443 (+SSH).
- [ ] Keep it invite-only — only create accounts for people you trust.
- [ ] Consider a second factor at the proxy (Cloudflare Access / Authelia) if it faces the open internet.
- [ ] Run `cargo audit` before launch and periodically after.

---

## 🧱 Stack

**Rust 2021** · **librqbit 8.1** (BitTorrent engine) · **axum 0.7** (HTTP) ·
**tokio** · tokio-util `ReaderStream` (range streaming) · embedded vanilla-JS UI.

---

## 💚 Support

MagnetBox is free and open. Donations fund maintenance, security fixes, and new
features — thank you. 🙏

[![Ko-fi](https://img.shields.io/badge/Ko--fi-Support_development-FF5E5B?style=for-the-badge&logo=ko-fi&logoColor=white)](https://ko-fi.com/lidaf)
[![PayPal](https://img.shields.io/badge/PayPal-Donate-00457C?style=for-the-badge&logo=paypal&logoColor=white)](https://www.paypal.com/donate/?business=JG7J2JMQ8DP38)

### Crypto

> Hover a code block and click the **copy** icon on GitHub.

**![BTC](https://img.shields.io/badge/BTC-F7931A?style=flat-square&logo=bitcoin&logoColor=white) Bitcoin**
```
13sZHQvfYx3o9Ctb4oGX9zeeDDWi8qNJF6
```

**![ETH](https://img.shields.io/badge/ETH-627EEA?style=flat-square&logo=ethereum&logoColor=white) Ethereum · ![USDT](https://img.shields.io/badge/USDT-26A17B?style=flat-square&logo=tether&logoColor=white) USDT — ERC-20**
```
0x25ab7f10d0d2586838017c24126c420ebc2368dc
```

**![LTC](https://img.shields.io/badge/LTC-345D9D?style=flat-square&logo=litecoin&logoColor=white) Litecoin**
```
LSCNAVmn7ozcwBwKqig5LEPAWgiuubRmmB
```

**![DOGE](https://img.shields.io/badge/DOGE-C2A633?style=flat-square&logo=dogecoin&logoColor=white) Dogecoin**
```
DRzwctKM3JCfmHCRmLUbyYbUbUVGeESmKp
```

---

## 📜 License

Licensed under the **GNU Affero General Public License v3.0** — see
[`LICENSE`](LICENSE). In short: it's free and open, and if you run a **modified**
version as a network service you must make your source available under the same
license. Contributions are welcome.
