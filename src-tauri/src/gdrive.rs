//! Google Drive backup and sync, straight between this PC and the user's own Google Drive (no server in between).
//!
//! Sign-in: OAuth 2.0 for desktop apps. The system browser opens Google's page; Google sends the answer back to a
//! one-time listener on 127.0.0.1 (loopback). PKCE (S256) and a random `state` protect the exchange. The refresh token
//! is kept in the Windows Credential Manager, never in a file, a log or an error message.
//!
//! Permission: only `drive.appdata`, Drive's hidden app folder. ChatVault cannot see any other file in the Drive, and
//! the user cannot see this folder in Drive either; they can remove it from Drive's settings (Manage apps).
//!
//! What is stored: the files of the local backup folder (see sync.rs), compressed with zstd. Each PC writes its own
//! copies, marked with the PC's id, so two PCs never overwrite each other's files in Drive. Every PC then downloads
//! what the others wrote. Locally a file is only replaced when that cannot lose anything: the local copy was already in
//! Drive, or the newer copy is the same chat with more messages (chat files only grow). Otherwise the other PC's copy
//! is kept next to it as `<name>.from-<PC>.<ext>`. Nothing is ever deleted by a sync.

use crate::paths::*;
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter};

const AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const REVOKE_URL: &str = "https://oauth2.googleapis.com/revoke";
const FILES_URL: &str = "https://www.googleapis.com/drive/v3/files";
const UPLOAD_URL: &str = "https://www.googleapis.com/upload/drive/v3/files";
const ABOUT_URL: &str = "https://www.googleapis.com/drive/v3/about";
/// The only permission ChatVault asks for.
pub const SCOPE: &str = "https://www.googleapis.com/auth/drive.appdata";
const KEYRING_SERVICE: &str = "ChatVault Google Drive";
const KEYRING_USER: &str = "refresh-token";
/// Up to this size a file goes up in one request; larger ones use a resumable upload in chunks.
const SIMPLE_LIMIT: usize = 5 * 1024 * 1024;
/// Resumable chunks must be a multiple of 256 KB.
const CHUNK: usize = 32 * 256 * 1024;
/// How long the browser sign-in may take before it gives up.
const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(300);
/// Top-level folders of the backup that may be written from Drive. Anything else in Drive is ignored.
const ALLOWED_ROOTS: [&str; 6] = ["claude", "codex", "cursor", "gemini", "cline", "opencode"];

static BUSY: AtomicBool = AtomicBool::new(false);
type Unit = (String, String);
static ACTIVE_UNITS: Mutex<Vec<HashSet<Unit>>> = Mutex::new(Vec::new());

struct DriveGuard(Option<HashSet<Unit>>);

impl Drop for DriveGuard {
    fn drop(&mut self) {
        let mut active = ACTIVE_UNITS.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(units) = &self.0 {
            active.retain(|job| job != units);
        } else {
            CANCEL_SYNC.store(false, Ordering::SeqCst);
            BUSY.store(false, Ordering::SeqCst);
        }
    }
}

fn drive_guard(units: Option<&HashSet<Unit>>) -> Result<DriveGuard, String> {
    let mut active = ACTIVE_UNITS.lock().unwrap_or_else(|e| e.into_inner());
    if BUSY.load(Ordering::SeqCst)
        || units.map_or(!active.is_empty(), |units| active.len() >= 2 || active.iter().any(|job| !job.is_disjoint(units)))
    {
        return Err("A backup job is using these files. Try again when it finishes.".into());
    }
    if let Some(units) = units {
        active.push(units.clone());
    } else {
        BUSY.store(true, Ordering::SeqCst);
    }
    Ok(DriveGuard(units.cloned()))
}

fn drive_busy() -> bool {
    let active = ACTIVE_UNITS.lock().unwrap_or_else(|e| e.into_inner());
    BUSY.load(Ordering::SeqCst) || !active.is_empty()
}

pub(crate) fn backup_scope() -> (String, bool) {
    let state = load();
    let connected = client().is_some() && state.connected && refresh_token().is_some();
    let folder = backup_root();
    let key = json!([folder, if connected { state.email } else { None }]).to_string();
    (key, connected)
}

pub(crate) fn validate_scope(expected: &str) -> Result<(), String> {
    let (key, connected) = backup_scope();
    if !connected || key != expected {
        return Err("Reconnect the original Google Drive account and backup folder before resuming.".into());
    }
    Ok(())
}

static CANCEL_SIGN_IN: AtomicBool = AtomicBool::new(false);
static TOKEN: Mutex<Option<(String, Instant)>> = Mutex::new(None);
const LIST_TTL: Duration = Duration::from_secs(120);
const METADATA_TIMEOUT: Duration = Duration::from_secs(20);
static LIST_LOCK: Mutex<()> = Mutex::new(());
static LIST_CACHE: Mutex<Option<(Instant, Vec<Value>)>> = Mutex::new(None);
static STATE_LOCK: Mutex<()> = Mutex::new(());
/// Set when the user cancels a running sync.
static CANCEL_SYNC: AtomicBool = AtomicBool::new(false);
/// How many files the running sync has uploaded so far.
static RUN_DONE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

// ---------------------------------------------------------------- credentials of the app

/// The OAuth client built into this copy of ChatVault (official releases get it from GitHub Actions secrets; builds
/// from source from `.env`, see build.rs). None means this build has no Google Drive.
fn client() -> Option<(&'static str, Option<&'static str>)> {
    let id = option_env!("CHATVAULT_GOOGLE_CLIENT_ID").filter(|s| !s.is_empty())?;
    Some((id, option_env!("CHATVAULT_GOOGLE_CLIENT_SECRET").filter(|s| !s.is_empty())))
}

// ---------------------------------------------------------------- local state

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(default)]
struct State {
    device_id: String,
    email: Option<String>,
    name: Option<String>,
    connected: bool,
    auto_sync: Option<bool>,
    last_sync_ms: Option<i64>,
    last_error: Option<String>,
    last_result: Option<Stats>,
    /// Backup files as they were the last time they matched Drive: path → (size, modified ms).
    synced: HashMap<String, (i64, i64)>,
    /// The copy from Drive last applied to each path (file id + its version), so it is not looked at again.
    applied: HashMap<String, String>,
    /// The PCs that sync with this Google account, as last seen.
    devices: Vec<Device>,
    /// Files whose local copy was deleted on purpose: the Drive version that was there, not downloaded again until
    /// it changes.
    skip_download: HashMap<String, String>,
    /// Large uploads in progress: Google's upload address for each, so a sync that was interrupted carries on from the
    /// byte Google already has instead of starting the file again.
    uploads: HashMap<String, SavedUpload>,
}

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(default)]
struct SavedUpload {
    /// Size and time of the local file, and the size of what is being sent: the saved upload is only used while all
    /// three are the same.
    size: i64,
    mtime: i64,
    total: usize,
    session: String,
    started_ms: i64,
}

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Device {
    pub id: String,
    pub name: String,
    pub last_sync_ms: Option<i64>,
    pub files: usize,
    pub this_pc: bool,
}

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Stats {
    pub backed_up: usize,
    pub uploaded: usize,
    pub downloaded: usize,
    pub conflicts: usize,
    pub failed: usize,
    pub unchanged: usize,
    pub bytes_up: u64,
    pub bytes_down: u64,
    pub restored: usize,
    /// The sync was cancelled by the user (what it had uploaded is kept; the next sync goes on from there).
    pub cancelled: bool,
}

fn state_path() -> Option<PathBuf> {
    data_dir().map(|d| d.join("gdrive.json"))
}

fn load() -> State {
    let mut s: State = state_path().and_then(|p| fs::read_to_string(p).ok()).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    if s.device_id.len() != 16 || !s.device_id.chars().all(|c| c.is_ascii_hexdigit()) {
        s.device_id = hex(&random::<8>());
    }
    s
}

fn save(s: &State) {
    let Some(p) = state_path() else { return };
    if let Some(d) = p.parent() {
        let _ = fs::create_dir_all(d);
    }
    if let Ok(t) = serde_json::to_string(s) {
        let tmp = p.with_extension("json.tmp");
        if fs::write(&tmp, t).is_ok() {
            let _ = fs::rename(&tmp, &p);
        }
    }
}

/// Changes the saved state under a lock, so a sync and the dialog never write over each other.
fn update(f: impl FnOnce(&mut State)) -> State {
    let _g = STATE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut s = load();
    f(&mut s);
    save(&s);
    s
}

pub(crate) fn this_pc_name() -> String {
    std::env::var("COMPUTERNAME").ok().filter(|n| !n.trim().is_empty()).unwrap_or_else(|| "This PC".into())
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

// ---------------------------------------------------------------- small helpers

fn random<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    getrandom::fill(&mut b).expect("the system random number generator is available");
    b
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn b64url(b: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b)
}

/// PKCE: a random verifier and its S256 challenge.
pub(crate) fn pkce() -> (String, String) {
    let verifier = b64url(&random::<32>());
    let challenge = b64url(&Sha256::digest(verifier.as_bytes()));
    (verifier, challenge)
}

fn http() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .user_agent(concat!("ChatVault/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(300))
        .build()
        .map_err(|_| "Could not start a secure connection.".to_string())
}

/// One shared connection pool for all Drive requests (cheap to clone).
fn hc() -> reqwest::blocking::Client {
    static C: std::sync::OnceLock<reqwest::blocking::Client> = std::sync::OnceLock::new();
    C.get_or_init(|| http().unwrap_or_else(|_| reqwest::blocking::Client::new())).clone()
}

/// A plain message for a failed request. Never includes a token, a URL or a server answer.
fn net_err(e: &reqwest::Error) -> String {
    if e.is_timeout() || e.is_connect() || e.is_request() {
        "Could not reach Google Drive. Check the internet connection and try again.".into()
    } else {
        "Google Drive did not answer as expected. Try again in a moment.".into()
    }
}

/// Sends a request, retrying on a dropped connection, "too many requests" and server errors (1, 2, 4, 8 s apart).
/// A 401 drops the cached access token and tries once more with a fresh one.
fn send(build: impl Fn(&str) -> reqwest::blocking::RequestBuilder) -> Result<reqwest::blocking::Response, String> {
    let mut wait = 1;
    let mut renewed = false;
    for attempt in 0..5 {
        let token = access_token()?;
        match build(&token).bearer_auth(&token).send() {
            Ok(r) if r.status().as_u16() == 401 && !renewed => {
                renewed = true;
                *TOKEN.lock().unwrap_or_else(|e| e.into_inner()) = None;
            }
            Ok(r) if (r.status().as_u16() == 429 || r.status().is_server_error()) && attempt < 4 => {
                std::thread::sleep(Duration::from_secs(wait));
                wait *= 2;
            }
            Ok(r) if r.status().is_success() || r.status().as_u16() == 308 => return Ok(r),
            Ok(r) => return Err(drive_err(r)),
            Err(e) if attempt < 4 && (e.is_timeout() || e.is_connect() || e.is_request()) => {
                std::thread::sleep(Duration::from_secs(wait));
                wait *= 2;
            }
            Err(e) => return Err(net_err(&e)),
        }
    }
    Err("Google Drive is busy right now. ChatVault will try again later.".into())
}

fn drive_err(r: reqwest::blocking::Response) -> String {
    let code = r.status().as_u16();
    let reason = r
        .json::<Value>()
        .ok()
        .and_then(|v| v["error"]["errors"][0]["reason"].as_str().or(v["error"]["status"].as_str()).map(String::from))
        .unwrap_or_default();
    match (code, reason.as_str()) {
        (403, "storageQuotaExceeded") | (403, "quotaExceeded") => "Your Google Drive is full. Free up space in Google Drive and try again.".into(),
        (403, "accessNotConfigured") => "The Google Drive API is not switched on for this copy of ChatVault.".into(),
        (403, _) => "Google Drive refused the request. Try connecting again.".into(),
        (404, _) => "A file in Google Drive was not found. It may have been removed; the next sync will catch up.".into(),
        _ => format!("Google Drive answered with an error ({code}). Try again in a moment."),
    }
}

// ---------------------------------------------------------------- tokens

fn keyring() -> Result<keyring::Entry, String> {
    keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER).map_err(|_| "The Windows Credential Manager is not available.".to_string())
}

