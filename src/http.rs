//! HTTP layer: JSON control API + the direct download/stream endpoints.

use std::collections::HashMap;
use std::io::SeekFrom;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use axum::{
    body::{Body, Bytes},
    extract::{FromRef, Path, Query, Request, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    middleware::{self, Next},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
    Extension, Json, Router,
};
use librqbit::AddTorrent;
use serde::Deserialize;
use serde_json::json;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncSeek, AsyncSeekExt};
use tokio::process::{Child, ChildStdout, Command};
use tokio_util::io::ReaderStream;

use crate::auth::{require_auth, token_from_headers, Auth, Identity, LoginOutcome, Role};
use crate::downloads::DirectManager;
use crate::engine::App;
use crate::metrics::Metrics;

/// Combined state so handlers can extract either the torrent engine or auth
/// (via `FromRef`), while existing `State<App>` handlers keep working.
#[derive(Clone)]
pub struct AppState {
    engine: App,
    auth: Auth,
    direct: DirectManager,
    metrics: Metrics,
    started: Instant,
    /// Whether `ffmpeg` is on PATH — enables on-the-fly transcoding so the
    /// in-browser player can play formats the browser can't decode natively.
    ffmpeg: bool,
    /// In-flight transcodes per user, to cap CPU-heavy concurrent conversions.
    transcodes: Arc<Mutex<HashMap<String, usize>>>,
}

impl FromRef<AppState> for App {
    fn from_ref(s: &AppState) -> App {
        s.engine.clone()
    }
}
impl FromRef<AppState> for Auth {
    fn from_ref(s: &AppState) -> Auth {
        s.auth.clone()
    }
}
impl FromRef<AppState> for DirectManager {
    fn from_ref(s: &AppState) -> DirectManager {
        s.direct.clone()
    }
}

pub fn router(engine: App, auth: Auth, direct: DirectManager, metrics: Metrics) -> Router {
    let ffmpeg = ffmpeg_available();
    if ffmpeg {
        tracing::info!("ffmpeg detected — in-browser transcoding enabled for unsupported formats");
    } else {
        tracing::info!("ffmpeg not found — unsupported formats fall back to external players");
    }
    let state = AppState {
        engine,
        auth: auth.clone(),
        direct,
        metrics,
        started: Instant::now(),
        ffmpeg,
        transcodes: Arc::new(Mutex::new(HashMap::new())),
    };

    // Everything here needs a valid session (admin-only paths enforced inside
    // the middleware).
    let protected = Router::new()
        .route("/", get(index))
        .route("/admin", get(admin_page))
        .route("/settings", get(settings_page))
        .route("/account", get(account_page))
        .route("/docs", get(docs_page))
        .route("/api/openapi.json", get(openapi_json))
        .route("/api/settings", get(settings_get).post(settings_set))
        .route("/api/torrents", get(list))
        .route("/api/add", post(add))
        .route("/api/upload", post(upload))
        .route("/api/torrents/:id/files", post(set_files))
        .route("/api/torrents/:id/pause", post(pause))
        .route("/api/torrents/:id/resume", post(resume))
        .route("/api/torrents/:id/delete", post(delete))
        .route("/download/:id/:file", get(download))
        .route("/stream/:id/:file", get(stream))
        .route("/transcode/:id/:file", get(transcode))
        .route("/transcode-dl/:id", get(transcode_dl))
        .route("/subtitle/:id/:file", get(subtitle))
        .route("/api/links", get(links_list))
        .route("/api/links/:id/delete", post(links_delete))
        .route("/dl/:id", get(direct_file))
        .route("/api/me", get(me))
        .route("/api/me/password", post(change_password))
        .route("/api/logout", post(logout))
        .route("/api/account", get(account_get))
        .route("/api/account/token", post(account_token))
        .route("/api/account/logout-others", post(account_logout_others))
        .route("/api/account/2fa/start", post(account_2fa_start))
        .route("/api/account/2fa/confirm", post(account_2fa_confirm))
        .route("/api/account/2fa/disable", post(account_2fa_disable))
        .route("/api/users", get(users_list).post(users_create))
        .route("/api/users/:username/password", post(users_set_password))
        .route("/api/users/:username/delete", post(users_delete))
        .route("/api/users/:username/disabled", post(users_set_disabled))
        .route("/api/users/:username/token", post(users_reset_token))
        .route(
            "/api/admin/invites",
            get(admin_invites).post(admin_invite_create),
        )
        .route("/api/admin/invites/:code/delete", post(admin_invite_delete))
        .route(
            "/api/admin/config",
            get(admin_config_get).post(admin_config_set),
        )
        .route("/api/admin/overview", get(admin_overview))
        .route("/api/admin/activity", get(admin_activity))
        .route("/api/admin/sessions", get(admin_sessions))
        .route(
            "/api/admin/sessions/:sid/revoke",
            post(admin_session_revoke),
        )
        .route(
            "/api/admin/sessions/revoke-others",
            post(admin_revoke_others),
        )
        .route("/api/admin/torrents/pause-all", post(admin_pause_all))
        .route("/api/admin/torrents/resume-all", post(admin_resume_all))
        .route(
            "/api/admin/downloads/clear-completed",
            post(admin_clear_completed),
        )
        .route("/api/admin/cleanup", post(admin_cleanup))
        .route_layer(middleware::from_fn_with_state(auth, require_auth));

    // Public: first-run setup + login + invite-only registration + PWA assets.
    let public = Router::new()
        .route("/manifest.webmanifest", get(manifest))
        .route("/sw.js", get(service_worker))
        .route("/icon.svg", get(app_icon))
        .route("/setup", get(setup_page))
        .route("/api/setup", post(api_setup))
        .route("/login", get(login_page))
        .route("/api/login", post(api_login))
        .route("/api/login/2fa", post(api_login_2fa))
        .route("/register", get(register_page))
        .route("/api/register", post(api_register))
        .route("/api/register/status", get(register_status));

    public
        .merge(protected)
        .layer(middleware::from_fn(security_headers))
        .with_state(state)
}

