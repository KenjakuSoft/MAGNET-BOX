//! Authentication: argon2-hashed users, server-side cookie sessions, login
//! throttling, and the axum middleware that gates the whole app.
//!
//! Users are persisted to a JSON file; sessions live in memory (cleared on
//! restart, which just means everyone re-logs in). Cookies are HttpOnly +
//! SameSite=Strict, and mutating requests get an Origin check — together that
//! covers CSRF for a same-origin app.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Context, Result};
use argon2::password_hash::rand_core::{OsRng, RngCore};
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use axum::{
    extract::{Request, State},
    http::{header, HeaderMap, StatusCode},
    middleware::Next,
    response::{IntoResponse, Json, Redirect, Response},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use totp_rs::{Algorithm, Secret, TOTP};

const COOKIE: &str = "mb_session";
const SESSION_TTL: Duration = Duration::from_secs(60 * 60 * 24 * 7); // 7 days
const MAX_FAILS: u32 = 5;
const FAIL_WINDOW: Duration = Duration::from_secs(15 * 60);
const MIN_PASSWORD_LEN: usize = 8;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Admin,
    User,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct User {
    pub username: String,
    pub password_hash: String,
    pub role: Role,
    pub created: u64,
    /// Optional API token for headless/automation access.
    #[serde(default)]
    pub api_token: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    /// Disabled (banned) accounts cannot log in and their sessions are rejected.
    #[serde(default)]
    pub disabled: bool,
    /// base32 TOTP secret; `Some` = two-factor enabled.
    #[serde(default)]
    pub totp_secret: Option<String>,
    /// SHA-256 hashes of one-time recovery codes.
    #[serde(default)]
    pub recovery_hashes: Vec<String>,
}

/// Result of a password check: either a ready session, or a 2FA challenge.
pub enum LoginOutcome {
    Session(String),
    TwoFactor(String),
}

/// An invite code (closed, invite-only registration).
#[derive(Clone, Serialize, Deserialize)]
pub struct Invite {
    pub code: String,
    pub created_by: String,
    pub created: u64,
    /// 0 = unlimited uses.
    pub max_uses: u32,
    pub used: u32,
    /// Unix expiry, or None for no expiry.
    pub expires: Option<u64>,
    pub note: String,
}

/// Instance-wide access config (persisted).
#[derive(Clone, Serialize, Deserialize)]
pub struct AuthConfig {
    #[serde(default)]
    pub registration_open: bool,
    #[serde(default)]
    pub maintenance: bool,
    /// Max simultaneous direct downloads per user (0 = unlimited).
    #[serde(default = "default_concurrency")]
    pub max_concurrent_downloads: u32,
    /// Auto-delete torrents/downloads older than this many days (0 = keep forever).
    #[serde(default)]
    pub retention_days: u32,
}

fn default_concurrency() -> u32 {
    3
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            registration_open: false,
            maintenance: false,
            max_concurrent_downloads: default_concurrency(),
            retention_days: 0,
        }
    }
}

/// Per-user activity. `bytes` (cumulative bandwidth served) is persisted;
/// last_seen/last_ip are refreshed live.
#[derive(Default, Clone, Serialize, Deserialize)]
struct Usage {
    #[serde(default)]
    last_seen: u64,
    #[serde(default)]
    last_ip: Option<String>,
    #[serde(default)]
    bytes: u64,
}

/// Logged-in identity, injected into request extensions by [`require_auth`].
#[derive(Clone)]
pub struct Identity {
    pub username: String,
    pub role: Role,
}

struct SessionData {
    id: String,
    username: String,
    role: Role,
    created: SystemTime,
    expires: SystemTime,
}

/// One audit-log entry.
#[derive(Clone, Serialize)]
pub struct Event {
    time: u64,
    user: String,
    action: String,
}

const MAX_EVENTS: usize = 300;

/// 2FA enrollment state: username -> (pending TOTP secret, expiry).
type PendingTwoFactor = HashMap<String, (String, SystemTime)>;
/// 2FA login step: challenge token -> (username, expiry, attempts).
type ChallengeMap = HashMap<String, (String, SystemTime, u32)>;

#[derive(Clone)]
pub struct Auth {
    path: PathBuf,
    invites_path: PathBuf,
    config_path: PathBuf,
    usage_path: PathBuf,
    secure_cookie: bool,
    users: Arc<Mutex<HashMap<String, User>>>,
    sessions: Arc<Mutex<HashMap<String, SessionData>>>,
    /// username -> (failure count, window start)
    throttle: Arc<Mutex<HashMap<String, (u32, SystemTime)>>>,
    /// Audit log (newest pushed to the back, capped at MAX_EVENTS).
    events: Arc<Mutex<VecDeque<Event>>>,
    invites: Arc<Mutex<HashMap<String, Invite>>>,
    config: Arc<Mutex<AuthConfig>>,
    usage: Arc<Mutex<HashMap<String, Usage>>>,
    /// username -> (pending TOTP secret, expiry) during enrollment.
    pending_2fa: Arc<Mutex<PendingTwoFactor>>,
    /// challenge token -> (username, expiry, attempts) for the 2FA login step.
    challenges: Arc<Mutex<ChallengeMap>>,
    /// One-time setup code, present only until the first admin is created via
    /// the web setup wizard. `None` once configured.
    setup_token: Arc<Mutex<Option<String>>>,
}