fn refresh_token() -> Option<String> {
    keyring().ok()?.get_password().ok().filter(|t| !t.is_empty())
}

/// A valid access token: the cached one, or a new one from the refresh token in the Credential Manager.
fn access_token() -> Result<String, String> {
    if let Some((t, until)) = TOKEN.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        if Instant::now() < *until {
            return Ok(t.clone());
        }
    }
    let (id, secret) = client().ok_or("Google Drive is not available in this build of ChatVault.")?;
    let refresh = refresh_token().ok_or("Google Drive is not connected.")?;
    let mut form = vec![("client_id", id.to_string()), ("grant_type", "refresh_token".to_string()), ("refresh_token", refresh)];
    if let Some(s) = secret {
        form.push(("client_secret", s.to_string()));
    }
    let r = hc().post(TOKEN_URL).timeout(METADATA_TIMEOUT).form(&form).send().map_err(|e| net_err(&e))?;
    let status = r.status();
    let v: Value = r.json().unwrap_or(Value::Null);
    if !status.is_success() {
        if v["error"].as_str() == Some("invalid_grant") {
            // Access was removed in the Google account (or the sign-in expired): forget it here too.
            forget_sign_in();
            return Err("Google Drive access was removed or has expired. Connect Google Drive again.".into());
        }
        return Err("Could not renew the Google Drive sign-in. Try again in a moment.".into());
    }
    remember_access(&v)
}

fn remember_access(v: &Value) -> Result<String, String> {
    let token = v["access_token"].as_str().ok_or("Google did not return a sign-in.")?.to_string();
    let secs = v["expires_in"].as_u64().unwrap_or(3600).saturating_sub(60).max(30);
    *TOKEN.lock().unwrap_or_else(|e| e.into_inner()) = Some((token.clone(), Instant::now() + Duration::from_secs(secs)));
    Ok(token)
}

fn forget_sign_in() {
    invalidate_listing();
    *TOKEN.lock().unwrap_or_else(|e| e.into_inner()) = None;
    if let Ok(k) = keyring() {
        let _ = k.delete_credential();
    }
    update(|s| {
        s.connected = false;
        s.email = None;
        s.name = None;
        s.last_sync_ms = None;
        s.last_result = None;
        s.uploads.clear();
    });
}

// ---------------------------------------------------------------- sign-in

/// What a callback request to the loopback listener said.
#[derive(Debug, PartialEq)]
pub(crate) enum Callback {
    Code(String),
    Denied(String),
    /// Not the answer (a favicon request, a wrong state, a stray visit): ignore it and keep waiting.
    Other,
}

/// Reads the first line of an HTTP request ("GET /?code=...&state=... HTTP/1.1") and checks its `state`.
pub(crate) fn parse_callback(request_line: &str, expected_state: &str) -> Callback {
    let mut parts = request_line.split_whitespace();
    if parts.next() != Some("GET") {
        return Callback::Other;
    }
    let Some(target) = parts.next() else { return Callback::Other };
    let Ok(url) = url::Url::parse(&format!("http://127.0.0.1{target}")) else { return Callback::Other };
    if url.path() != "/" {
        return Callback::Other;
    }
    let q: HashMap<String, String> = url.query_pairs().into_owned().collect();
    if q.get("state").map(String::as_str) != Some(expected_state) {
        return Callback::Other;
    }
    if let Some(e) = q.get("error") {
        return Callback::Denied(e.clone());
    }
    match q.get("code") {
        Some(c) if !c.is_empty() => Callback::Code(c.clone()),
        _ => Callback::Other,
    }
}