/// Add hardening headers to every response.
async fn security_headers(req: Request, next: Next) -> Response {
    let mut resp = next.run(req).await;
    let h = resp.headers_mut();
    h.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    h.insert("x-frame-options", HeaderValue::from_static("DENY"));
    h.insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    h.insert(
        "permissions-policy",
        HeaderValue::from_static("camera=(), microphone=(), geolocation=()"),
    );
    // Content-Security-Policy: defense-in-depth. The embedded UI keeps inline
    // scripts/handlers, so script/style allow 'unsafe-inline'; everything else
    // is locked to same-origin, plugins blocked, framing/base/form restricted.
    // User-supplied values are still HTML-escaped at the source.
    h.insert(
        "content-security-policy",
        HeaderValue::from_static(
            "default-src 'self'; img-src 'self' data:; media-src 'self' blob:; \
             script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; \
             connect-src 'self'; object-src 'none'; base-uri 'self'; \
             frame-ancestors 'none'; form-action 'self'",
        ),
    );
    resp
}

async fn index() -> Html<&'static str> {
    Html(include_str!("web/index.html"))
}

// ---- PWA assets (public, so the app is installable from any page) ----

async fn manifest() -> Response {
    (
        [(header::CONTENT_TYPE, "application/manifest+json")],
        include_str!("web/manifest.webmanifest"),
    )
        .into_response()
}

async fn service_worker() -> Response {
    (
        [(header::CONTENT_TYPE, "application/javascript")],
        include_str!("web/sw.js"),
    )
        .into_response()
}

async fn app_icon() -> Response {
    (
        [(header::CONTENT_TYPE, "image/svg+xml")],
        include_str!("web/icon.svg"),
    )
        .into_response()
}

async fn login_page(State(auth): State<Auth>, headers: HeaderMap) -> Response {
    if auth.needs_setup() {
        return Redirect::to("/setup").into_response();
    }
    if auth.identity_from_headers(&headers).is_some() {
        return Redirect::to("/").into_response();
    }
    Html(include_str!("web/login.html")).into_response()
}

// ---- first-run setup wizard ----

async fn setup_page(State(auth): State<Auth>) -> Response {
    if !auth.needs_setup() {
        return Redirect::to("/login").into_response();
    }
    Html(include_str!("web/setup.html")).into_response()
}

#[derive(Deserialize)]
struct SetupReq {
    username: String,
    password: String,
    token: String,
}

async fn api_setup(State(auth): State<Auth>, Json(req): Json<SetupReq>) -> Response {
    if !auth.needs_setup() {
        return err(
            StatusCode::CONFLICT,
            "MagnetBox is already set up — sign in.",
        );
    }
    if !auth.setup_token_ok(req.token.trim()) {
        return err(StatusCode::FORBIDDEN, "Incorrect setup code.");
    }
    match auth.complete_setup(req.username.trim(), &req.password) {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => err(StatusCode::BAD_REQUEST, &format!("{e}")),
    }
}

async fn admin_page() -> Html<&'static str> {
    Html(include_str!("web/admin.html"))
}

async fn settings_page() -> Html<&'static str> {
    Html(include_str!("web/settings.html"))
}

// ---- app settings: global speed limits (admin only, gated in middleware) ----

async fn settings_get(State(app): State<App>) -> Json<serde_json::Value> {
    let (download_bps, upload_bps) = app.rate_limits();
    Json(json!({ "download_bps": download_bps, "upload_bps": upload_bps }))
}

#[derive(Deserialize)]
struct LimitsReq {
    /// bytes/sec; null or 0 means unlimited.
    download_bps: Option<u32>,
    upload_bps: Option<u32>,
}

async fn settings_set(State(app): State<App>, Json(req): Json<LimitsReq>) -> Response {
    let norm = |v: Option<u32>| v.filter(|n| *n > 0);
    app.set_rate_limits(norm(req.download_bps), norm(req.upload_bps));
    Json(json!({ "ok": true })).into_response()
}

// ---- auth endpoints ----