impl Auth {
    /// Load users from `path`; bootstrap an admin account on first run.
    pub fn load(path: PathBuf, secure_cookie: bool) -> Result<Self> {
        let users: HashMap<String, User> = if path.exists() {
            let data = std::fs::read(&path).context("reading users file")?;
            if data.is_empty() {
                HashMap::new()
            } else {
                let list: Vec<User> =
                    serde_json::from_slice(&data).context("parsing users file")?;
                list.into_iter().map(|u| (u.username.clone(), u)).collect()
            }
        } else {
            HashMap::new()
        };

        let dir = path.parent().map(|p| p.to_path_buf()).unwrap_or_default();
        let invites_path = dir.join("invites.json");
        let config_path = dir.join("auth_config.json");
        let usage_path = dir.join("usage.json");

        let invites: HashMap<String, Invite> = std::fs::read(&invites_path)
            .ok()
            .and_then(|d| serde_json::from_slice::<Vec<Invite>>(&d).ok())
            .map(|list| list.into_iter().map(|i| (i.code.clone(), i)).collect())
            .unwrap_or_default();
        let config: AuthConfig = std::fs::read(&config_path)
            .ok()
            .and_then(|d| serde_json::from_slice(&d).ok())
            .unwrap_or_default();
        let usage: HashMap<String, Usage> = std::fs::read(&usage_path)
            .ok()
            .and_then(|d| serde_json::from_slice(&d).ok())
            .unwrap_or_default();

        let auth = Self {
            path,
            invites_path,
            config_path,
            usage_path,
            secure_cookie,
            users: Arc::new(Mutex::new(users)),
            sessions: Default::default(),
            throttle: Default::default(),
            events: Default::default(),
            invites: Arc::new(Mutex::new(invites)),
            config: Arc::new(Mutex::new(config)),
            usage: Arc::new(Mutex::new(usage)),
            pending_2fa: Default::default(),
            challenges: Default::default(),
            setup_token: Default::default(),
        };

        auth.init_admin()?;
        Ok(auth)
    }

    fn save_invites(&self) {
        let list: Vec<Invite> = self.invites.lock().unwrap().values().cloned().collect();
        if let Ok(bytes) = serde_json::to_vec_pretty(&list) {
            let _ = write_private(&self.invites_path, &bytes);
        }
    }

    fn save_config(&self) {
        if let Ok(bytes) = serde_json::to_vec_pretty(&*self.config.lock().unwrap()) {
            let _ = write_private(&self.config_path, &bytes);
        }
    }

    /// Ensure an admin account exists.
    ///
    /// - If `MAGNETBOX_ADMIN_PASSWORD` is set (≥8 chars), the admin account is
    ///   created or **reset** to it on every startup. This is the recovery
    ///   path when you've lost the generated password — set the env var,
    ///   restart, log in, then unset it.
    /// - Otherwise, on a truly empty store, a random password is generated and
    ///   printed once.
    fn init_admin(&self) -> Result<()> {
        let username = std::env::var("MAGNETBOX_ADMIN_USER").unwrap_or_else(|_| "admin".into());
        let env_pw = std::env::var("MAGNETBOX_ADMIN_PASSWORD")
            .ok()
            .filter(|p| p.len() >= MIN_PASSWORD_LEN);
        let exists = self.users.lock().unwrap().contains_key(&username);

        if let Some(pw) = env_pw {
            if exists {
                self.set_password(&username, &pw)?;
                self.ensure_role(&username, Role::Admin)?;
                println!("  Admin '{username}' password reset from MAGNETBOX_ADMIN_PASSWORD.");
            } else {
                self.add_user(&username, &pw, Role::Admin)?;
                println!("  Created admin '{username}' from MAGNETBOX_ADMIN_PASSWORD.");
            }
            return Ok(());
        }

        // Fresh install: no admin yet. Instead of generating a throwaway
        // password, enter setup mode — the user creates their own account in
        // the browser, guarded by this one-time code (main.rs prints it).
        if self.users.lock().unwrap().is_empty() {
            *self.setup_token.lock().unwrap() = Some(gen_token());
        }
        Ok(())
    }

    /// True until the first admin account is created (fresh install).
    pub fn needs_setup(&self) -> bool {
        self.users.lock().unwrap().is_empty()
    }

    /// The one-time setup code (shown at startup), or `None` once configured.
    pub fn setup_token(&self) -> Option<String> {
        self.setup_token.lock().unwrap().clone()
    }

    /// True if `token` matches the active setup code (constant-time).
    pub fn setup_token_ok(&self, token: &str) -> bool {
        match &*self.setup_token.lock().unwrap() {
            Some(t) => ct_eq(t.as_bytes(), token.as_bytes()),
            None => false,
        }
    }

    /// Create the first admin from the setup wizard, then close setup mode.
    pub fn complete_setup(&self, username: &str, password: &str) -> Result<()> {
        if !self.needs_setup() {
            return Err(anyhow!("already set up"));
        }
        self.add_user(username, password, Role::Admin)?;
        *self.setup_token.lock().unwrap() = None;
        self.log_event(username, "completed first-run setup");
        Ok(())
    }

    fn ensure_role(&self, username: &str, role: Role) -> Result<()> {
        {
            let mut users = self.users.lock().unwrap();
            if let Some(u) = users.get_mut(username) {
                u.role = role;
            }
        }
        self.save()
    }