fn reply(mut s: TcpStream, status: &str, title: &str, body: &str) {
    let logo = base64::engine::general_purpose::STANDARD.encode(include_bytes!("../icons/128x128.png"));
    let page = format!(
        r##"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>{title} | ChatVault</title>
  <style>
    :root {{ color-scheme: light; font: 16px/1.5 "Segoe UI", system-ui, sans-serif; }}
    body {{ box-sizing: border-box; display: grid; place-items: center; min-height: 100vh; margin: 0; padding: 24px; background: #f7f8fa; color: #18181b; }}
    main {{ box-sizing: border-box; width: min(100%, 29rem); padding: clamp(24px, 6vw, 40px); background: #fff; border: 1px solid #e4e4e7; border-radius: 20px; box-shadow: 0 12px 32px #18181b0a; }}
    .brand {{ display: flex; gap: 10px; align-items: center; font-size: .9375rem; font-weight: 600; }}
    .brand img {{ display: block; width: 36px; height: 36px; }}
    h1 {{ margin: 24px 0 8px; font-size: clamp(1.5rem, 5vw, 2rem); line-height: 1.18; letter-spacing: -.025em; }}
    p {{ margin: 0; color: #52525b; }}
  </style>
</head>
<body>
  <main>
    <div class="brand"><img src="data:image/png;base64,{logo}" width="36" height="36" alt="">ChatVault</div>
    <h1>{title}</h1>
    <p>{body}</p>
  </main>
</body>
</html>"##
    );
    let _ = write!(
        s,
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nConnection: close\r\n\r\n{page}",
        page.len()
    );
    let _ = s.flush();
}

fn read_request_line(s: &mut TcpStream) -> Option<String> {
    let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    while buf.len() < 16 * 1024 {
        let n = s.read(&mut chunk).ok()?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
    }
    let text = String::from_utf8_lossy(&buf);
    text.lines().next().map(String::from)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    /// This build has a Google OAuth client.
    pub available: bool,
    pub connected: bool,
    pub email: Option<String>,
    pub name: Option<String>,
    pub this_pc: String,
    pub auto_sync: bool,
    pub last_sync_ms: Option<i64>,
    pub last_error: Option<String>,
    pub last_result: Option<Stats>,
    pub busy: bool,
    /// Files the running sync has uploaded so far (kept in Drive if it is cancelled).
    pub run_uploaded: usize,
    /// The user has cancelled the running sync and it is stopping.
    pub cancelling: bool,
    pub devices: Vec<Device>,
}

pub fn status() -> Status {
    let s = load();
    let connected = s.connected && refresh_token().is_some();
    Status {
        available: client().is_some(),
        connected,
        email: if connected { s.email } else { None },
        name: if connected { s.name } else { None },
        this_pc: this_pc_name(),
        auto_sync: s.auto_sync.unwrap_or(true),
        last_sync_ms: s.last_sync_ms,
        last_error: s.last_error,
        last_result: s.last_result,
        busy: drive_busy(),
        run_uploaded: if BUSY.load(Ordering::SeqCst) { RUN_DONE.load(Ordering::SeqCst) } else { 0 },
        cancelling: cancel_requested(),
        devices: if connected { s.devices } else { Vec::new() },
    }
}

/// Asks Google whether the connection still works (the user may have removed ChatVault's access in their Google
/// account, or deleted its data). A removed access signs this PC out; being offline changes nothing. Then the status.
pub fn check() -> Status {
    let s = load();
    if client().is_some() && s.connected && refresh_token().is_some() && !BUSY.load(Ordering::SeqCst) {
        let ok = send(|_| hc().get(FILES_URL).query(&[("spaces", "appDataFolder"), ("pageSize", "1"), ("fields", "files(id)")]));
        if ok.is_ok() {
            update(|s| {
                if s.last_error.as_deref().is_some_and(|e| e.contains("removed") || e.contains("expired")) {
                    s.last_error = None;
                }
            });
        }
    }
    status()
}

pub fn set_auto(on: bool) {
    update(|s| s.auto_sync = Some(on));
}

/// Asks the running sync to stop. What it already uploaded stays in Drive (each file is complete), so the next sync
/// carries on from there instead of starting over; a large file cut off midway resumes from its saved upload session.
pub fn cancel_sync() {
    if BUSY.load(Ordering::SeqCst) {
        CANCEL_SYNC.store(true, Ordering::SeqCst);
    }
}

fn cancelled() -> bool {
    CANCEL_SYNC.load(Ordering::SeqCst)
}

/// True while a Drive job is running and the user has cancelled it (so the local backup it started can stop too).
pub(crate) fn cancel_requested() -> bool {
    BUSY.load(Ordering::SeqCst) && CANCEL_SYNC.load(Ordering::SeqCst)
}

fn begin_run() {
    CANCEL_SYNC.store(false, Ordering::SeqCst);
    RUN_DONE.store(0, Ordering::SeqCst);
}

pub fn cancel_sign_in() {
    CANCEL_SIGN_IN.store(true, Ordering::SeqCst);
}

/// Opens Google's sign-in page in the browser and waits for the answer on 127.0.0.1. Only `drive.appdata` is asked for.
pub fn connect(app: &AppHandle) -> Result<Status, String> {
    use tauri_plugin_opener::OpenerExt;
    let _guard = drive_guard(None)?;
    let (id, secret) = client().ok_or("Google Drive is not available in this build of ChatVault.")?;
    CANCEL_SIGN_IN.store(false, Ordering::SeqCst);
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|_| "Could not get ready for the Google sign-in on this PC.")?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let redirect = format!("http://127.0.0.1:{port}");
    let (verifier, challenge) = pkce();
    let state = b64url(&random::<24>());
    let mut url = url::Url::parse(AUTH_URL).map_err(|e| e.to_string())?;
    url.query_pairs_mut()
        .append_pair("client_id", id)
        .append_pair("redirect_uri", &redirect)
        .append_pair("response_type", "code")
        .append_pair("scope", SCOPE)
        .append_pair("code_challenge", &challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("state", &state)
        .append_pair("access_type", "offline")
        .append_pair("prompt", "consent");
    app.opener().open_url(url.as_str(), None::<&str>).map_err(|_| "Could not open the browser for the Google sign-in.")?;

    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    let started = Instant::now();
    let code = loop {
        if CANCEL_SIGN_IN.load(Ordering::SeqCst) {
            return Err("The Google sign-in was cancelled.".into());
        }
        if started.elapsed() > SIGN_IN_TIMEOUT {
            return Err("The Google sign-in took too long. Try again.".into());
        }
        match listener.accept() {
            Ok((mut stream, _)) => {
                let _ = stream.set_nonblocking(false);
                let line = read_request_line(&mut stream).unwrap_or_default();
                match parse_callback(&line, &state) {
                    Callback::Code(c) => {
                        reply(stream, "200 OK", "Google sign-in complete", "You can close this tab and return to ChatVault to finish connecting Google Drive.");
                        break c;
                    }
                    Callback::Denied(_) => {
                        reply(stream, "200 OK", "Google Drive was not connected", "You can close this tab. Nothing was shared with ChatVault.");
                        return Err("Google Drive was not connected: the permission was not given.".into());
                    }
                    Callback::Other => reply(stream, "404 Not Found", "Not found", "This address is only used while connecting ChatVault to Google Drive."),
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(150)),
            Err(_) => std::thread::sleep(Duration::from_millis(150)),
        }
    };
    drop(listener);

    let mut form = vec![
        ("client_id", id.to_string()),
        ("code", code),
        ("code_verifier", verifier),
        ("grant_type", "authorization_code".to_string()),
        ("redirect_uri", redirect),
    ];
    if let Some(s) = secret {
        form.push(("client_secret", s.to_string()));
    }
    let r = http()?.post(TOKEN_URL).form(&form).send().map_err(|e| net_err(&e))?;
    let ok = r.status().is_success();
    let v: Value = r.json().unwrap_or(Value::Null);
    if !ok {
        return Err("Google did not accept the sign-in. Try connecting again.".into());
    }
    // Google may let people untick a permission on its consent page; without it there is nothing to sync.
    if !v["scope"].as_str().unwrap_or(SCOPE).split(' ').any(|s| s == SCOPE) {
        return Err("ChatVault needs permission to keep its backup in your Google Drive. Connect again and leave that box ticked.".into());
    }
    let refresh = v["refresh_token"].as_str().ok_or("Google did not return a lasting sign-in. Try connecting again.")?;
    keyring()?.set_password(refresh).map_err(|_| "Could not save the sign-in in the Windows Credential Manager.")?;
    remember_access(&v)?;

    let about: Value = send(|_| hc().get(ABOUT_URL).query(&[("fields", "user(displayName,emailAddress)")]))?
        .json()
        .map_err(|_| "Google Drive did not answer as expected.")?;
    update(|s| {
        s.connected = true;
        s.email = about["user"]["emailAddress"].as_str().map(String::from);
        s.name = about["user"]["displayName"].as_str().map(String::from);
        s.last_error = None;
    });
    invalidate_listing();
    Ok(status())
}

/// Signs ChatVault out of Google Drive: the access is revoked at Google and the saved sign-in removed from this PC.
/// With `delete_backup`, every ChatVault file in the Drive app folder (from every PC) is deleted first.
pub fn disconnect(delete_backup: bool) -> Result<Status, String> {
    let _guard = drive_guard(None)?;
    if delete_backup {
        let files = list_remote()?;
        for f in files {
            send(|_| hc().delete(format!("{FILES_URL}/{}", f.id)))?;
        }
        update(|s| {
            s.synced.clear();
            s.applied.clear();
            s.devices.clear();
        });
    }
    if let Some(t) = refresh_token() {
        // Best effort: even if Google cannot be reached, the sign-in is removed from this PC below.
        if let Ok(c) = http() {
            let _ = c.post(REVOKE_URL).form(&[("token", t)]).send();
        }
    }
    forget_sign_in();
    update(|s| {
        s.last_error = None;
        s.devices.clear();
    });
    Ok(status())
}

// ---------------------------------------------------------------- Drive files

#[derive(Debug, Clone)]
pub(crate) struct Remote {
    pub(crate) id: String,
    /// Path inside the backup folder, with `/` between folders.
    pub(crate) rel: String,
    pub(crate) device: String,
    pub(crate) size: i64,
    pub(crate) mtime: i64,
    pub(crate) zipped: bool,
    /// Size of the file as stored in Drive (compressed).
    pub(crate) bytes: i64,
}

impl Remote {
    pub(crate) fn version(&self) -> String {
        format!("{}:{}:{}", self.id, self.size, self.mtime)
    }
}

fn invalidate_listing() {
    *LIST_CACHE.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

fn cached_listing() -> Option<Vec<Value>> {
    LIST_CACHE.lock().unwrap_or_else(|e| e.into_inner()).as_ref()
        .filter(|(checked, _)| checked.elapsed() < LIST_TTL)
        .map(|(_, all)| all.clone())
}

fn list_all() -> Result<Vec<Value>, String> {
    let _refresh = LIST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(all) = cached_listing() {
        return Ok(all);
    }
    let all = fetch_listing()?;
    *LIST_CACHE.lock().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), all.clone()));
    Ok(all)
}

pub(crate) fn refresh_listing() {
    let _refresh = LIST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    invalidate_listing();
}

// Metadata actions should fail promptly so the UI can report the failure and let the user retry.
fn metadata_request(allow_missing: bool, build: impl Fn() -> reqwest::blocking::RequestBuilder) -> Result<reqwest::blocking::Response, String> {
    for attempt in 0..2 {
        let token = access_token()?;
        let response = build().timeout(METADATA_TIMEOUT).bearer_auth(token).send().map_err(|e| net_err(&e))?;
        if response.status().as_u16() == 401 && attempt == 0 {
            *TOKEN.lock().unwrap_or_else(|e| e.into_inner()) = None;
        } else if response.status().is_success() || (allow_missing && response.status().as_u16() == 404) {
            return Ok(response);
        } else {
            return Err(drive_err(response));
        }
    }
    Err("Google Drive authorization expired. Try again.".into())
}

fn fetch_listing() -> Result<Vec<Value>, String> {
    let mut out = Vec::new();
    let mut page: Option<String> = None;
    loop {
        if cancelled() {
            return Err("cancelled".into());
        }
        let p = page.clone();
        let r: Value = metadata_request(false, move || {
            let mut q = vec![
                ("spaces", "appDataFolder".to_string()),
                ("pageSize", "1000".to_string()),
                ("fields", "nextPageToken,files(id,name,size,appProperties)".to_string()),
            ];
            if let Some(t) = &p {
                q.push(("pageToken", t.clone()));
            }
            hc().get(FILES_URL).query(&q)
        })?
        .json()
        .map_err(|_| "Google Drive did not answer as expected.")?;
        let files = r["files"].as_array().ok_or("Google Drive did not return its backup list.")?;
        out.extend(files.iter().cloned());
        match r["nextPageToken"].as_str() {
            Some(t) => page = Some(t.to_string()),
            None => break,
        }
    }
    Ok(out)
}

fn list_remote() -> Result<Vec<Remote>, String> {
    Ok(list_all()?.iter().filter_map(remote_of).collect())
}

fn remote_of(v: &Value) -> Option<Remote> {
    let p = &v["appProperties"];
    if p["cv"].as_str() != Some("1") {
        return None;
    }
    Some(Remote {
        id: v["id"].as_str()?.to_string(),
        rel: v["name"].as_str()?.to_string(),
        device: p["dev"].as_str()?.to_string(),
        size: p["sz"].as_str()?.parse().ok()?,
        mtime: p["mt"].as_str()?.parse().ok()?,
        zipped: p["z"].as_str() == Some("1"),
        bytes: v["size"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0),
    })
}

/// A path from Drive, made safe: only plain folder and file names under one of the tools' folders. Anything that
/// could point outside the backup folder (`..`, a drive letter, a backslash, an absolute path) is refused.
pub(crate) fn safe_rel(rel: &str) -> Option<PathBuf> {
    if rel.is_empty() || rel.len() > 400 || rel.starts_with('/') {
        return None;
    }
    let parts: Vec<&str> = rel.split('/').collect();
    if parts.len() < 2 || parts.len() > 12 || !ALLOWED_ROOTS.contains(&parts[0]) {
        return None;
    }
    let mut p = PathBuf::new();
    for part in parts {
        // Windows device names (CON, NUL, COM1, ...) are refused too, with or without an extension.
        let base = part.split('.').next().unwrap_or("").to_ascii_uppercase();
        let device = matches!(base.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$")
            || ((base.starts_with("COM") || base.starts_with("LPT")) && base.len() == 4 && base.as_bytes()[3].is_ascii_digit());
        let bad = part.is_empty()
            || part == "."
            || part == ".."
            || part.ends_with('.')
            || part.ends_with(' ')
            || device
            || part.chars().any(|c| c.is_control() || "<>:\"\\|?*".contains(c));
        if bad {
            return None;
        }
        p.push(part);
    }
    Some(p)
}

type LocalFile = (String, PathBuf, i64, i64);

pub(crate) fn visit_local_files(root: &Path, mut visit: impl FnMut(LocalFile)) {
    let mut dirs = vec![root.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        let Ok(entries) = fs::read_dir(dir) else { continue };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else { continue };
            let path = entry.path();
            if kind.is_dir() {
                dirs.push(path);
                continue;
            }
            let Ok(rel) = path.strip_prefix(root) else { continue };
            let rel = rel.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/");
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if !kind.is_file() || name.starts_with(".chatvault") || name.ends_with(".cvpart") || safe_rel(&rel).is_none() {
                continue;
            }
            let Ok(m) = entry.metadata() else { continue };
            let Some(mtime) = m.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()) else { continue };
            visit((rel, path, m.len() as i64, mtime.as_millis() as i64));
        }
    }
}

pub(crate) fn local_files(root: &Path) -> Vec<LocalFile> {
    let mut files = Vec::new();
    visit_local_files(root, |file| files.push(file));
    files
}

fn remote_versions(remote: &[Remote]) -> HashSet<(&str, i64, i64)> {
    remote.iter().map(|r| (r.rel.as_str(), r.size, r.mtime)).collect()
}

fn needs_upload(versions: &HashSet<(&str, i64, i64)>, rel: &str, size: i64, mtime: i64) -> bool {
    !versions.contains(&(rel, size, mtime))
}

fn upload_compressed(rel: &str) -> bool {
    let lower = rel.to_ascii_lowercase();
    ![".zst", ".png", ".jpg", ".jpeg", ".webp", ".gif", ".heic", ".pdf", ".zip", ".gz", ".mp3", ".mp4", ".mov", ".webm"]
        .iter().any(|extension| lower.ends_with(extension))
}

/// Uploads one file. Returns the bytes sent and, when the file was newly created in Drive (not an update of one that
/// was already there), its Drive id, so a cancelled sync can remove exactly what it created.
#[cfg(test)]
fn upload(existing: Option<&str>, rel: &str, device: &str, path: &Path, size: i64, mtime: i64) -> Result<(u64, Option<String>), String> {
    upload_cancellable(existing, rel, device, path, size, mtime, &cancelled)
}

fn upload_cancellable(
    existing: Option<&str>, rel: &str, device: &str, path: &Path, size: i64, mtime: i64,
    is_stopped: &(dyn Fn() -> bool + Sync),
) -> Result<(u64, Option<String>), String> {
    if is_stopped() {
        return Err("cancelled".into());
    }
    let raw = fs::read(path).map_err(|_| format!("Could not read {rel}"))?;
    // Already-compressed files (pictures, PDFs, archives, .zst backups) go up as they are; compressing them again
    // only costs time.
    let zipped = upload_compressed(rel);
    let body = if zipped { zstd::encode_all(&raw[..], 3).map_err(|_| "Could not compress a file.")? } else { raw };
    let props = json!({ "cv": "1", "dev": device, "sz": size.to_string(), "mt": mtime.to_string(), "z": if zipped { "1" } else { "0" } });
    let meta = match existing {
        Some(_) => json!({ "appProperties": props }),
        None => json!({ "name": rel, "parents": ["appDataFolder"], "appProperties": props }),
    };
    let sent = body.len() as u64;
    if body.len() <= SIMPLE_LIMIT {
        let boundary = format!("chatvault{}", hex(&random::<12>()));
        let mut multipart = Vec::with_capacity(body.len() + 512);
        multipart.extend_from_slice(format!("--{boundary}\r\nContent-Type: application/json; charset=UTF-8\r\n\r\n{meta}\r\n--{boundary}\r\nContent-Type: application/octet-stream\r\n\r\n").as_bytes());
        multipart.extend_from_slice(&body);
        multipart.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        let ctype = format!("multipart/related; boundary={boundary}");
        let r = send(|_| {
            let c = hc();
            let rb = match existing {
                Some(id) => c.patch(format!("{UPLOAD_URL}/{id}")),
                None => c.post(UPLOAD_URL),
            };
            rb.query(&[("uploadType", "multipart"), ("fields", "id")]).header("Content-Type", ctype.clone()).body(multipart.clone())
        })?;
        return Ok((sent, created_id(existing, r)));
    }
    // Resumable upload: start a session, then send the file in chunks. A chunk that fails is resumed from the byte
    // Google says it has, so an interrupted upload does not start over. The session address is also kept on disk, so
    // after ChatVault was closed in the middle of a big file, the next sync asks Google how much arrived and goes on.
    let total = body.len();
    let mut at = 0usize;
    let mut session = String::new();
    if let Some(saved) = load().uploads.get(rel).filter(|u| u.size == size && u.mtime == mtime && u.total == total && !u.session.is_empty() && now_ms() - u.started_ms < 5 * 24 * 3600 * 1000).cloned() {
        match probe_upload(&saved.session, total) {
            Probe::Done(id) => {
                update(|s| {
                    s.uploads.remove(rel);
                });
                return Ok((sent, if existing.is_some() { None } else { id }));
            }
            Probe::At(n) => {
                session = saved.session;
                at = n;
            }
            Probe::Gone => {}
        }
    }
    if session.is_empty() {
    session = send(|_| {
        let c = hc();
        let rb = match existing {
            Some(id) => c.patch(format!("{UPLOAD_URL}/{id}")),
            None => c.post(UPLOAD_URL),
        };
        rb.query(&[("uploadType", "resumable")])
            .header("Content-Type", "application/json; charset=UTF-8")
            .header("X-Upload-Content-Type", "application/octet-stream")
            .header("X-Upload-Content-Length", body.len().to_string())
            .body(meta.to_string())
    })?
    .headers()
    .get("location")
    .and_then(|l| l.to_str().ok())
    .map(String::from)
    .ok_or("Google Drive did not start the upload.")?;
    let (saved_session, rel_owned) = (session.clone(), rel.to_string());
    update(|s| {
        s.uploads.insert(rel_owned, SavedUpload { size, mtime, total, session: saved_session, started_ms: now_ms() });
    });
    }
    let mut failures = 0;
    while at < total {
        // Cancelled: stop between two pieces. The pieces already sent stay in Google's hands until the upload address
        // expires; nothing is left in the user's Drive.
        if is_stopped() {
            return Err("cancelled".into());
        }
        let end = (at + CHUNK).min(total);
        let piece = body[at..end].to_vec();
        let range = format!("bytes {}-{}/{}", at, end - 1, total);
        let res = http()?.put(&session).header("Content-Range", range).body(piece).send();
        match res {
            Ok(r) if r.status().is_success() => {
                update(|s| {
                    s.uploads.remove(rel);
                });
                return Ok((sent, created_id(existing, r)));
            }
            Ok(r) if r.status().as_u16() == 308 => {
                at = next_byte(r.headers().get("range").and_then(|h| h.to_str().ok()));
                failures = 0;
            }
            Ok(r) if r.status().as_u16() == 404 || r.status().as_u16() == 410 => return Err("The upload expired; it will start again on the next sync.".into()),
            _ => {
                failures += 1;
                if failures > 5 {
                    return Err("The upload was interrupted; it will continue on the next sync.".into());
                }
                std::thread::sleep(Duration::from_secs(1 << failures.min(4)));
                // Ask how much arrived, and carry on from there.
                if let Ok(r) = http()?.put(&session).header("Content-Range", format!("bytes */{total}")).send() {
                    if r.status().is_success() {
                        update(|s| {
                            s.uploads.remove(rel);
                        });
                        return Ok((sent, created_id(existing, r)));
                    }
                    if r.status().as_u16() == 308 {
                        at = next_byte(r.headers().get("range").and_then(|h| h.to_str().ok()));
                    }
                }
            }
        }
    }
    Ok((sent, None))
}

enum Probe {
    /// The whole file is already there (with the new file's id, when Google says it).
    Done(Option<String>),
    /// This many bytes have arrived: carry on from here.
    At(usize),
    /// The saved upload is gone or unusable: start the file again.
    Gone,
}

/// Asks Google how much of a saved upload has arrived.
fn probe_upload(session: &str, total: usize) -> Probe {
    let Ok(client) = http() else { return Probe::Gone };
    match client.put(session).header("Content-Range", format!("bytes */{total}")).header("Content-Length", "0").send() {
        Ok(r) if r.status().is_success() => Probe::Done(r.json::<Value>().ok().and_then(|v| v["id"].as_str().map(String::from))),
        Ok(r) if r.status().as_u16() == 308 => Probe::At(next_byte(r.headers().get("range").and_then(|h| h.to_str().ok()))),
        _ => Probe::Gone,
    }
}

/// The Drive id of a file this upload created (None for an update of an existing file).
fn created_id(existing: Option<&str>, r: reqwest::blocking::Response) -> Option<String> {
    if existing.is_some() {
        return None;
    }
    r.json::<Value>().ok().and_then(|v| v["id"].as_str().map(String::from))
}

/// The next byte to send after a `Range: bytes=0-N` answer (no header means nothing has arrived yet).
pub(crate) fn next_byte(range: Option<&str>) -> usize {
    range.and_then(|r| r.rsplit('-').next()).and_then(|n| n.trim().parse::<usize>().ok()).map(|n| n + 1).unwrap_or(0)
}

/// Files at least this big (as stored in Drive) are downloaded in a way that can carry on after an interruption.
const RESUME_DOWNLOAD_FROM: i64 = 4 * 1024 * 1024;

/// The bytes of a file as stored in Drive. A big one is saved piece by piece into a temporary file named after the
/// Drive file and its version, and a download that was interrupted (a lost connection, ChatVault closed) asks Google
/// for just the rest next time.
fn fetch_stored(r: &Remote) -> Result<Vec<u8>, String> {
    if r.bytes < RESUME_DOWNLOAD_FROM {
        return send(|_| hc().get(format!("{FILES_URL}/{}", r.id)).query(&[("alt", "media")]))?.bytes().map(|b| b.to_vec()).map_err(|e| net_err(&e));
    }
    let dir = data_dir().ok_or("No data folder.")?.join("downloads");
    fs::create_dir_all(&dir).map_err(|_| "Could not prepare the download.")?;
    let part = dir.join(format!("{}-{}-{}.part", r.id.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect::<String>(), r.bytes, r.mtime));
    let mut failures = 0;
    let mut rounds = 0;
    loop {
        // Never loop for ever if Google keeps sending something unexpected.
        rounds += 1;
        if rounds > 20 {
            let _ = fs::remove_file(&part);
            return Err("The download did not finish; it will be tried again on the next sync.".into());
        }
        let have = fs::metadata(&part).map(|m| m.len() as i64).unwrap_or(0);
        if have > r.bytes {
            let _ = fs::remove_file(&part);
            continue;
        }
        if have == r.bytes {
            break;
        }
        let mut resp = send(|_| {
            let rb = hc().get(format!("{FILES_URL}/{}", r.id)).query(&[("alt", "media")]);
            if have > 0 { rb.header("Range", format!("bytes={have}-")) } else { rb }
        })?;
        // 206: Google sent just the rest. 200: it sent everything, so start the file again.
        let append = have > 0 && resp.status().as_u16() == 206;
        let mut out = fs::OpenOptions::new().create(true).write(true).append(append).truncate(!append).open(&part).map_err(|_| "Could not write the download.")?;
        let copied = std::io::copy(&mut resp, &mut out);
        let _ = out.flush();
        if copied.is_err() {
            failures += 1;
            if failures > 5 {
                return Err("The download was interrupted; it will continue on the next sync.".into());
            }
            std::thread::sleep(Duration::from_secs(1 << failures.min(4)));
        }
    }
    let bytes = fs::read(&part).map_err(|_| "Could not read the download.")?;
    let _ = fs::remove_file(&part);
    Ok(bytes)
}

fn download(r: &Remote) -> Result<Vec<u8>, String> {
    let bytes = fetch_stored(r)?;
    if r.zipped {
        // Unpack at most a little more than the size the file says it has (and never more than 2 GB), so a damaged or
        // hostile file cannot fill the memory.
        let limit = (r.size.max(0) as u64).saturating_add(1024 * 1024).min(2 * 1024 * 1024 * 1024);
        let mut out = Vec::new();
        zstd::stream::read::Decoder::new(&bytes[..])
            .map_err(|_| "A file from Google Drive could not be unpacked.")?
            .take(limit + 1)
            .read_to_end(&mut out)
            .map_err(|_| "A file from Google Drive could not be unpacked.")?;
        if out.len() as u64 > limit {
            return Err("A file from Google Drive is larger than it should be and was skipped.".into());
        }
        Ok(out)
    } else {
        Ok(bytes.to_vec())
    }
}

/// Writes next to the target first and then swaps it in, so an interrupted download never leaves half a file.
fn write_atomic(path: &Path, data: &[u8], mtime: i64) -> Result<(), String> {
    if let Some(d) = path.parent() {
        fs::create_dir_all(d).map_err(|_| "Could not create a folder in the backup.")?;
    }
    let tmp = path.with_extension(format!("{}.cvpart", path.extension().map(|e| e.to_string_lossy().to_string()).unwrap_or_default()));
    {
        let mut f = fs::File::create(&tmp).map_err(|_| "Could not write to the backup folder.")?;
        f.write_all(data).map_err(|_| "Could not write to the backup folder.")?;
        let _ = f.set_modified(UNIX_EPOCH + Duration::from_millis(mtime.max(0) as u64));
    }
    fs::rename(&tmp, path).map_err(|_| {
        let _ = fs::remove_file(&tmp);
        "Could not write to the backup folder.".to_string()
    })
}

/// `name.ext` → `name.from-<pc>.ext`: where another PC's different copy of a file is kept, so nothing is overwritten.
pub(crate) fn conflict_path(path: &Path, pc: &str) -> PathBuf {
    let pc: String = pc.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '-' }).take(40).collect();
    let stem = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    match path.extension() {
        Some(ext) => path.with_file_name(format!("{stem}.from-{pc}.{}", ext.to_string_lossy())),
        None => path.with_file_name(format!("{stem}.from-{pc}")),
    }
}

/// What to do with a copy from Drive for a file that also exists here.
#[derive(Debug, PartialEq)]
pub(crate) enum Merge {
    /// Already the same, or the local file is newer and will be uploaded.
    Keep,
    /// Safe to replace the local file.
    Replace,
    /// Both changed: keep the other PC's copy next to it.
    KeepBoth,
}

/// Decides how a copy from Drive meets the local file. `local_unchanged` = the local file is exactly as it was when it
/// last matched Drive, so its content is already safe in Drive. `remote_extends_local` = the Drive copy starts with
/// the whole local file (a chat that only gained messages).
pub(crate) fn merge(local_size: i64, local_mtime: i64, remote_size: i64, remote_mtime: i64, local_unchanged: bool, remote_extends_local: bool) -> Merge {
    if local_size == remote_size && (local_mtime - remote_mtime).abs() < 2000 {
        return Merge::Keep;
    }
    if remote_extends_local && remote_size > local_size {
        return Merge::Replace;
    }
    if local_unchanged && remote_mtime > local_mtime {
        return Merge::Replace;
    }
    if remote_mtime > local_mtime {
        return Merge::KeepBoth;
    }
    Merge::Keep
}

#[derive(Serialize, Clone)]
struct Progress {
    phase: String,
    done: usize,
    total: usize,
    current: String,
}

fn progress(app: &AppHandle, phase: &str, done: usize, total: usize, current: &str) {
    let _ = app.emit("gdrive-progress", Progress { phase: phase.into(), done, total, current: current.into() });
}

/// Uploads this PC's changed backup files, downloads the other PCs' files, and (with `restore`) then copies anything
/// missing back into the tools. The local backup runs first so the newest chats go up.
pub fn sync(app: &AppHandle, restore: bool, scope: Option<&str>) -> Result<Stats, String> {
    if client().is_none() {
        return Err("Google Drive is not available in this build of ChatVault.".into());
    }
    if refresh_token().is_none() {
        return Err("Google Drive is not connected.".into());
    }
    let guard = drive_guard(None)?;
    if let Some(scope) = scope {
        validate_scope(scope)?;
    }
    begin_run();
    let result = run_sync(app, restore);
    CANCEL_SYNC.store(false, Ordering::SeqCst);
    match &result {
        // A cancelled sync is not a finished one: "last synced" stays as it was.
        Ok(stats) if stats.cancelled && !restore => {
            update(|s| {
                s.last_error = None;
                s.last_result = Some(stats.clone());
            });
        }
        Ok(stats) if stats.cancelled => {}
        Ok(stats) if !restore => {
            let st = stats.clone();
            update(|s| {
                s.last_sync_ms = Some(now_ms());
                s.last_error = None;
                s.last_result = Some(st);
            });
        }
        Ok(_) => {}
        Err(e) => {
            let e = e.clone();
            update(|s| s.last_error = Some(e));
        }
    }
    drop(guard);
    progress(app, "Done", 0, 0, "");
    result
}

fn run_sync(app: &AppHandle, restore: bool) -> Result<Stats, String> {
    let backup = if restore {
        crate::sync::SyncStats::default()
    } else {
        progress(app, "Backing up this PC", 0, 0, "");
        crate::sync::backup(app)?
    };
    let mut stats = Stats { backed_up: backup.copied, failed: backup.failed, ..Default::default() };
    if cancelled() {
        return Ok(Stats { cancelled: true, ..stats });
    }
    let root = backup_root().ok_or("No backup folder.")?;
    fs::create_dir_all(&root).map_err(|_| "Could not create the backup folder.")?;
    let me = load().device_id;
    // Keep the device id from the first load for good.
    update(|s| s.device_id = me.clone());

    if cached_listing().is_none() {
        progress(app, "Checking Google Drive", 0, 0, "");
    }
    let all = match list_all() {
        Ok(a) => a,
        Err(_) if cancelled() => return Ok(Stats { cancelled: true, ..stats }),
        Err(e) => return Err(e),
    };
    let remote: Vec<Remote> = all.iter().filter_map(remote_of).collect();
    let mut state = load();

    // 1. Upload what changed on this PC.
    let mine: HashMap<&str, &Remote> = remote.iter().filter(|r| r.device == me).map(|r| (r.rel.as_str(), r)).collect();
    let local = local_files(&root);
    let remote_versions = remote_versions(&remote);
    let todo: Vec<_> = local.iter().filter(|(rel, _, size, mtime)| {
        !restore && needs_upload(&remote_versions, rel, *size, *mtime)
    }).collect();
    stats.unchanged = local.len() - todo.len();
    let items: Vec<(String, PathBuf, i64, i64)> = todo.iter().map(|t| (*t).clone()).collect();
    let up = upload_many(app, &items, &mine, &me, None);
    for (i, n) in &up.done {
        let (rel, _, size, mtime) = &items[*i];
        stats.uploaded += 1;
        stats.bytes_up += n;
        state.synced.insert(rel.clone(), (*size, *mtime));
    }
    stats.failed += up.failed;
    save_merge(&state);
    // Cancelled: what went up stays (and is recorded above), so the next sync goes on from here.
    if cancelled() {
        stats.cancelled = true;
        return Ok(stats);
    }
    if let Some(e) = up.fatal {
        return Err(e);
    }

    // 2. Download what the other PCs (or this PC before a reinstall) have that is newer or missing here.
    let mut best: HashMap<&str, &Remote> = HashMap::new();
    for r in &remote {
        let Some(rel) = safe_rel(&r.rel) else { continue };
        if r.device == me && root.join(rel).exists() {
            continue;
        }
        let better = match best.get(r.rel.as_str()) {
            None => true,
            Some(b) => (r.mtime, r.size, &r.id) > (b.mtime, b.size, &b.id),
        };
        if better {
            best.insert(r.rel.as_str(), r);
        }
    }
    let names = device_names(&all);
    let candidates: Vec<&Remote> = best
        .into_values()
        .filter(|r| {
            let missing = safe_rel(&r.rel).is_some_and(|p| !root.join(p).exists());
            (missing || state.applied.get(&r.rel) != Some(&r.version()))
                && ((restore && missing) || state.skip_download.get(&r.rel) != Some(&r.version()))
        })
        .collect();
    for (i, r) in candidates.iter().enumerate() {
        if cancelled() {
            stats.cancelled = true;
            break;
        }
        progress(app, "Downloading", i, candidates.len(), &r.rel);
        let Some(rel_path) = safe_rel(&r.rel) else { continue };
        let path = root.join(&rel_path);
        let outcome = (|| -> Result<(), String> {
            match fs::metadata(&path) {
                Err(_) => {
                    let data = download(r)?;
                    write_atomic(&path, &data, r.mtime)?;
                    stats.downloaded += 1;
                    stats.bytes_down += data.len() as u64;
                    state.synced.insert(r.rel.clone(), (data.len() as i64, r.mtime));
                }
                Ok(m) => {
                    let size = m.len() as i64;
                    let mtime = m.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_millis() as i64).unwrap_or(0);
                    let unchanged = state.synced.get(&r.rel) == Some(&(size, mtime));
                    // Only a larger, newer copy can be "the same chat with more messages"; download it to check.
                    let maybe_extends = r.size > size;
                    let quick = merge(size, mtime, r.size, r.mtime, unchanged, false);
                    if quick == Merge::Keep && !maybe_extends {
                        return Ok(());
                    }
                    let data = download(r)?;
                    let local = fs::read(&path).map_err(|_| "Could not read the backup folder.")?;
                    let extends = data.len() > local.len() && data.starts_with(&local);
                    match merge(size, mtime, r.size, r.mtime, unchanged, extends) {
                        Merge::Keep => {}
                        Merge::Replace => {
                            write_atomic(&path, &data, r.mtime)?;
                            stats.downloaded += 1;
                            stats.bytes_down += data.len() as u64;
                            state.synced.insert(r.rel.clone(), (data.len() as i64, r.mtime));
                        }
                        Merge::KeepBoth => {
                            let pc = names.get(&r.device).cloned().unwrap_or_else(|| r.device.clone());
                            let other = conflict_path(&path, &pc);
                            if !other.exists() {
                                write_atomic(&other, &data, r.mtime)?;
                                stats.conflicts += 1;
                                stats.bytes_down += data.len() as u64;
                            }
                        }
                    }
                }
            }
            Ok(())
        })();
        match outcome {
            Ok(()) => {
                state.applied.insert(r.rel.clone(), r.version());
            }
            Err(e) if e.contains("not connected") || e.contains("removed") => {
                save_merge(&state);
                return Err(e);
            }
            Err(_) => stats.failed += 1,
        }
        if i % 25 == 24 {
            save_merge(&state);
        }
    }
    save_merge(&state);

    // 3. This PC's entry in the list of PCs, and the list itself for the dialog.
    if !restore && !stats.cancelled && (!items.is_empty() || !device_names(&all).contains_key(&me)
        || load().last_sync_ms.is_none_or(|last| now_ms() - last >= 8 * 60 * 1000))
    {
        publish_device(&all, &me, local.len());
    }
    let mut devices: Vec<Device> = Vec::new();
    for v in &all {
        if v["appProperties"]["cv"].as_str() == Some("dev") {
            if let Some(id) = v["appProperties"]["dev"].as_str() {
                let files = remote.iter().filter(|r| r.device == id).count();
                devices.push(Device {
                    id: id.to_string(),
                    name: v["appProperties"]["pc"].as_str().unwrap_or("A PC").to_string(),
                    last_sync_ms: v["appProperties"]["at"].as_str().and_then(|s| s.parse().ok()),
                    files,
                    this_pc: id == me,
                });
            }
        }
    }
    if !devices.iter().any(|d| d.this_pc) {
        devices.push(Device { id: me.clone(), name: this_pc_name(), last_sync_ms: Some(now_ms()), files: local.len(), this_pc: true });
    }
    for d in devices.iter_mut().filter(|d| d.this_pc) {
        d.last_sync_ms = Some(now_ms());
        d.files = local.len();
        d.name = this_pc_name();
    }
    devices.sort_by(|a, b| b.this_pc.cmp(&a.this_pc).then(b.last_sync_ms.cmp(&a.last_sync_ms)));
    update(|s| s.devices = devices);

    if stats.cancelled {
        return Ok(stats);
    }
    if restore {
        progress(app, "Restoring into the tools", 0, 0, "");
        let restored = crate::sync::restore(app)?;
        stats.restored = restored.copied;
        stats.failed += restored.failed;
        // Stopped while copying into the tools: what was copied stays, the rest is copied next time.
        stats.cancelled = cancelled();
    }
    Ok(stats)
}

/// How many files go up at the same time. One request at a time took about a second per file, so a first backup of
/// thousands of pictures and chats took hours; a handful at once is fast and still gentle on Google's limits.
const UPLOAD_WORKERS: usize = 6;

struct Uploaded {
    /// (index in the list, bytes sent) for each file that went up.
    done: Vec<(usize, u64)>,
    failed: usize,
    /// A problem that stops the whole sync (Drive full, access removed).
    fatal: Option<String>,
}

/// Uploads the files with several workers. The list order, progress and the first fatal error are all kept.
fn upload_many(
    app: &AppHandle, items: &[(String, PathBuf, i64, i64)], mine: &HashMap<&str, &Remote>, me: &str,
    job: Option<(&str, &AtomicBool)>,
) -> Uploaded {
    use std::sync::atomic::AtomicUsize;
    let is_stopped = || job.map_or_else(cancelled, |(_, stop)| stop.load(Ordering::Acquire));
    let report = |done, total, current: &str| {
        if let Some((id, _)) = job {
            let _ = app.emit("backup-upload-progress", json!({ "id": id, "done": done, "total": total }));
        } else {
            progress(app, "Uploading", done, total, current);
        }
    };
    let next = AtomicUsize::new(0);
    let finished = AtomicUsize::new(0);
    let stop = AtomicBool::new(false);
    let out = Mutex::new(Uploaded { done: Vec::new(), failed: 0, fatal: None });
    if let Some((rel, ..)) = items.first() {
        report(0, items.len(), rel);
    }
    std::thread::scope(|scope| {
        for _ in 0..UPLOAD_WORKERS.min(items.len().max(1)) {
            scope.spawn(|| loop {
                if stop.load(Ordering::SeqCst) || is_stopped() {
                    break;
                }
                let i = next.fetch_add(1, Ordering::SeqCst);
                let Some((rel, path, size, mtime)) = items.get(i) else { break };
                let existing = mine.get(rel.as_str()).map(|r| r.id.as_str());
                let res = upload_cancellable(existing, rel, me, path, *size, *mtime, &is_stopped);
                let n = finished.fetch_add(1, Ordering::SeqCst) + 1;
                if n % 5 == 0 || n == items.len() {
                    report(n, items.len(), rel);
                }
                let mut o = out.lock().unwrap_or_else(|e| e.into_inner());
                match res {
                    Ok((bytes, created)) => {
                        if let Some(id) = created.as_deref().or(existing) {
                            update_listing(json!({
                                "id": id,
                                "name": rel,
                                "size": bytes.to_string(),
                                "appProperties": {
                                    "cv": "1", "dev": me, "sz": size.to_string(),
                                    "mt": mtime.to_string(), "z": if upload_compressed(rel) { "1" } else { "0" }
                                }
                            }));
                        } else {
                            invalidate_listing();
                        }
                        if job.is_none() { RUN_DONE.fetch_add(1, Ordering::SeqCst); }
                        o.done.push((i, bytes));
                    }
                    Err(e) if e == "cancelled" => stop.store(true, Ordering::SeqCst),
                    Err(e) if e.contains("full") || e.contains("not connected") || e.contains("removed") => {
                        o.fatal.get_or_insert(e);
                        stop.store(true, Ordering::SeqCst);
                    }
                    Err(_) => o.failed += 1,
                }
            });
        }
    });
    out.into_inner().unwrap_or_else(|e| e.into_inner())
}

/// Saves the sync bookkeeping without losing a setting changed meanwhile in the dialog.
fn save_merge(work: &State) {
    let synced = work.synced.clone();
    let applied = work.applied.clone();
    let skip_download = work.skip_download.clone();
    update(|s| {
        s.synced = synced;
        s.applied = applied;
        s.skip_download = skip_download;
    });
}

pub(crate) fn device_names(all: &[Value]) -> HashMap<String, String> {
    all.iter()
        .filter(|v| v["appProperties"]["cv"].as_str() == Some("dev"))
        .filter_map(|v| Some((v["appProperties"]["dev"].as_str()?.to_string(), v["appProperties"]["pc"].as_str()?.to_string())))
        .collect()
}

/// A small file per PC (name and time of its last sync, in its properties) so every PC can show the others.
fn publish_device(all: &[Value], me: &str, files: usize) {
    let existing = all.iter().find(|v| v["appProperties"]["cv"].as_str() == Some("dev") && v["appProperties"]["dev"].as_str() == Some(me)).and_then(|v| v["id"].as_str());
    let pc: String = this_pc_name().chars().take(60).collect();
    let props = json!({ "cv": "dev", "dev": me, "pc": pc, "at": now_ms().to_string(), "n": files.to_string() });
    let result = metadata_request(false, || {
        let c = hc();
        match existing {
            Some(id) => c.patch(format!("{FILES_URL}/{id}")),
            None => c.post(FILES_URL),
        }
        .query(&[("fields", "id,name,size,appProperties")])
        .json(&match existing {
            Some(_) => json!({ "appProperties": props }),
            None => json!({ "name": format!("device-{me}.json"), "parents": ["appDataFolder"], "mimeType": "application/json", "appProperties": props }),
        })
    }).and_then(|response| response.json::<Value>().map_err(|e| net_err(&e)));
    if let Ok(value) = result {
        update_listing(value);
    }
}

fn update_listing(value: Value) {
    if let Some((_, all)) = LIST_CACHE.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        all.retain(|v| v["id"] != value["id"]);
        all.push(value);
    }
}