#[derive(Deserialize)]
struct LoginReq {
    username: String,
    password: String,
}

async fn api_login(State(auth): State<Auth>, Json(req): Json<LoginReq>) -> Response {
    match auth.login(req.username.trim(), &req.password) {
        Ok(LoginOutcome::Session(token)) => {
            auth.log_event(req.username.trim(), "signed in");
            session_cookie_response(&auth, &token)
        }
        Ok(LoginOutcome::TwoFactor(challenge)) => {
            Json(json!({ "ok": true, "twofa": true, "challenge": challenge })).into_response()
        }
        Err(e) => (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "ok": false, "error": format!("{e}") })),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
struct TwoFactorReq {
    challenge: String,
    code: String,
}

async fn api_login_2fa(State(auth): State<Auth>, Json(req): Json<TwoFactorReq>) -> Response {
    match auth.verify_2fa(&req.challenge, &req.code) {
        Ok(token) => session_cookie_response(&auth, &token),
        Err(e) => (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "ok": false, "error": format!("{e}") })),
        )
            .into_response(),
    }
}

fn session_cookie_response(auth: &Auth, token: &str) -> Response {
    let mut h = HeaderMap::new();
    if let Ok(v) = HeaderValue::from_str(&auth.make_cookie(token)) {
        h.insert(header::SET_COOKIE, v);
    }
    (h, Json(json!({ "ok": true }))).into_response()
}

async fn register_page() -> Html<&'static str> {
    Html(include_str!("web/register.html"))
}

async fn register_status(State(auth): State<Auth>) -> Json<serde_json::Value> {
    Json(json!({ "open": auth.registration_open() }))
}

#[derive(Deserialize)]
struct RegisterReq {
    code: String,
    username: String,
    email: String,
    password: String,
}

async fn api_register(State(auth): State<Auth>, Json(req): Json<RegisterReq>) -> Response {
    match auth.register(&req.code, &req.username, &req.email, &req.password) {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => err(StatusCode::BAD_REQUEST, &format!("{e}")),
    }
}

async fn logout(State(auth): State<Auth>, headers: HeaderMap) -> Response {
    if let Some(tok) = token_from_headers(&headers) {
        auth.logout(&tok);
    }
    let mut h = HeaderMap::new();
    if let Ok(v) = HeaderValue::from_str(&auth.clear_cookie()) {
        h.insert(header::SET_COOKIE, v);
    }
    (h, Json(json!({ "ok": true }))).into_response()
}

async fn me(Extension(id): Extension<Identity>) -> Json<serde_json::Value> {
    Json(json!({ "username": id.username, "role": id.role }))
}

#[derive(Deserialize)]
struct ChangePw {
    old: String,
    new: String,
}

async fn change_password(
    State(auth): State<Auth>,
    Extension(id): Extension<Identity>,
    Json(req): Json<ChangePw>,
) -> Response {
    match auth.change_own_password(&id.username, &req.old, &req.new) {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => err(StatusCode::BAD_REQUEST, &format!("{e}")),
    }
}

// ---- admin: user management (gated to admins by the middleware) ----

async fn users_list(State(auth): State<Auth>) -> Json<serde_json::Value> {
    Json(json!({ "users": auth.list_users() }))
}

#[derive(Deserialize)]
struct NewUser {
    username: String,
    password: String,
    #[serde(default)]
    role: Option<String>,
}

async fn users_create(State(auth): State<Auth>, Json(req): Json<NewUser>) -> Response {
    let role = match req.role.as_deref() {
        Some("admin") => Role::Admin,
        _ => Role::User,
    };
    match auth.add_user(req.username.trim(), &req.password, role) {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => err(StatusCode::BAD_REQUEST, &format!("{e}")),
    }
}

#[derive(Deserialize)]
struct PwReq {
    password: String,
}

async fn users_set_password(
    State(auth): State<Auth>,
    Path(username): Path<String>,
    Json(req): Json<PwReq>,
) -> Response {
    match auth.set_password(&username, &req.password) {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => err(StatusCode::BAD_REQUEST, &format!("{e}")),
    }
}

async fn users_delete(State(auth): State<Auth>, Path(username): Path<String>) -> Response {
    match auth.delete_user(&username) {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => err(StatusCode::BAD_REQUEST, &format!("{e}")),
    }
}

#[derive(Deserialize)]
struct DisabledReq {
    disabled: bool,
}

async fn users_set_disabled(
    State(auth): State<Auth>,
    Path(username): Path<String>,
    Json(req): Json<DisabledReq>,
) -> Response {
    match auth.set_disabled(&username, req.disabled) {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => err(StatusCode::BAD_REQUEST, &format!("{e}")),
    }
}

async fn users_reset_token(State(auth): State<Auth>, Path(username): Path<String>) -> Response {
    match auth.regenerate_token(&username) {
        Ok(token) => Json(json!({ "ok": true, "token": token })).into_response(),
        Err(e) => err(StatusCode::BAD_REQUEST, &format!("{e}")),
    }
}

// ---- invite-only registration controls ----