    fn save(&self) -> Result<()> {
        let list: Vec<User> = self.users.lock().unwrap().values().cloned().collect();
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).ok();
        }
        write_private(&self.path, &serde_json::to_vec_pretty(&list)?).context("writing users file")
    }

    // ---- user management ----

    pub fn list_users(&self) -> Vec<serde_json::Value> {
        let usage = self.usage.lock().unwrap();
        let mut v: Vec<_> = self
            .users
            .lock()
            .unwrap()
            .values()
            .map(|u| {
                let us = usage.get(&u.username);
                json!({
                    "username": u.username,
                    "role": u.role,
                    "created": u.created,
                    "email": u.email,
                    "disabled": u.disabled,
                    "last_seen": us.map(|x| x.last_seen).unwrap_or(0),
                    "last_ip": us.and_then(|x| x.last_ip.clone()),
                    "bytes_served": us.map(|x| x.bytes).unwrap_or(0),
                })
            })
            .collect();
        v.sort_by(|a, b| a["username"].as_str().cmp(&b["username"].as_str()));
        v
    }

    pub fn add_user(&self, username: &str, password: &str, role: Role) -> Result<()> {
        self.add_user_full(username, password, role, None)
    }

    fn add_user_full(
        &self,
        username: &str,
        password: &str,
        role: Role,
        email: Option<String>,
    ) -> Result<()> {
        let username = username.trim();
        if username.is_empty() {
            return Err(anyhow!("username required"));
        }
        if username.len() > 32
            || !username
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            return Err(anyhow!(
                "username may only contain letters, numbers, '_' and '-'"
            ));
        }
        if password.len() < MIN_PASSWORD_LEN {
            return Err(anyhow!(
                "password must be at least {MIN_PASSWORD_LEN} characters"
            ));
        }
        let mut users = self.users.lock().unwrap();
        if users.contains_key(username) {
            return Err(anyhow!("user '{username}' already exists"));
        }
        users.insert(
            username.to_string(),
            User {
                username: username.to_string(),
                password_hash: hash_password(password)?,
                role,
                created: now_unix(),
                api_token: None,
                email,
                disabled: false,
                totp_secret: None,
                recovery_hashes: Vec::new(),
            },
        );
        drop(users);
        self.save()
    }

    // ---- invite-only registration ----

    pub fn registration_open(&self) -> bool {
        self.config.lock().unwrap().registration_open
    }

    pub fn maintenance(&self) -> bool {
        self.config.lock().unwrap().maintenance
    }

    pub fn max_concurrent(&self) -> u32 {
        self.config.lock().unwrap().max_concurrent_downloads
    }

    pub fn retention_days(&self) -> u32 {
        self.config.lock().unwrap().retention_days
    }

    pub fn config_json(&self) -> serde_json::Value {
        let c = self.config.lock().unwrap();
        json!({
            "registration_open": c.registration_open,
            "maintenance": c.maintenance,
            "max_concurrent_downloads": c.max_concurrent_downloads,
            "retention_days": c.retention_days,
        })
    }

    pub fn set_config(
        &self,
        registration_open: bool,
        maintenance: bool,
        max_concurrent: u32,
        retention_days: u32,
    ) {
        {
            let mut c = self.config.lock().unwrap();
            c.registration_open = registration_open;
            c.maintenance = maintenance;
            c.max_concurrent_downloads = max_concurrent;
            c.retention_days = retention_days;
        }
        self.save_config();
    }

    /// Register a new (regular) user with a valid invite code.
    pub fn register(&self, code: &str, username: &str, email: &str, password: &str) -> Result<()> {
        if !self.registration_open() {
            return Err(anyhow!("registration is currently closed"));
        }
        let code = code.trim();
        // Validate the invite up front (without consuming it yet).
        {
            let invites = self.invites.lock().unwrap();
            let inv = invites
                .get(code)
                .ok_or_else(|| anyhow!("invalid invite code"))?;
            if matches!(inv.expires, Some(exp) if now_unix() > exp) {
                return Err(anyhow!("this invite code has expired"));
            }
            if inv.max_uses != 0 && inv.used >= inv.max_uses {
                return Err(anyhow!("this invite code has been fully used"));
            }
        }
        let email = email.trim();
        if email.len() < 3 || !email.contains('@') || !email.contains('.') {
            return Err(anyhow!("a valid email address is required"));
        }
        // Creates the account (also validates username/password/uniqueness).
        self.add_user_full(username, password, Role::User, Some(email.to_string()))?;
        // Consume one invite use.
        if let Some(inv) = self.invites.lock().unwrap().get_mut(code) {
            inv.used += 1;
        }
        self.save_invites();
        self.log_event(username.trim(), "registered with invite");
        Ok(())
    }

    pub fn create_invite(&self, by: &str, max_uses: u32, expires_days: u32, note: &str) -> Invite {
        let invite = Invite {
            code: gen_token()[..16].to_string(),
            created_by: by.to_string(),
            created: now_unix(),
            max_uses,
            used: 0,
            expires: (expires_days > 0).then(|| now_unix() + expires_days as u64 * 86400),
            note: note.trim().to_string(),
        };
        self.invites
            .lock()
            .unwrap()
            .insert(invite.code.clone(), invite.clone());
        self.save_invites();
        invite
    }

    pub fn list_invites(&self) -> Vec<serde_json::Value> {
        let mut v: Vec<(u64, serde_json::Value)> = self
            .invites
            .lock()
            .unwrap()
            .values()
            .map(|i| {
                let expired = matches!(i.expires, Some(e) if now_unix() > e);
                let used_up = i.max_uses != 0 && i.used >= i.max_uses;
                (
                    i.created,
                    json!({
                        "code": i.code, "created_by": i.created_by, "created": i.created,
                        "max_uses": i.max_uses, "used": i.used, "expires": i.expires,
                        "note": i.note, "active": !expired && !used_up,
                    }),
                )
            })
            .collect();
        v.sort_by(|a, b| b.0.cmp(&a.0));
        v.into_iter().map(|(_, j)| j).collect()
    }

    pub fn delete_invite(&self, code: &str) {
        self.invites.lock().unwrap().remove(code);
        self.save_invites();
    }

    // ---- ban / activity ----

    pub fn set_disabled(&self, username: &str, disabled: bool) -> Result<()> {
        {
            let mut users = self.users.lock().unwrap();
            let u = users
                .get_mut(username)
                .ok_or_else(|| anyhow!("no such user"))?;
            if u.role == Role::Admin && disabled {
                return Err(anyhow!("cannot disable an admin account"));
            }
            u.disabled = disabled;
        }
        if disabled {
            // Drop the banned user's sessions immediately.
            self.sessions
                .lock()
                .unwrap()
                .retain(|_, s| s.username != username);
        }
        self.save()
    }

    pub fn is_disabled(&self, username: &str) -> bool {
        self.users
            .lock()
            .unwrap()
            .get(username)
            .map(|u| u.disabled)
            .unwrap_or(false)
    }

    pub fn touch(&self, username: &str, ip: Option<String>) {
        let mut usage = self.usage.lock().unwrap();
        let e = usage.entry(username.to_string()).or_default();
        e.last_seen = now_unix();
        if ip.is_some() {
            e.last_ip = ip;
        }
    }

    /// Add to a user's cumulative bytes-served counter.
    pub fn add_bytes(&self, username: &str, n: u64) {
        self.usage
            .lock()
            .unwrap()
            .entry(username.to_string())
            .or_default()
            .bytes += n;
    }

    /// Persist the usage map (cumulative bandwidth) to disk.
    pub fn save_usage(&self) {
        if let Ok(bytes) = serde_json::to_vec_pretty(&*self.usage.lock().unwrap()) {
            let _ = write_private(&self.usage_path, &bytes);
        }
    }

    // ---- account / API token ----

    /// Public account info for the given user (includes their API token).
    pub fn account(&self, username: &str) -> Option<serde_json::Value> {
        let users = self.users.lock().unwrap();
        let u = users.get(username)?;
        Some(json!({
            "username": u.username,
            "role": u.role,
            "created": u.created,
            "api_token": u.api_token,
            "twofa": u.totp_secret.is_some(),
            "recovery_left": u.recovery_hashes.len(),
            "sessions": self.session_count(username),
        }))
    }

    /// Issue (or replace) the user's API token and return it.
    pub fn regenerate_token(&self, username: &str) -> Result<String> {
        let token = format!("mb_{}", gen_token());
        {
            let mut users = self.users.lock().unwrap();
            let u = users
                .get_mut(username)
                .ok_or_else(|| anyhow!("no such user"))?;
            u.api_token = Some(token.clone());
        }
        self.save()?;
        Ok(token)
    }

    /// Resolve a bearer API token to an identity. Uses a constant-time compare
    /// so the match can't be reconstructed byte-by-byte via response timing.
    pub fn user_by_token(&self, token: &str) -> Option<Identity> {
        if token.is_empty() {
            return None;
        }
        self.users
            .lock()
            .unwrap()
            .values()
            .find(|u| {
                u.api_token
                    .as_deref()
                    .is_some_and(|t| ct_eq(t.as_bytes(), token.as_bytes()))
            })
            .map(|u| Identity {
                username: u.username.clone(),
                role: u.role,
            })
    }

    fn session_count(&self, username: &str) -> usize {
        self.sessions
            .lock()
            .unwrap()
            .values()
            .filter(|s| s.username == username)
            .count()
    }

    /// Drop all of this user's sessions except the current one.
    pub fn logout_others(&self, username: &str, keep_token: &str) {
        self.sessions
            .lock()
            .unwrap()
            .retain(|tok, s| s.username != username || tok == keep_token);
    }

    // ---- admin: sessions, stats, audit log ----

    /// (user count, active session count).
    pub fn stats(&self) -> (usize, usize) {
        (
            self.users.lock().unwrap().len(),
            self.sessions.lock().unwrap().len(),
        )
    }

    /// All active sessions, newest first; `current_token` marks the caller's.
    pub fn list_sessions(&self, current_token: &str) -> Vec<serde_json::Value> {
        let mut list: Vec<(SystemTime, serde_json::Value)> = self
            .sessions
            .lock()
            .unwrap()
            .iter()
            .map(|(tok, s)| {
                (
                    s.created,
                    json!({
                        "id": s.id,
                        "user": s.username,
                        "role": s.role,
                        "created": to_unix(s.created),
                        "expires": to_unix(s.expires),
                        "current": tok == current_token,
                    }),
                )
            })
            .collect();
        list.sort_by(|a, b| b.0.cmp(&a.0));
        list.into_iter().map(|(_, v)| v).collect()
    }

    pub fn revoke_session(&self, sid: &str) {
        self.sessions.lock().unwrap().retain(|_, s| s.id != sid);
    }

    /// Drop every session except the caller's (force everyone else to re-login).
    pub fn revoke_all_except(&self, keep_token: &str) {
        self.sessions
            .lock()
            .unwrap()
            .retain(|tok, _| tok == keep_token);
    }

    pub fn log_event(&self, user: &str, action: &str) {
        let mut ev = self.events.lock().unwrap();
        ev.push_back(Event {
            time: now_unix(),
            user: user.to_string(),
            action: action.to_string(),
        });
        while ev.len() > MAX_EVENTS {
            ev.pop_front();
        }
    }

    /// Recent audit-log entries, newest first.
    pub fn recent_events(&self) -> Vec<Event> {
        self.events.lock().unwrap().iter().rev().cloned().collect()
    }

    pub fn delete_user(&self, username: &str) -> Result<()> {
        let mut users = self.users.lock().unwrap();
        let is_last_admin = users.get(username).map(|u| u.role) == Some(Role::Admin)
            && users.values().filter(|u| u.role == Role::Admin).count() == 1;
        if is_last_admin {
            return Err(anyhow!("cannot delete the last admin"));
        }
        if users.remove(username).is_none() {
            return Err(anyhow!("no such user"));
        }
        drop(users);
        self.save()
    }

    pub fn set_password(&self, username: &str, password: &str) -> Result<()> {
        if password.len() < MIN_PASSWORD_LEN {
            return Err(anyhow!(
                "password must be at least {MIN_PASSWORD_LEN} characters"
            ));
        }
        let hash = hash_password(password)?;
        let mut users = self.users.lock().unwrap();
        users
            .get_mut(username)
            .ok_or_else(|| anyhow!("no such user"))?
            .password_hash = hash;
        drop(users);
        self.save()
    }

    pub fn change_own_password(&self, username: &str, old: &str, new: &str) -> Result<()> {
        let ok = self
            .users
            .lock()
            .unwrap()
            .get(username)
            .map(|u| verify_password(old, &u.password_hash))
            .unwrap_or(false);
        if !ok {
            return Err(anyhow!("current password is incorrect"));
        }
        self.set_password(username, new)
    }

    // ---- login / sessions ----

    /// Verify credentials. Returns a session token, or a 2FA challenge if the
    /// account has two-factor enabled.
    pub fn login(&self, username: &str, password: &str) -> Result<LoginOutcome> {
        if self.is_throttled(username) {
            return Err(anyhow!("too many attempts; wait a few minutes"));
        }

        let found = self.users.lock().unwrap().get(username).map(|u| {
            (
                u.role,
                u.disabled,
                verify_password(password, &u.password_hash),
            )
        });

        match found {
            None => {
                // Spend ~the same time as a real verify so response timing
                // doesn't reveal whether the username exists.
                let _ = verify_password(password, dummy_hash());
                self.record_failure(username);
                Err(anyhow!("invalid username or password"))
            }
            Some((_, true, _)) => Err(anyhow!("this account has been disabled")),
            Some((role, false, true)) => {
                self.throttle.lock().unwrap().remove(username);
                if self.has_2fa(username) {
                    let challenge = gen_token();
                    let now = SystemTime::now();
                    let mut ch = self.challenges.lock().unwrap();
                    ch.retain(|_, v| now < v.1); // drop expired challenges
                    ch.insert(
                        challenge.clone(),
                        (username.to_string(), now + Duration::from_secs(300), 0),
                    );
                    drop(ch);
                    Ok(LoginOutcome::TwoFactor(challenge))
                } else {
                    Ok(LoginOutcome::Session(self.new_session(username, role)))
                }
            }
            Some((_, false, false)) => {
                self.record_failure(username);
                Err(anyhow!("invalid username or password"))
            }
        }
    }

    fn new_session(&self, username: &str, role: Role) -> String {
        let token = gen_token();
        let now = SystemTime::now();
        self.sessions.lock().unwrap().insert(
            token.clone(),
            SessionData {
                id: gen_token()[..12].to_string(),
                username: username.to_string(),
                role,
                created: now,
                expires: now + SESSION_TTL,
            },
        );
        token
    }

    // ---- two-factor (TOTP) ----

    pub fn has_2fa(&self, username: &str) -> bool {
        self.users
            .lock()
            .unwrap()
            .get(username)
            .map(|u| u.totp_secret.is_some())
            .unwrap_or(false)
    }

    /// Complete the 2FA login step: verify a TOTP or recovery code for the
    /// challenge and issue a session.
    pub fn verify_2fa(&self, challenge: &str, code: &str) -> Result<String> {
        let username = {
            let mut ch = self.challenges.lock().unwrap();
            let entry = ch
                .get_mut(challenge)
                .ok_or_else(|| anyhow!("invalid or expired challenge — log in again"))?;
            if SystemTime::now() > entry.1 {
                ch.remove(challenge);
                return Err(anyhow!("challenge expired — log in again"));
            }
            entry.2 += 1;
            if entry.2 > 6 {
                ch.remove(challenge);
                return Err(anyhow!("too many attempts — log in again"));
            }
            entry.0.clone()
        };
        let (secret, role, disabled) = {
            let users = self.users.lock().unwrap();
            let u = users
                .get(&username)
                .ok_or_else(|| anyhow!("no such user"))?;
            (u.totp_secret.clone(), u.role, u.disabled)
        };
        if disabled {
            return Err(anyhow!("this account has been disabled"));
        }
        if self.is_throttled(&username) {
            return Err(anyhow!("too many attempts — wait a few minutes"));
        }
        let secret = secret.ok_or_else(|| anyhow!("2FA is not enabled"))?;
        let code = code.trim();
        if check_totp(&secret, &username, code) || self.consume_recovery(&username, code) {
            self.throttle.lock().unwrap().remove(&username);
            self.challenges.lock().unwrap().remove(challenge);
            Ok(self.new_session(&username, role))
        } else {
            // Count 2FA failures toward the per-username throttle so a stolen
            // password can't be used to brute-force the code.
            self.record_failure(&username);
            Err(anyhow!("invalid code"))
        }
    }

    /// Begin 2FA enrollment: returns (base32 secret, otpauth URL, QR SVG).
    pub fn start_enroll_2fa(&self, username: &str) -> Result<(String, String, String)> {
        if self.has_2fa(username) {
            return Err(anyhow!("2FA is already enabled"));
        }
        let secret = gen_totp_secret();
        let totp = make_totp(&secret, username)?;
        let url = totp.get_url();
        let qr = qr_svg(&url).ok_or_else(|| anyhow!("could not render QR code"))?;
        self.pending_2fa.lock().unwrap().insert(
            username.to_string(),
            (secret.clone(), SystemTime::now() + Duration::from_secs(600)),
        );
        Ok((secret, url, qr))
    }

    /// Confirm enrollment with a code; on success returns one-time recovery codes.
    pub fn confirm_2fa(&self, username: &str, code: &str) -> Result<Vec<String>> {
        let secret = {
            let mut p = self.pending_2fa.lock().unwrap();
            let (s, exp) = p
                .get(username)
                .cloned()
                .ok_or_else(|| anyhow!("start 2FA setup first"))?;
            if SystemTime::now() > exp {
                p.remove(username);
                return Err(anyhow!("setup expired — start again"));
            }
            s
        };
        if !check_totp(&secret, username, code.trim()) {
            return Err(anyhow!(
                "incorrect code — check your authenticator app's time"
            ));
        }
        let codes: Vec<String> = (0..8).map(|_| gen_recovery_code()).collect();
        let hashes: Vec<String> = codes.iter().map(|c| sha256_hex(c)).collect();
        {
            let mut users = self.users.lock().unwrap();
            let u = users
                .get_mut(username)
                .ok_or_else(|| anyhow!("no such user"))?;
            u.totp_secret = Some(secret);
            u.recovery_hashes = hashes;
        }
        self.pending_2fa.lock().unwrap().remove(username);
        self.save()?;
        Ok(codes)
    }

    /// Disable 2FA (requires the account password).
    pub fn disable_2fa(&self, username: &str, password: &str) -> Result<()> {
        let ok = self
            .users
            .lock()
            .unwrap()
            .get(username)
            .map(|u| verify_password(password, &u.password_hash))
            .unwrap_or(false);
        if !ok {
            return Err(anyhow!("password is incorrect"));
        }
        {
            let mut users = self.users.lock().unwrap();
            if let Some(u) = users.get_mut(username) {
                u.totp_secret = None;
                u.recovery_hashes.clear();
            }
        }
        self.pending_2fa.lock().unwrap().remove(username);
        self.save()
    }

    fn consume_recovery(&self, username: &str, code: &str) -> bool {
        let h = sha256_hex(code);
        let removed = {
            let mut users = self.users.lock().unwrap();
            match users.get_mut(username) {
                Some(u) => u
                    .recovery_hashes
                    .iter()
                    .position(|x| *x == h)
                    .map(|p| {
                        u.recovery_hashes.remove(p);
                    })
                    .is_some(),
                None => false,
            }
        };
        if removed {
            self.save().ok();
        }
        removed
    }

    fn is_throttled(&self, username: &str) -> bool {
        let t = self.throttle.lock().unwrap();
        match t.get(username) {
            Some((fails, since)) => {
                since.elapsed().unwrap_or(FAIL_WINDOW) < FAIL_WINDOW && *fails >= MAX_FAILS
            }
            None => false,
        }
    }

    fn record_failure(&self, username: &str) {
        let mut t = self.throttle.lock().unwrap();
        let entry = t
            .entry(username.to_string())
            .or_insert((0, SystemTime::now()));
        if entry.1.elapsed().unwrap_or_default() > FAIL_WINDOW {
            *entry = (0, SystemTime::now());
        }
        entry.0 += 1;
    }

    pub fn session(&self, token: &str) -> Option<Identity> {
        let mut s = self.sessions.lock().unwrap();
        match s.get(token) {
            Some(sd) if SystemTime::now() < sd.expires => Some(Identity {
                username: sd.username.clone(),
                role: sd.role,
            }),
            Some(_) => {
                s.remove(token);
                None
            }
            None => None,
        }
    }

    pub fn logout(&self, token: &str) {
        self.sessions.lock().unwrap().remove(token);
    }

    pub fn identity_from_headers(&self, headers: &HeaderMap) -> Option<Identity> {
        token_from_headers(headers).and_then(|t| self.session(&t))
    }

    pub fn make_cookie(&self, token: &str) -> String {
        let secure = if self.secure_cookie { "; Secure" } else { "" };
        format!(
            "{COOKIE}={token}; HttpOnly; SameSite=Strict; Path=/; Max-Age={}{secure}",
            SESSION_TTL.as_secs()
        )
    }

    pub fn clear_cookie(&self) -> String {
        let secure = if self.secure_cookie { "; Secure" } else { "" };
        format!("{COOKIE}=deleted; HttpOnly; SameSite=Strict; Path=/; Max-Age=0{secure}")
    }
}