// ---------------------------------------------------------------- the Backups tab

/// Whether Google Drive is connected on this PC (and this build has it).
pub(crate) fn connected() -> bool {
    client().is_some() && load().connected && refresh_token().is_some()
}

/// Everything ChatVault keeps in Drive, with the names of the PCs that wrote it.
pub(crate) fn remote_snapshot() -> Result<(Vec<Remote>, HashMap<String, String>), String> {
    let all = list_all()?;
    Ok((all.iter().filter_map(remote_of).collect(), device_names(&all)))
}

/// Runs `f` as the one Drive job at a time (a sync, a delete or a transfer never overlap).
pub(crate) fn exclusive<T>(f: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    let _guard = drive_guard(None)?;
    f()
}

pub(crate) fn scoped<T>(units: &HashSet<Unit>, f: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    if units.is_empty() {
        return Err("Choose at least one backup entry.".into());
    }
    let _guard = drive_guard(Some(units))?;
    f()
}

fn delete_remote(id: &str) -> Result<(), String> {
    metadata_request(true, || hc().delete(format!("{FILES_URL}/{id}"))).map(|_| ())
}

/// What a delete from the Backups tab removed.
#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Removed {
    pub local_files: usize,
    pub drive_files: usize,
    pub failed: usize,
    pub error: Option<String>,
    pub stopped: bool,
    pub remaining: Vec<String>,
}