async fn admin_invites(State(auth): State<Auth>) -> Json<serde_json::Value> {
    Json(json!({ "invites": auth.list_invites() }))
}

#[derive(Deserialize)]
struct NewInvite {
    #[serde(default)]
    max_uses: u32,
    #[serde(default)]
    expires_days: u32,
    #[serde(default)]
    note: String,
}

async fn admin_invite_create(
    State(auth): State<Auth>,
    Extension(id): Extension<Identity>,
    Json(req): Json<NewInvite>,
) -> Response {
    let invite = auth.create_invite(&id.username, req.max_uses, req.expires_days, &req.note);
    Json(json!({ "ok": true, "code": invite.code })).into_response()
}

async fn admin_invite_delete(State(auth): State<Auth>, Path(code): Path<String>) -> Response {
    auth.delete_invite(&code);
    Json(json!({ "ok": true })).into_response()
}

async fn admin_config_get(State(auth): State<Auth>) -> Json<serde_json::Value> {
    Json(auth.config_json())
}

#[derive(Deserialize)]
struct ConfigReq {
    #[serde(default)]
    registration_open: bool,
    #[serde(default)]
    maintenance: bool,
    #[serde(default)]
    max_concurrent_downloads: u32,
    #[serde(default)]
    retention_days: u32,
}

async fn admin_config_set(State(auth): State<Auth>, Json(req): Json<ConfigReq>) -> Response {
    auth.set_config(
        req.registration_open,
        req.maintenance,
        req.max_concurrent_downloads,
        req.retention_days,
    );
    Json(json!({ "ok": true })).into_response()
}

/// Run retention cleanup now (admin "clean up" button), using the configured days.
async fn admin_cleanup(State(s): State<AppState>) -> Response {
    let days = s.auth.retention_days();
    if days == 0 {
        return Json(
            json!({ "ok": true, "torrents": 0, "downloads": 0, "note": "retention disabled" }),
        )
        .into_response();
    }
    let secs = days as u64 * 86_400;
    let torrents = s.engine.remove_expired(secs).await;
    let downloads = s.direct.remove_expired(secs);
    Json(json!({ "ok": true, "torrents": torrents, "downloads": downloads })).into_response()
}

// ---- admin console (admin-gated by the middleware) ----

async fn admin_overview(State(s): State<AppState>) -> Json<serde_json::Value> {
    let engine = s.engine.summary();
    let downloads = s.direct.list();
    let dl_running = downloads
        .iter()
        .filter(|d| d.status == "downloading")
        .count();
    let dl_done = downloads.iter().filter(|d| d.status == "done").count();
    let dl_bytes: u64 = downloads.iter().map(|d| d.downloaded).sum();
    let (users, sessions) = s.auth.stats();
    let eng_bytes = engine
        .get("downloaded_bytes")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    Json(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "uptime_secs": s.started.elapsed().as_secs(),
        "engine": engine,
        "downloads": {
            "total": downloads.len(),
            "running": dl_running,
            "done": dl_done,
            "downloaded_bytes": dl_bytes,
        },
        "users": users,
        "sessions": sessions,
        "stored_bytes": eng_bytes + dl_bytes,
        "host": s.metrics.snapshot(),
    }))
}

async fn admin_activity(State(auth): State<Auth>) -> Json<serde_json::Value> {
    Json(json!({ "events": auth.recent_events() }))
}

async fn admin_sessions(State(auth): State<Auth>, headers: HeaderMap) -> Json<serde_json::Value> {
    let current = token_from_headers(&headers).unwrap_or_default();
    Json(json!({ "sessions": auth.list_sessions(&current) }))
}

async fn admin_session_revoke(State(auth): State<Auth>, Path(sid): Path<String>) -> Response {
    auth.revoke_session(&sid);
    Json(json!({ "ok": true })).into_response()
}

async fn admin_revoke_others(State(auth): State<Auth>, headers: HeaderMap) -> Response {
    let current = token_from_headers(&headers).unwrap_or_default();
    auth.revoke_all_except(&current);
    Json(json!({ "ok": true })).into_response()
}

async fn admin_pause_all(State(app): State<App>) -> Response {
    app.pause_all().await;
    Json(json!({ "ok": true })).into_response()
}

async fn admin_resume_all(State(app): State<App>) -> Response {
    app.resume_all().await;
    Json(json!({ "ok": true })).into_response()
}

async fn admin_clear_completed(State(direct): State<DirectManager>) -> Response {
    let removed = direct.clear_completed();
    Json(json!({ "ok": true, "removed": removed })).into_response()
}

async fn list(State(app): State<App>) -> Json<serde_json::Value> {
    let (adding, errors) = app.take_status();
    Json(json!({ "torrents": app.list(), "adding": adding, "errors": errors }))
}

#[derive(Deserialize)]
struct AddReq {
    source: String,
    /// Add without starting the download, so files can be picked first.
    #[serde(default)]
    paused: bool,
    /// Add a set of public trackers to help find peers (skip for private trackers).
    #[serde(default)]
    trackers: bool,
}