/// Read an `Authorization: Bearer <token>` header.
fn bearer(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .map(|s| s.trim().to_string())
}

/// Read the password from an `Authorization: Basic <base64(user:pass)>` header.
///
/// This lets any download manager / media player (JDownloader, IDM, VLC, curl)
/// authenticate with a plain URL of the form `https://user:API_TOKEN@host/...`.
/// The username is ignored; the password must be a valid API token.
fn basic_password(headers: &HeaderMap) -> Option<String> {
    let b64 = headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Basic ")?
        .trim();
    let text = String::from_utf8(b64_decode(b64)?).ok()?;
    // "user:token" — the token is everything after the first ':'.
    text.split_once(':').map(|(_, token)| token.to_string())
}

/// Minimal standard-alphabet base64 decoder (avoids pulling in a dependency).
/// Tolerates missing padding; returns `None` on any non-alphabet byte.
fn b64_decode(s: &str) -> Option<Vec<u8>> {
    fn val(c: u8) -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => (c - b'A') as u32,
            b'a'..=b'z' => (c - b'a' + 26) as u32,
            b'0'..=b'9' => (c - b'0' + 52) as u32,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        })
    }
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut buf, mut bits) = (0u32, 0u32);
    for &b in s.as_bytes().iter().filter(|&&b| b != b'=') {
        buf = (buf << 6) | val(b)?;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
        }
    }
    Some(out)
}