/// Deletes the selected Drive copies. An unchanged copy still on this PC can be uploaded again.
pub(crate) fn delete_drive(
    units: &HashSet<(String, String)>,
    out: &mut Removed,
    stop: &AtomicBool,
    retry: Option<&HashSet<String>>,
    report: impl Fn(usize, usize) + Sync,
) -> Result<(), String> {
    let (remote, _) = remote_snapshot()?;
    let targets: Vec<_> = remote.into_iter().filter(|r| {
        retry.is_none_or(|ids| ids.contains(&r.id))
            && crate::backups::unit_of(&r.rel).is_some_and(|u| units.contains(&u))
    }).collect();
    let (deleted, error) = delete_parallel(&targets, stop, |r| delete_remote(&r.id), report);
    let mut ids = HashSet::new();
    for i in deleted {
        let r = &targets[i];
        ids.insert(r.id.as_str());
    }
    out.drive_files = ids.len();
    out.remaining = targets.iter().filter(|r| !ids.contains(r.id.as_str())).map(|r| r.id.clone()).collect();
    out.stopped = stop.load(Ordering::Acquire);
    out.error = error;
    if !ids.is_empty() {
        if let Some((_, all)) = LIST_CACHE.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
            all.retain(|v| !v["id"].as_str().is_some_and(|id| ids.contains(id)));
        }
        update(|s| {
            for r in targets.iter().filter(|r| ids.contains(r.id.as_str())) {
                s.synced.remove(&r.rel);
                s.applied.remove(&r.rel);
            }
            s.last_result = None;
        });
    }
    Ok(())
}