/// Add a magnet, a `.torrent` URL, or a plain `http(s)` download link. The kind
/// is auto-detected so one box handles both torrents and direct links.
async fn add(
    State(app): State<App>,
    State(direct): State<DirectManager>,
    State(auth): State<Auth>,
    Extension(id): Extension<Identity>,
    Json(req): Json<AddReq>,
) -> Response {
    let src = req.source.trim().to_string();
    if src.is_empty() {
        return err(StatusCode::BAD_REQUEST, "empty source");
    }
    let lower = src.to_ascii_lowercase();
    let is_torrent =
        lower.starts_with("magnet:") || lower.ends_with(".torrent") || lower.contains(".torrent?");

    if is_torrent {
        app.spawn_add(AddTorrent::from_url(src), req.paused, req.trackers);
        Json(json!({ "ok": true, "kind": "torrent" })).into_response()
    } else if lower.starts_with("http://") || lower.starts_with("https://") {
        let limit = auth.max_concurrent();
        if limit > 0 && direct.active_count(&id.username) >= limit as usize {
            return err(
                StatusCode::TOO_MANY_REQUESTS,
                &format!("you already have {limit} downloads in progress — wait for one to finish"),
            );
        }
        match direct.add(src, id.username.clone()) {
            Ok(new_id) => Json(json!({ "ok": true, "kind": "link", "id": new_id })).into_response(),
            Err(e) => err(StatusCode::BAD_REQUEST, &format!("{e}")),
        }
    } else {
        err(
            StatusCode::BAD_REQUEST,
            "enter a magnet link, a .torrent URL, or an http(s) download link",
        )
    }
}

#[derive(Deserialize)]
struct UploadParams {
    #[serde(default)]
    paused: bool,
    #[serde(default)]
    trackers: bool,
}

/// Add by uploading raw `.torrent` bytes (request body is the file).
async fn upload(State(app): State<App>, Query(q): Query<UploadParams>, body: Bytes) -> Response {
    if body.is_empty() {
        return err(StatusCode::BAD_REQUEST, "empty .torrent upload");
    }
    app.spawn_add(AddTorrent::from_bytes(body), q.paused, q.trackers);
    Json(json!({ "ok": true })).into_response()
}

#[derive(Deserialize)]
struct FilesReq {
    files: Vec<usize>,
}

/// Replace the set of files selected for download.
async fn set_files(
    State(app): State<App>,
    Path(id): Path<usize>,
    Json(req): Json<FilesReq>,
) -> Response {
    match app.set_files(id, req.files).await {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => err(StatusCode::BAD_REQUEST, &format!("{e}")),
    }
}

async fn pause(State(app): State<App>, Path(id): Path<usize>) -> Response {
    match app.pause(id).await {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => err(StatusCode::BAD_REQUEST, &format!("{e}")),
    }
}

async fn resume(State(app): State<App>, Path(id): Path<usize>) -> Response {
    match app.resume(id).await {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => err(StatusCode::BAD_REQUEST, &format!("{e}")),
    }
}

#[derive(Deserialize)]
struct DeleteParams {
    /// Also erase downloaded data from disk.
    #[serde(default)]
    files: bool,
}

async fn delete(
    State(app): State<App>,
    Path(id): Path<usize>,
    Query(q): Query<DeleteParams>,
) -> Response {
    match app.delete(id, q.files).await {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => err(StatusCode::BAD_REQUEST, &format!("{e}")),
    }
}

async fn download(
    State(app): State<App>,
    Path((id, file)): Path<(usize, usize)>,
    headers: HeaderMap,
) -> Response {
    serve(app, id, file, headers, true).await
}

async fn stream(
    State(app): State<App>,
    Path((id, file)): Path<(usize, usize)>,
    headers: HeaderMap,
) -> Response {
    serve(app, id, file, headers, false).await
}

/// Serve a torrent file over HTTP (range-aware; librqbit fetches needed pieces).
async fn serve(app: App, id: usize, file: usize, headers: HeaderMap, attachment: bool) -> Response {
    let Some(handle) = app.get(id) else {
        return err(StatusCode::NOT_FOUND, "no such torrent");
    };
    let Some(rel_name) = app.file_name(id, file) else {
        return err(StatusCode::NOT_FOUND, "no such file (metadata not ready?)");
    };
    let base = basename(&rel_name);
    let file_stream = match handle.clone().stream(file) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(torrent = id, file, error = %format!("{e:#}"), "stream open failed");
            return err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "could not open file for streaming",
            );
        }
    };
    let total = file_stream.len();
    let range = parse_range_header(&headers, total);
    let name = attachment.then_some(base.as_str());
    ranged(file_stream, total, content_type(&base), name, range).await
}

/// Serve a plain file from disk (used by direct-link downloads).
async fn serve_disk_file(
    path: &std::path::Path,
    name: &str,
    headers: HeaderMap,
    attachment: bool,
) -> Response {
    let file = match tokio::fs::File::open(path).await {
        Ok(f) => f,
        Err(_) => return err(StatusCode::NOT_FOUND, "file not available yet"),
    };
    let total = file.metadata().await.map(|m| m.len()).unwrap_or(0);
    let range = parse_range_header(&headers, total);
    ranged(
        file,
        total,
        content_type(name),
        attachment.then_some(name),
        range,
    )
    .await
}