/// Constant-time byte-slice equality — no early return on first mismatch, so a
/// secret can't be reconstructed byte-by-byte from timing. Differing lengths
/// short-circuit (token lengths are fixed and not themselves secret).
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Read the session token from the `Cookie` header.
pub fn token_from_headers(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(header::COOKIE)?.to_str().ok()?;
    let prefix = format!("{COOKIE}=");
    raw.split(';')
        .map(str::trim)
        .find_map(|part| part.strip_prefix(&prefix))
        .map(str::to_string)
}

fn hash_password(pw: &str) -> Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    Ok(Argon2::default()
        .hash_password(pw.as_bytes(), &salt)
        .map_err(|e| anyhow!("password hashing failed: {e}"))?
        .to_string())
}

/// A real argon2 hash, computed once, used to equalize verify timing for
/// unknown usernames (mitigates timing-based username enumeration).
fn dummy_hash() -> &'static str {
    static H: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    H.get_or_init(|| hash_password("magnetbox-timing-equalizer").unwrap_or_default())
}

fn verify_password(pw: &str, hash: &str) -> bool {
    match PasswordHash::new(hash) {
        Ok(parsed) => Argon2::default()
            .verify_password(pw.as_bytes(), &parsed)
            .is_ok(),
        Err(_) => false,
    }
}