fn delete_parallel<T: Sync>(
    targets: &[T],
    stop: &AtomicBool,
    delete: impl Fn(&T) -> Result<(), String> + Sync,
    report: impl Fn(usize, usize) + Sync,
) -> (Vec<usize>, Option<String>) {
    use std::sync::atomic::AtomicUsize;
    let next = AtomicUsize::new(0);
    let failed = AtomicBool::new(false);
    let result = Mutex::new((Vec::new(), None));
    report(0, targets.len());
    std::thread::scope(|scope| {
        for _ in 0..UPLOAD_WORKERS.min(targets.len()) {
            scope.spawn(|| loop {
                if stop.load(Ordering::Acquire) || failed.load(Ordering::Acquire) {
                    break;
                }
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some(target) = targets.get(i) else { break };
                let attempt = delete(target);
                let mut result = result.lock().unwrap_or_else(|e| e.into_inner());
                match attempt {
                    Ok(()) => {
                        result.0.push(i);
                        report(result.0.len(), targets.len());
                    }
                    Err(error) => {
                        failed.store(true, Ordering::Release);
                        result.1.get_or_insert(error);
                    }
                }
            });
        }
    });
    result.into_inner().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
fn forget_remote_id(id: &str) {
    if let Some((_, all)) = LIST_CACHE.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        all.retain(|v| v["id"].as_str() != Some(id));
    }
}