/// Build a (possibly partial) file response from any seekable async reader.
async fn ranged<R>(
    mut reader: R,
    total: u64,
    ctype: &str,
    attachment_name: Option<&str>,
    range: Option<(u64, u64)>,
) -> Response
where
    R: AsyncRead + AsyncSeek + Unpin + Send + 'static,
{
    let mut out = HeaderMap::new();
    out.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    if let Ok(v) = HeaderValue::from_str(ctype) {
        out.insert(header::CONTENT_TYPE, v);
    }
    if let Some(name) = attachment_name {
        if let Ok(v) = HeaderValue::from_str(&format!(
            "attachment; filename=\"{}\"",
            name.replace('"', "")
        )) {
            out.insert(header::CONTENT_DISPOSITION, v);
        }
    }

    match range {
        Some((start, end)) => {
            let len = end - start + 1;
            if let Err(e) = reader.seek(SeekFrom::Start(start)).await {
                tracing::error!(error = %e, "range seek failed");
                return err(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "could not read the requested range",
                );
            }
            out.insert(header::CONTENT_LENGTH, HeaderValue::from(len));
            if let Ok(v) = HeaderValue::from_str(&format!("bytes {start}-{end}/{total}")) {
                out.insert(header::CONTENT_RANGE, v);
            }
            let body = Body::from_stream(ReaderStream::new(reader.take(len)));
            (StatusCode::PARTIAL_CONTENT, out, body).into_response()
        }
        None => {
            out.insert(header::CONTENT_LENGTH, HeaderValue::from(total));
            let body = Body::from_stream(ReaderStream::new(reader));
            (StatusCode::OK, out, body).into_response()
        }
    }
}

fn basename(rel: &str) -> String {
    std::path::Path::new(rel)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| rel.to_string())
}

fn parse_range_header(headers: &HeaderMap, total: u64) -> Option<(u64, u64)> {
    headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(|h| parse_range(h, total))
}

// ---- direct-link downloads ----

async fn links_list(State(direct): State<DirectManager>) -> Json<serde_json::Value> {
    Json(json!({ "downloads": direct.list() }))
}

async fn links_delete(
    State(direct): State<DirectManager>,
    Path(id): Path<usize>,
    Query(q): Query<DeleteParams>,
) -> Response {
    match direct.delete(id, q.files) {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => err(StatusCode::BAD_REQUEST, &format!("{e}")),
    }
}

async fn direct_file(
    State(direct): State<DirectManager>,
    Path(id): Path<usize>,
    headers: HeaderMap,
) -> Response {
    let Some((path, name)) = direct.path_of(id) else {
        return err(StatusCode::NOT_FOUND, "no such download");
    };
    serve_disk_file(&path, &name, headers, true).await
}

// ---- subtitles: convert a torrent's .srt to WebVTT for the <video> player ----

async fn subtitle(State(app): State<App>, Path((id, file)): Path<(usize, usize)>) -> Response {
    let Some(handle) = app.get(id) else {
        return err(StatusCode::NOT_FOUND, "no such torrent");
    };
    let stream = match handle.clone().stream(file) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(torrent = id, file, error = %format!("{e:#}"), "subtitle stream open failed");
            return err(StatusCode::INTERNAL_SERVER_ERROR, "could not open subtitle");
        }
    };
    let mut buf = Vec::new();
    // Subtitles are tiny; cap the read so a wrong index can't pull a huge file.
    if stream.take(8_000_000).read_to_end(&mut buf).await.is_err() {
        return err(StatusCode::INTERNAL_SERVER_ERROR, "could not read subtitle");
    }
    let vtt = srt_to_vtt(&String::from_utf8_lossy(&buf));
    ([(header::CONTENT_TYPE, "text/vtt; charset=utf-8")], vtt).into_response()
}

fn srt_to_vtt(srt: &str) -> String {
    let mut out = String::with_capacity(srt.len() + 16);
    out.push_str("WEBVTT\n\n");
    for line in srt.lines() {
        if line.contains("-->") {
            out.push_str(&line.replace(',', "."));
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }
    out
}

// ---- account + API token ----

async fn account_page() -> Html<&'static str> {
    Html(include_str!("web/account.html"))
}

async fn docs_page() -> Html<&'static str> {
    Html(include_str!("web/docs.html"))
}

async fn openapi_json() -> Response {
    (
        [(header::CONTENT_TYPE, "application/json")],
        include_str!("web/openapi.json"),
    )
        .into_response()
}

async fn account_get(State(auth): State<Auth>, Extension(id): Extension<Identity>) -> Response {
    match auth.account(&id.username) {
        Some(v) => Json(v).into_response(),
        None => err(StatusCode::NOT_FOUND, "no such user"),
    }
}