fn gen_token() -> String {
    let mut b = [0u8; 32];
    OsRng.fill_bytes(&mut b);
    b.iter().map(|x| format!("{x:02x}")).collect()
}

// ---- TOTP helpers ----

fn make_totp(secret_b32: &str, account: &str) -> Result<TOTP> {
    let bytes = Secret::Encoded(secret_b32.to_string())
        .to_bytes()
        .map_err(|_| anyhow!("invalid TOTP secret"))?;
    TOTP::new(
        Algorithm::SHA1,
        6,
        1,
        30,
        bytes,
        Some("MagnetBox".to_string()),
        account.to_string(),
    )
    .map_err(|e| anyhow!("totp init failed: {e}"))
}

fn check_totp(secret_b32: &str, account: &str, code: &str) -> bool {
    match make_totp(secret_b32, account) {
        Ok(totp) => totp.check_current(code).unwrap_or(false),
        Err(_) => false,
    }
}

fn gen_totp_secret() -> String {
    let mut b = [0u8; 20];
    OsRng.fill_bytes(&mut b);
    match Secret::Raw(b.to_vec()).to_encoded() {
        Secret::Encoded(s) => s,
        _ => String::new(),
    }
}

fn gen_recovery_code() -> String {
    // 64 bits of entropy so the stored SHA-256 hash isn't offline-brute-forceable.
    let mut b = [0u8; 8];
    OsRng.fill_bytes(&mut b);
    let hex: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!(
        "{}-{}-{}-{}",
        &hex[0..4],
        &hex[4..8],
        &hex[8..12],
        &hex[12..16]
    )
}