/// After local copies were deleted: do not download the Drive versions that are there now.
pub(crate) fn forget_local(rels: &[String]) {
    let mut best: HashMap<String, Remote> = HashMap::new();
    let cached = LIST_CACHE.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(|(_, all)| all.clone());
    for r in cached.unwrap_or_default().iter().filter_map(remote_of) {
        if best.get(&r.rel).is_none_or(|previous| (r.mtime, r.size, &r.id) > (previous.mtime, previous.size, &previous.id)) {
            best.insert(r.rel.clone(), r);
        }
    }
    update(|s| {
        for rel in rels {
            s.synced.remove(rel);
            s.applied.remove(rel);
            if let Some(r) = best.get(rel) {
                s.skip_download.insert(rel.clone(), r.version());
            }
        }
    });
}

pub fn selected_upload(
    app: &AppHandle, units: &HashSet<Unit>, id: &str, stop: &AtomicBool, scope: &str,
) -> Result<Stats, String> {
    scoped(units, || {
        validate_scope(scope)?;
        let _ = app.emit("backup-upload-progress", json!({ "id": id, "done": 0, "total": 0 }));
        let root = backup_root().ok_or("No backup folder.")?;
        let me = update(|_| {}).device_id;
        let remote = list_remote()?;
        let mine: HashMap<_, _> = remote.iter().filter(|r| r.device == me).map(|r| (r.rel.as_str(), r)).collect();
        let versions = remote_versions(&remote);
        let mut unchanged = 0;
        let todo: Vec<_> = local_files(&root).into_iter().filter(|(rel, _, size, mtime)| {
            if !crate::backups::unit_of(rel).is_some_and(|unit| units.contains(&unit)) {
                return false;
            }
            if needs_upload(&versions, rel, *size, *mtime) {
                return true;
            }
            unchanged += 1;
            false
        }).collect();
        let uploads = upload_many(app, &todo, &mine, &me, Some((id, stop)));
        if !uploads.done.is_empty() {
            update(|state| {
                for (i, _) in &uploads.done {
                    let (rel, _, size, mtime) = &todo[*i];
                    state.synced.insert(rel.clone(), (*size, *mtime));
                }
            });
        }
        if let Some(error) = uploads.fatal {
            return Err(error);
        }
        Ok(Stats {
            uploaded: uploads.done.len(), bytes_up: uploads.done.iter().map(|(_, n)| n).sum(),
            unchanged, failed: uploads.failed, cancelled: stop.load(Ordering::Acquire),
            ..Default::default()
        })
    })
}