async fn account_token(State(auth): State<Auth>, Extension(id): Extension<Identity>) -> Response {
    match auth.regenerate_token(&id.username) {
        Ok(token) => Json(json!({ "ok": true, "token": token })).into_response(),
        Err(e) => err(StatusCode::BAD_REQUEST, &format!("{e}")),
    }
}

async fn account_logout_others(
    State(auth): State<Auth>,
    Extension(id): Extension<Identity>,
    headers: HeaderMap,
) -> Response {
    let keep = token_from_headers(&headers).unwrap_or_default();
    auth.logout_others(&id.username, &keep);
    Json(json!({ "ok": true })).into_response()
}

async fn account_2fa_start(
    State(auth): State<Auth>,
    Extension(id): Extension<Identity>,
) -> Response {
    match auth.start_enroll_2fa(&id.username) {
        Ok((secret, url, qr)) => {
            Json(json!({ "ok": true, "secret": secret, "url": url, "qr": qr })).into_response()
        }
        Err(e) => err(StatusCode::BAD_REQUEST, &format!("{e}")),
    }
}

#[derive(Deserialize)]
struct CodeReq {
    code: String,
}

async fn account_2fa_confirm(
    State(auth): State<Auth>,
    Extension(id): Extension<Identity>,
    Json(req): Json<CodeReq>,
) -> Response {
    match auth.confirm_2fa(&id.username, &req.code) {
        Ok(codes) => Json(json!({ "ok": true, "recovery_codes": codes })).into_response(),
        Err(e) => err(StatusCode::BAD_REQUEST, &format!("{e}")),
    }
}

#[derive(Deserialize)]
struct PasswordReq {
    password: String,
}

async fn account_2fa_disable(
    State(auth): State<Auth>,
    Extension(id): Extension<Identity>,
    Json(req): Json<PasswordReq>,
) -> Response {
    match auth.disable_2fa(&id.username, &req.password) {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => err(StatusCode::BAD_REQUEST, &format!("{e}")),
    }
}

/// Parse the first byte range from a `Range` header into an inclusive
/// `(start, end)`. Supports `bytes=a-b`, `bytes=a-`, and `bytes=-suffix`.
fn parse_range(h: &str, total: u64) -> Option<(u64, u64)> {
    if total == 0 {
        return None;
    }
    let spec = h.strip_prefix("bytes=")?;
    let part = spec.split(',').next()?.trim();
    let (s, e) = part.split_once('-')?;

    let (start, end) = if s.is_empty() {
        // suffix range: last N bytes
        let n: u64 = e.parse().ok()?;
        if n == 0 {
            return None;
        }
        let n = n.min(total);
        (total - n, total - 1)
    } else {
        let start: u64 = s.parse().ok()?;
        let end: u64 = if e.is_empty() {
            total - 1
        } else {
            e.parse::<u64>().ok()?.min(total - 1)
        };
        (start, end)
    };

    if start > end || start >= total {
        return None;
    }
    Some((start, end))
}

// ---- on-the-fly transcoding (optional, requires ffmpeg on PATH) ----

/// Is `ffmpeg` available on PATH? Checked once at startup.
fn ffmpeg_available() -> bool {
    std::process::Command::new("ffmpeg")
        .arg("-version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// ffmpeg output args: H.264 + AAC in fragmented MP4 on stdout, so the browser
/// can play (almost) anything and start before the conversion finishes.
const FFMPEG_OUT: &[&str] = &[
    "-c:v",
    "libx264",
    "-preset",
    "veryfast",
    "-crf",
    "23",
    "-pix_fmt",
    "yuv420p",
    "-c:a",
    "aac",
    "-b:a",
    "160k",
    "-ac",
    "2",
    "-movflags",
    "frag_keyframe+empty_moov+default_base_moof",
    "-f",
    "mp4",
    "pipe:1",
];

/// Max simultaneous transcodes per user — each one runs an ffmpeg encode, so
/// this caps the CPU a single account can demand.
const MAX_TRANSCODES_PER_USER: usize = 2;

/// RAII slot for one in-flight transcode; dropping it frees the user's slot.
struct TranscodeGuard {
    map: Arc<Mutex<HashMap<String, usize>>>,
    user: String,
}
impl Drop for TranscodeGuard {
    fn drop(&mut self) {
        let mut m = self.map.lock().unwrap();
        if let Some(n) = m.get_mut(&self.user) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                m.remove(&self.user);
            }
        }
    }
}

/// Reserve a transcode slot for `user`, or `None` if already at the cap.
fn acquire_transcode(state: &AppState, user: &str) -> Option<TranscodeGuard> {
    let mut m = state.transcodes.lock().unwrap();
    let n = m.entry(user.to_string()).or_insert(0);
    if *n >= MAX_TRANSCODES_PER_USER {
        return None;
    }
    *n += 1;
    Some(TranscodeGuard {
        map: state.transcodes.clone(),
        user: user.to_string(),
    })
}