fn sha256_hex(s: &str) -> String {
    Sha256::digest(s.as_bytes())
        .iter()
        .map(|x| format!("{x:02x}"))
        .collect()
}

fn qr_svg(data: &str) -> Option<String> {
    use qrcode::render::svg;
    let code = qrcode::QrCode::new(data.as_bytes()).ok()?;
    Some(
        code.render::<svg::Color>()
            .min_dimensions(220, 220)
            .quiet_zone(true)
            .build(),
    )
}

/// Write a file containing secrets, restricting it to owner-only (0600) on Unix.
fn write_private(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    std::fs::write(path, bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

fn now_unix() -> u64 {
    to_unix(SystemTime::now())
}

fn to_unix(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}

/// Gate every protected request: 401 API calls / redirect browsers when there's
/// no valid session, enforce admin-only paths, and inject [`Identity`].
pub async fn require_auth(State(auth): State<Auth>, req: Request, next: Next) -> Response {
    let path = req.uri().path().to_string();

    // API clients authenticate with `Authorization: Bearer <token>`; download
    // managers / players use HTTP Basic (`user:API_TOKEN@host`); browsers use
    // the session cookie.
    let via_token = bearer(req.headers())
        .or_else(|| basic_password(req.headers()))
        .and_then(|t| auth.user_by_token(&t));

    // CSRF only matters for ambient cookie auth — token auth isn't forgeable
    // cross-site, so skip the Origin check for it.
    if via_token.is_none() && matches!(req.method().as_str(), "POST" | "PUT" | "DELETE" | "PATCH") {
        if let Some(reason) = cross_origin(req.headers()) {
            return (
                StatusCode::FORBIDDEN,
                Json(json!({ "ok": false, "error": reason })),
            )
                .into_response();
        }
    }

    let identity = via_token.or_else(|| auth.identity_from_headers(req.headers()));
    let Some(ident) = identity else {
        // Send first-run visitors to the setup wizard, everyone else to login.
        let dest = if auth.needs_setup() {
            "/setup"
        } else {
            "/login"
        };
        return if path.starts_with("/api/") {
            (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "ok": false, "error": "login required" })),
            )
                .into_response()
        } else {
            Redirect::to(dest).into_response()
        };
    };

    // Banned accounts are rejected even if they still hold a cookie/token.
    if auth.is_disabled(&ident.username) {
        return if path.starts_with("/api/") {
            (
                StatusCode::FORBIDDEN,
                Json(json!({ "ok": false, "error": "account disabled" })),
            )
                .into_response()
        } else {
            Redirect::to("/login").into_response()
        };
    }

    // Maintenance mode: only admins get through.
    if auth.maintenance() && ident.role != Role::Admin {
        return if path.starts_with("/api/") {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({ "ok": false, "error": "MagnetBox is in maintenance mode" })),
            )
                .into_response()
        } else {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "MagnetBox is down for maintenance — please check back soon.",
            )
                .into_response()
        };
    }

    let admin_area = path == "/admin"
        || path.starts_with("/api/users")
        || path.starts_with("/api/settings")
        || path.starts_with("/api/admin");
    if admin_area && ident.role != Role::Admin {
        return if path.starts_with("/api/") {
            (
                StatusCode::FORBIDDEN,
                Json(json!({ "ok": false, "error": "admin only" })),
            )
                .into_response()
        } else {
            Redirect::to("/").into_response()
        };
    }

    // Record activity (in-memory) and audit-log state changes.
    auth.touch(&ident.username, client_ip(req.headers()));
    if matches!(req.method().as_str(), "POST" | "PUT" | "DELETE" | "PATCH") {
        auth.log_event(&ident.username, &format!("{} {}", req.method(), path));
    }

    let username = ident.username.clone();
    let is_file =
        path.starts_with("/download/") || path.starts_with("/stream/") || path.starts_with("/dl/");

    let mut req = req;
    req.extensions_mut().insert(ident);
    let resp = next.run(req).await;

    // Attribute served bytes (file responses) to the user for bandwidth stats.
    if is_file {
        if let Some(len) = resp
            .headers()
            .get(header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok())
        {
            auth.add_bytes(&username, len);
        }
    }
    resp
}