/// Uploads (`up`) or downloads the files of the given backup entries, for the Backups tab. A file that differs on
/// both sides is never overwritten: the other PC's copy is kept next to it.
pub fn transfer(app: &AppHandle, units: &HashSet<(String, String)>, up: bool) -> Result<Stats, String> {
    if !connected() {
        return Err("Google Drive is not connected.".into());
    }
    exclusive(|| {
        begin_run();
        let root = backup_root().ok_or("No backup folder.")?;
        let me = load().device_id;
        let all = list_all()?;
        let remote: Vec<Remote> = all.iter().filter_map(remote_of).collect();
        let names = device_names(&all);
        let mut state = load();
        let mut stats = Stats::default();
        let inside = |rel: &str| crate::backups::unit_of(rel).is_some_and(|u| units.contains(&u));
        if up {
            let mine: HashMap<&str, &Remote> = remote.iter().filter(|r| r.device == me).map(|r| (r.rel.as_str(), r)).collect();
            let all_here: Vec<_> = local_files(&root).into_iter().filter(|(rel, ..)| inside(rel)).collect();
            let versions = remote_versions(&remote);
            let todo: Vec<_> = all_here.iter().filter(|(rel, _, size, mtime)| needs_upload(&versions, rel, *size, *mtime)).cloned().collect();
            stats.unchanged += all_here.len() - todo.len();
            let up = upload_many(app, &todo, &mine, &me, None);
            for (i, n) in &up.done {
                let (rel, _, size, mtime) = &todo[*i];
                stats.uploaded += 1;
                stats.bytes_up += n;
                state.synced.insert(rel.clone(), (*size, *mtime));
            }
            stats.failed += up.failed;
            if cancelled() {
                stats.cancelled = true;
                save_merge(&state);
                progress(app, "Done", 0, 0, "");
                return Ok(stats);
            }
            if let Some(e) = up.fatal {
                save_merge(&state);
                return Err(e);
            }
        } else {
            let mut best: HashMap<&str, &Remote> = HashMap::new();
            for r in remote.iter().filter(|r| inside(&r.rel)) {
                if best.get(r.rel.as_str()).is_none_or(|b| (r.mtime, r.size, &r.id) > (b.mtime, b.size, &b.id)) {
                    best.insert(r.rel.as_str(), r);
                }
            }
            let list: Vec<&&Remote> = best.values().collect();
            for (i, r) in list.iter().enumerate() {
                // Cancelled: what was downloaded so far stays (each file is written whole), the rest comes next time.
                if cancelled() {
                    stats.cancelled = true;
                    break;
                }
                progress(app, "Downloading", i, list.len(), &r.rel);
                let Some(rel_path) = safe_rel(&r.rel) else { continue };
                let path = root.join(&rel_path);
                let res = (|| -> Result<(), String> {
                    let local = fs::metadata(&path).ok();
                    if let Some(m) = &local {
                        let mtime = m.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_millis() as i64).unwrap_or(0);
                        if m.len() as i64 == r.size && (mtime - r.mtime).abs() < 2000 {
                            stats.unchanged += 1;
                            return Ok(());
                        }
                    }
                    let data = download(r)?;
                    match local {
                        None => {
                            write_atomic(&path, &data, r.mtime)?;
                            stats.downloaded += 1;
                        }
                        Some(_) => {
                            let current = fs::read(&path).map_err(|_| "Could not read the backup folder.")?;
                            if data.len() > current.len() && data.starts_with(&current) {
                                write_atomic(&path, &data, r.mtime)?;
                                stats.downloaded += 1;
                            } else if data != current {
                                let other = conflict_path(&path, names.get(&r.device).map(String::as_str).unwrap_or(&r.device));
                                if !other.exists() {
                                    write_atomic(&other, &data, r.mtime)?;
                                    stats.conflicts += 1;
                                }
                            }
                        }
                    }
                    stats.bytes_down += data.len() as u64;
                    Ok(())
                })();
                match res {
                    Ok(()) => {
                        state.applied.insert(r.rel.clone(), r.version());
                        state.skip_download.remove(&r.rel);
                    }
                    Err(_) => stats.failed += 1,
                }
            }
        }
        save_merge(&state);
        progress(app, "Done", 0, 0, "");
        Ok(stats)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bulk_delete_finishes_all_296_targets_and_limits_concurrency() {
        use std::sync::atomic::AtomicUsize;
        let targets: Vec<_> = (0..296).collect();
        let stop = AtomicBool::new(false);
        let active = AtomicUsize::new(0);
        let peak = AtomicUsize::new(0);
        let (deleted, error) = delete_parallel(&targets, &stop, |_| {
            let n = active.fetch_add(1, Ordering::SeqCst) + 1;
            peak.fetch_max(n, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(2));
            active.fetch_sub(1, Ordering::SeqCst);
            Ok(())
        }, |_, _| {});
        assert!(error.is_none());
        assert_eq!(deleted.into_iter().collect::<HashSet<_>>().len(), targets.len());
        assert!((2..=UPLOAD_WORKERS).contains(&peak.load(Ordering::SeqCst)));
    }

    #[test]
    fn stopping_or_failing_preserves_the_unprocessed_targets() {
        let targets: Vec<_> = (0..296).collect();
        let stop = AtomicBool::new(false);
        let (deleted, error) = delete_parallel(&targets, &stop, |_| Ok(()), |done, _| {
            if done == 12 {
                stop.store(true, Ordering::Release);
            }
        });
        assert!(error.is_none());
        assert!((12..12 + UPLOAD_WORKERS).contains(&deleted.len()));

        let stop = AtomicBool::new(false);
        let (deleted, error) = delete_parallel(&targets, &stop, |i| {
            if *i == 12 { Err("Drive refused a request".into()) } else { Ok(()) }
        }, |_, _| {});
        assert_eq!(error.as_deref(), Some("Drive refused a request"));
        assert!(!deleted.contains(&12));
        assert!(deleted.len() < targets.len() - 1);
    }

    /// A real round trip against the signed-in Google Drive, with throwaway files written under a device id of their own
    /// ("selftest-device"), so nothing of the user's is read, changed or deleted. Run by hand:
    /// `cargo test --lib live_drive_roundtrip -- --ignored --nocapture`
    #[test]
    #[ignore = "uses the connected Google Drive"]
    fn live_drive_roundtrip() {
        if !connected() {
            println!("Google Drive is not connected on this PC: nothing to test.");
            return;
        }
        const DEV: &str = "selftest-roundtrip";
        let dir = std::env::temp_dir().join(format!("chatvault-live-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let mut created: Vec<String> = Vec::new();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            // Files: a small chat (compressed on upload), a picture, and a 6 MB file that needs a resumable upload.
            let chat = dir.join("chat.jsonl");
            fs::write(&chat, "{\"type\":\"user\"}\n".repeat(200)).unwrap();
            let pic = dir.join("pixel.png");
            fs::write(&pic, [137u8, 80, 78, 71, 13, 10, 26, 10, 1, 2, 3]).unwrap();
            let big = dir.join("big.png");
            let mut big_bytes = vec![0u8; 6 * 1024 * 1024 + 123];
            getrandom::fill(&mut big_bytes).unwrap();
            fs::write(&big, &big_bytes).unwrap();
            let meta = |p: &Path| {
                let m = fs::metadata(p).unwrap();
                (m.len() as i64, m.modified().unwrap().duration_since(UNIX_EPOCH).unwrap().as_millis() as i64)
            };
            let (chat_rel, pic_rel, big_rel) = ("claude/projects/zz-selftest/live-1.jsonl", "claude/projects/zz-selftest/live-1/tool-results/pixel.png", "claude/projects/zz-selftest/live-1/tool-results/big.png");

            // 1. Uploads: small (multipart, compressed), picture (as is), big (resumable).
            for (rel, p) in [(chat_rel, &chat), (pic_rel, &pic)] {
                let (size, mtime) = meta(p);
                let (sent, id) = upload(None, rel, DEV, p, size, mtime).expect("upload");
                assert!(sent > 0);
                created.push(id.expect("a new file reports its Drive id"));
            }
            // 2. A big file interrupted halfway: start the upload session by hand, send 1 MiB, then ask Google how much
            // arrived, exactly as a sync after a restart does, and let upload() carry on from there.
            let (bsize, bmtime) = meta(&big);
            let total = big_bytes.len();
            let props = json!({ "cv": "1", "dev": DEV, "sz": bsize.to_string(), "mt": bmtime.to_string(), "z": "0" });
            let meta_json = json!({ "name": big_rel, "parents": ["appDataFolder"], "appProperties": props });
            let session = send(|_| {
                hc().post(UPLOAD_URL)
                    .query(&[("uploadType", "resumable")])
                    .header("Content-Type", "application/json; charset=UTF-8")
                    .header("X-Upload-Content-Length", total.to_string())
                    .body(meta_json.to_string())
            })
            .expect("start the upload")
            .headers()
            .get("location")
            .and_then(|l| l.to_str().ok())
            .map(String::from)
            .expect("an upload address");
            let first = 1024 * 1024;
            let r = http().unwrap().put(&session).header("Content-Range", format!("bytes 0-{}/{}", first - 1, total)).body(big_bytes[..first].to_vec()).send().unwrap();
            assert_eq!(r.status().as_u16(), 308, "Google keeps the first piece");
            assert!(matches!(probe_upload(&session, total), Probe::At(n) if n == first), "Google says how much arrived");
            update(|s| {
                s.uploads.insert(big_rel.to_string(), SavedUpload { size: bsize, mtime: bmtime, total, session: session.clone(), started_ms: now_ms() });
            });
            let (_, id) = upload(None, big_rel, DEV, &big, bsize, bmtime).expect("resumed upload");
            assert!(load().uploads.get(big_rel).is_none(), "the saved upload is forgotten once it is done");
            // The resumed upload finished the same file: find it in Drive.
            let all = remote_snapshot().expect("list").0;
            let mine: Vec<&Remote> = all.iter().filter(|r| r.device == DEV).collect();
            assert_eq!(mine.len(), 3, "exactly the three throwaway files: {:?}", mine.iter().map(|r| &r.rel).collect::<Vec<_>>());
            let big_remote = mine.iter().find(|r| r.rel == big_rel).expect("the big file is in Drive");
            assert_eq!(big_remote.bytes as usize, total, "the whole file arrived");
            created.extend(mine.iter().map(|r| r.id.clone()));
            let _ = id;

            // 3. The Backups tab's view: local and Drive agree.
            let local: HashMap<String, (i64, i64)> = [(chat_rel, &chat), (pic_rel, &pic), (big_rel, &big)].iter().map(|(rel, p)| (rel.to_string(), meta(p))).collect();
            let remote: HashMap<String, (i64, i64)> = mine.iter().map(|r| (r.rel.clone(), (r.size, r.mtime))).collect();
            assert_eq!(crate::backups::status_of(&local, &remote, true), "synced");
            let versions = remote_versions(&all);
            for (rel, (size, mtime)) in &local {
                assert!(!needs_upload(&versions, rel, *size, *mtime), "unchanged files are skipped");
                assert!(needs_upload(&versions, rel, *size + 1, *mtime), "changed content is uploaded");
                assert!(needs_upload(&versions, rel, *size, *mtime + 1), "same-size edits are uploaded");
            }

            assert_eq!(crate::backups::unit_of(chat_rel), crate::backups::unit_of(big_rel), "the chat and its pictures are one entry");

            // 4. Downloads: small file, and the big one after a "closed in the middle" (first half already on disk).
            let small_remote = mine.iter().find(|r| r.rel == chat_rel).unwrap();
            assert_eq!(download(small_remote).expect("download"), fs::read(&chat).unwrap());
            let part = data_dir().unwrap().join("downloads").join(format!("{}-{}-{}.part", big_remote.id.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect::<String>(), big_remote.bytes, big_remote.mtime));
            fs::create_dir_all(part.parent().unwrap()).unwrap();
            let stored_first_half = (big_remote.bytes / 2) as usize;
            fs::write(&part, &big_bytes[..stored_first_half]).unwrap();
            assert_eq!(download(big_remote).expect("resumed download"), big_bytes, "the rest was fetched and joined to what was on disk");
            assert!(!part.exists(), "the temporary file is removed when the download is done");

            // 5. Delete through the same handler as the Backups tab, then re-upload the retained copy.
            assert!(all.iter().filter(|r| crate::backups::unit_of(&r.rel) == crate::backups::unit_of(chat_rel)).all(|r| r.device == DEV));
            let units = HashSet::from([crate::backups::unit_of(chat_rel).unwrap()]);
            let mut removed = Removed::default();
            exclusive(|| delete_drive(&units, &mut removed, &AtomicBool::new(false), None, |_, _| {})).expect("delete the test chat from Drive");
            assert_eq!(removed.drive_files, 3);
            assert!(removed.error.is_none());
            let remaining = remote_snapshot().expect("list after delete").0;
            let (size, mtime) = meta(&chat);
            assert!(needs_upload(&remote_versions(&remaining), chat_rel, size, mtime));
            let (_, id) = upload(None, chat_rel, DEV, &chat, size, mtime).expect("re-upload");
            created.push(id.expect("re-upload returns its id"));
            refresh_listing();
            let after = remote_snapshot().expect("list after re-upload").0;
            assert!(!needs_upload(&remote_versions(&after), chat_rel, size, mtime));
            println!("Live Drive upload, unchanged skip, download, chat deletion, and re-upload passed.");
        }));
        // Always clean up, even if a check failed: remove whatever the test created in Drive, and its local files.
        for id in &created {
            if delete_remote(id).is_ok() {
                forget_remote_id(id);
            }
        }
        let leftovers = remote_snapshot().map(|(r, _)| r.into_iter().filter(|r| r.device == DEV).collect::<Vec<_>>()).unwrap_or_default();
        for r in leftovers {
            let _ = delete_remote(&r.id);
        }
        update(|s| s.uploads.retain(|k, _| !k.contains("zz-selftest")));
        let _ = fs::remove_dir_all(&dir);
        if let Err(e) = result {
            std::panic::resume_unwind(e);
        }
    }

    #[test]
    fn cancel_is_only_seen_while_a_drive_job_runs() {
        BUSY.store(false, Ordering::SeqCst);
        CANCEL_SYNC.store(true, Ordering::SeqCst);
        assert!(!cancel_requested(), "a leftover flag must not stop a local backup");
        BUSY.store(true, Ordering::SeqCst);
        assert!(cancel_requested());
        BUSY.store(false, Ordering::SeqCst);
        CANCEL_SYNC.store(false, Ordering::SeqCst);
    }

    #[test]
    fn pkce_challenge_is_the_s256_of_the_verifier() {
        let (v, c) = pkce();
        assert!((43..=128).contains(&v.len()), "RFC 7636 length");
        assert!(v.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_'));
        assert_eq!(c, b64url(&Sha256::digest(v.as_bytes())));
        assert_ne!(pkce().0, v, "a new verifier every time");
        // RFC 7636 appendix B test vector.
        assert_eq!(b64url(&Sha256::digest(b"dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk")), "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    }

    #[test]
    fn callback_needs_the_right_state_and_path() {
        assert_eq!(parse_callback("GET /?state=abc&code=4%2F0Ab HTTP/1.1", "abc"), Callback::Code("4/0Ab".into()));
        assert_eq!(parse_callback("GET /?state=abc&error=access_denied HTTP/1.1", "abc"), Callback::Denied("access_denied".into()));
        assert_eq!(parse_callback("GET /?state=evil&code=x HTTP/1.1", "abc"), Callback::Other, "a wrong state is ignored");
        assert_eq!(parse_callback("GET /?code=x HTTP/1.1", "abc"), Callback::Other, "a missing state is ignored");
        assert_eq!(parse_callback("GET /favicon.ico HTTP/1.1", "abc"), Callback::Other);
        assert_eq!(parse_callback("POST /?state=abc&code=x HTTP/1.1", "abc"), Callback::Other);
        assert_eq!(parse_callback("", "abc"), Callback::Other);
    }

    #[test]
    fn paths_from_drive_cannot_leave_the_backup_folder() {
        assert_eq!(safe_rel("claude/projects/C--x/abc.jsonl"), Some(PathBuf::from("claude").join("projects").join("C--x").join("abc.jsonl")));
        for bad in [
            "../evil.jsonl",
            "claude/../../evil",
            "claude/./x",
            "C:/Windows/evil",
            "claude\\..\\evil",
            "/claude/x",
            "evil/x.jsonl",
            "claude",
            "claude//x",
            "claude/x:stream",
            "claude/con.",
            "claude/x\u{0}y",
            "claude/CON",
            "claude/nul.jsonl",
            "codex/com1.txt",
        ] {
            assert!(safe_rel(bad).is_none(), "{bad} must be refused");
        }
    }

    #[test]
    fn merging_never_loses_a_local_change() {
        // Same file: nothing to do.
        assert_eq!(merge(100, 5000, 100, 5500, false, false), Merge::Keep);
        // The other PC's copy is the same chat with more messages: take it.
        assert_eq!(merge(100, 5000, 180, 9000, false, true), Merge::Replace);
        // Local file untouched since it last matched Drive, and Drive has a newer one: take it.
        assert_eq!(merge(100, 5000, 90, 9000, true, false), Merge::Replace);
        // Both changed, Drive newer: keep both.
        assert_eq!(merge(100, 5000, 120, 9000, false, false), Merge::KeepBoth);
        // Local newer: keep it (it gets uploaded).
        assert_eq!(merge(150, 9000, 120, 5000, false, false), Merge::Keep);
    }

    #[test]
    fn conflict_copies_and_resume_offsets() {
        assert_eq!(conflict_path(Path::new("a/b/chat.jsonl"), "OFFICE PC"), PathBuf::from("a/b/chat.from-OFFICE-PC.jsonl"));
        assert_eq!(conflict_path(Path::new("a/opencode.db"), "Laptop"), PathBuf::from("a/opencode.from-Laptop.db"));
        assert_eq!(next_byte(Some("bytes=0-524287")), 524288);
        assert_eq!(next_byte(None), 0);
    }
}