/// Transcode a torrent file: pipe its piece-aware stream (which blocks until
/// pieces arrive) through ffmpeg and serve the browser-friendly result.
async fn transcode(
    State(state): State<AppState>,
    Extension(ident): Extension<Identity>,
    Path((id, file)): Path<(usize, usize)>,
) -> Response {
    if !state.ffmpeg {
        return err(
            StatusCode::SERVICE_UNAVAILABLE,
            "transcoding unavailable — install ffmpeg on the server",
        );
    }
    let Some(guard) = acquire_transcode(&state, &ident.username) else {
        return err(
            StatusCode::TOO_MANY_REQUESTS,
            "too many active conversions — close another and retry",
        );
    };
    let Some(handle) = state.engine.get(id) else {
        return err(StatusCode::NOT_FOUND, "no such torrent");
    };
    if state.engine.file_name(id, file).is_none() {
        return err(StatusCode::NOT_FOUND, "no such file (metadata not ready?)");
    }
    let stream = match handle.clone().stream(file) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(torrent = id, file, error = %format!("{e:#}"), "transcode stream open failed");
            return err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "could not open file for transcoding",
            );
        }
    };
    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-hide_banner", "-loglevel", "error", "-i", "pipe:0"])
        .args(FFMPEG_OUT)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    spawn_transcode_reader(cmd, stream, guard)
}

/// Transcode a finished direct-link download (read from disk — seekable).
async fn transcode_dl(
    State(state): State<AppState>,
    Extension(ident): Extension<Identity>,
    Path(id): Path<usize>,
) -> Response {
    if !state.ffmpeg {
        return err(
            StatusCode::SERVICE_UNAVAILABLE,
            "transcoding unavailable — install ffmpeg on the server",
        );
    }
    let Some(guard) = acquire_transcode(&state, &ident.username) else {
        return err(
            StatusCode::TOO_MANY_REQUESTS,
            "too many active conversions — close another and retry",
        );
    };
    let Some((path, _name)) = state.direct.path_of(id) else {
        return err(StatusCode::NOT_FOUND, "no such download");
    };
    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-hide_banner", "-loglevel", "error", "-i"])
        .arg(&path)
        .args(FFMPEG_OUT)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    spawn_transcode_path(cmd, guard)
}

/// Spawn ffmpeg, feeding `reader` (a non-seekable piece stream) into its stdin,
/// and stream stdout as the response.
fn spawn_transcode_reader<R>(mut cmd: Command, reader: R, guard: TranscodeGuard) -> Response
where
    R: AsyncRead + Unpin + Send + 'static,
{
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(error = %e, "failed to start ffmpeg");
            return err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "could not start the converter",
            );
        }
    };
    if let Some(mut stdin) = child.stdin.take() {
        tokio::spawn(async move {
            let mut reader = reader;
            let _ = tokio::io::copy(&mut reader, &mut stdin).await;
        });
    }
    finish_transcode(child, guard)
}

/// Spawn ffmpeg that reads its input itself (e.g. a seekable file path).
fn spawn_transcode_path(mut cmd: Command, guard: TranscodeGuard) -> Response {
    match cmd.spawn() {
        Ok(child) => finish_transcode(child, guard),
        Err(e) => {
            tracing::error!(error = %e, "failed to start ffmpeg");
            err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "could not start the converter",
            )
        }
    }
}

/// Stream ffmpeg stdout as the response; reap the child so it never lingers
/// (`kill_on_drop` covers client disconnects and shutdown).
fn finish_transcode(mut child: Child, guard: TranscodeGuard) -> Response {
    let Some(stdout) = child.stdout.take() else {
        return err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "converter produced no output",
        );
    };
    tokio::spawn(async move {
        let _guard = guard; // releases the per-user slot once ffmpeg exits
        let _ = child.wait().await;
    });
    transcode_response(stdout)
}

fn transcode_response(stdout: ChildStdout) -> Response {
    let mut out = HeaderMap::new();
    out.insert(header::CONTENT_TYPE, HeaderValue::from_static("video/mp4"));
    out.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    out.insert(header::ACCEPT_RANGES, HeaderValue::from_static("none"));
    let body = Body::from_stream(ReaderStream::new(stdout));
    (StatusCode::OK, out, body).into_response()
}

fn content_type(name: &str) -> &'static str {
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "mp4" | "m4v" => "video/mp4",
        "mkv" => "video/x-matroska",
        "webm" => "video/webm",
        "avi" => "video/x-msvideo",
        "mov" => "video/quicktime",
        "ts" => "video/mp2t",
        "mp3" => "audio/mpeg",
        "flac" => "audio/flac",
        "m4a" => "audio/mp4",
        "aac" => "audio/aac",
        "ogg" | "oga" => "audio/ogg",
        "opus" => "audio/opus",
        "wav" => "audio/wav",
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "pdf" => "application/pdf",
        "srt" | "txt" | "nfo" => "text/plain; charset=utf-8",
        "zip" => "application/zip",
        _ => "application/octet-stream",
    }
}

fn err(code: StatusCode, msg: &str) -> Response {
    (code, Json(json!({ "ok": false, "error": msg }))).into_response()
}