/// Best-effort client IP from proxy headers (set by Caddy/nginx in production).
fn client_ip(headers: &HeaderMap) -> Option<String> {
    if let Some(v) = headers.get("x-forwarded-for").and_then(|v| v.to_str().ok()) {
        if let Some(first) = v
            .split(',')
            .next()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
        {
            return Some(first.to_string());
        }
    }
    headers
        .get("x-real-ip")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim().to_string())
}

/// `Some(reason)` if a mutating request's `Origin` doesn't match its `Host`.
fn cross_origin(headers: &HeaderMap) -> Option<String> {
    let host = headers.get(header::HOST)?.to_str().ok()?;
    let origin = headers.get(header::ORIGIN)?.to_str().ok()?;
    let origin_host = origin.split("://").nth(1).unwrap_or(origin);
    if origin_host != host {
        return Some("cross-origin request blocked".into());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn b64_decodes_standard_and_unpadded() {
        // "user:mb_secret" — the form a download manager sends for user:pass@host.
        assert_eq!(
            b64_decode("dXNlcjptYl9zZWNyZXQ=").unwrap(),
            b"user:mb_secret"
        );
        // Same payload without the '=' padding still decodes.
        assert_eq!(
            b64_decode("dXNlcjptYl9zZWNyZXQ").unwrap(),
            b"user:mb_secret"
        );
        assert_eq!(b64_decode("").unwrap(), b"");
    }

    #[test]
    fn b64_rejects_invalid_chars() {
        assert!(b64_decode("not valid base64!").is_none());
    }

    #[test]
    fn basic_password_extracts_token_after_first_colon() {
        let mut h = HeaderMap::new();
        // base64("alice:mb_tok:with:colons") — password keeps the trailing colons.
        h.insert(
            header::AUTHORIZATION,
            "Basic YWxpY2U6bWJfdG9rOndpdGg6Y29sb25z".parse().unwrap(),
        );
        assert_eq!(basic_password(&h).as_deref(), Some("mb_tok:with:colons"));
    }

    #[test]
    fn basic_password_none_without_basic_header() {
        let h = HeaderMap::new();
        assert!(basic_password(&h).is_none());
    }

    #[test]
    fn ct_eq_matches_only_identical_bytes() {
        assert!(ct_eq(b"mb_secrettoken", b"mb_secrettoken"));
        assert!(!ct_eq(b"mb_secrettoken", b"mb_secrettokem")); // one byte differs
        assert!(!ct_eq(b"short", b"longer-value")); // different lengths
        assert!(ct_eq(b"", b"")); // both empty
    }
}
