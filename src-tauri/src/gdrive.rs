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
use std::collections::HashMap;
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
static CANCEL_SIGN_IN: AtomicBool = AtomicBool::new(false);
static TOKEN: Mutex<Option<(String, Instant)>> = Mutex::new(None);
static STATE_LOCK: Mutex<()> = Mutex::new(());

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
    pub uploaded: usize,
    pub downloaded: usize,
    pub conflicts: usize,
    pub failed: usize,
    pub unchanged: usize,
    pub bytes_up: u64,
    pub bytes_down: u64,
    pub restored: usize,
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

fn this_pc_name() -> String {
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
    let r = http()?.post(TOKEN_URL).form(&form).send().map_err(|e| net_err(&e))?;
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
    *TOKEN.lock().unwrap_or_else(|e| e.into_inner()) = None;
    if let Ok(k) = keyring() {
        let _ = k.delete_credential();
    }
    update(|s| {
        s.connected = false;
        s.email = None;
        s.name = None;
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
    let page = format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>{title}</title>\
         <style>html{{color-scheme:light dark}}body{{font-family:system-ui,sans-serif;display:grid;place-items:center;min-height:100vh;margin:0;background:#0a0a0a;color:#e4e4e7}}\
         main{{max-width:28rem;padding:2rem;text-align:center}}h1{{font-size:1.25rem;font-weight:600}}p{{color:#a1a1aa;line-height:1.5}}</style></head>\
         <body><main><h1>{title}</h1><p>{body}</p></main></body></html>"
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
        busy: BUSY.load(Ordering::SeqCst),
        devices: if connected { s.devices } else { Vec::new() },
    }
}

pub fn set_auto(on: bool) {
    update(|s| s.auto_sync = Some(on));
}

pub fn cancel_sign_in() {
    CANCEL_SIGN_IN.store(true, Ordering::SeqCst);
}

/// Opens Google's sign-in page in the browser and waits for the answer on 127.0.0.1. Only `drive.appdata` is asked for.
pub fn connect(app: &AppHandle) -> Result<Status, String> {
    use tauri_plugin_opener::OpenerExt;
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
                        reply(stream, "200 OK", "ChatVault is connected", "You can close this tab and go back to ChatVault.");
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
    Ok(status())
}

/// Signs ChatVault out of Google Drive: the access is revoked at Google and the saved sign-in removed from this PC.
/// With `delete_backup`, every ChatVault file in the Drive app folder (from every PC) is deleted first.
pub fn disconnect(delete_backup: bool) -> Result<Status, String> {
    if BUSY.load(Ordering::SeqCst) {
        return Err("Wait for the Google Drive sync to finish first.".into());
    }
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
    id: String,
    /// Path inside the backup folder, with `/` between folders.
    rel: String,
    device: String,
    size: i64,
    mtime: i64,
    zipped: bool,
}

impl Remote {
    fn version(&self) -> String {
        format!("{}:{}:{}", self.id, self.size, self.mtime)
    }
}

fn list_all() -> Result<Vec<Value>, String> {
    let mut out = Vec::new();
    let mut page: Option<String> = None;
    loop {
        let p = page.clone();
        let r: Value = send(move |_| {
            let mut q = vec![
                ("spaces", "appDataFolder".to_string()),
                ("pageSize", "1000".to_string()),
                ("fields", "nextPageToken,files(id,name,appProperties)".to_string()),
            ];
            if let Some(t) = &p {
                q.push(("pageToken", t.clone()));
            }
            hc().get(FILES_URL).query(&q)
        })?
        .json()
        .map_err(|_| "Google Drive did not answer as expected.")?;
        if let Some(files) = r["files"].as_array() {
            out.extend(files.iter().cloned());
        }
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

fn local_files(root: &Path) -> Vec<(String, PathBuf, i64, i64)> {
    let mut files = Vec::new();
    crate::sync::walk_all(root, &mut files);
    files
        .into_iter()
        .filter_map(|f| {
            let rel = f.strip_prefix(root).ok()?.components().map(|c| c.as_os_str().to_string_lossy().to_string()).collect::<Vec<_>>().join("/");
            let name = f.file_name()?.to_string_lossy().to_string();
            if name.starts_with(".chatvault") || name.ends_with(".cvpart") || safe_rel(&rel).is_none() {
                return None;
            }
            let m = fs::metadata(&f).ok()?;
            let mtime = m.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_millis() as i64;
            Some((rel, f, m.len() as i64, mtime))
        })
        .collect()
}

fn upload(existing: Option<&str>, rel: &str, device: &str, path: &Path, size: i64, mtime: i64) -> Result<u64, String> {
    let raw = fs::read(path).map_err(|_| format!("Could not read {rel}"))?;
    let zipped = !rel.ends_with(".zst");
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
        send(|_| {
            let c = hc();
            let rb = match existing {
                Some(id) => c.patch(format!("{UPLOAD_URL}/{id}")),
                None => c.post(UPLOAD_URL),
            };
            rb.query(&[("uploadType", "multipart"), ("fields", "id")]).header("Content-Type", ctype.clone()).body(multipart.clone())
        })?;
        return Ok(sent);
    }
    // Resumable upload: start a session, then send the file in chunks. A chunk that fails is resumed from the byte
    // Google says it has, so an interrupted upload does not start over.
    let session = send(|_| {
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
    let total = body.len();
    let mut at = 0usize;
    let mut failures = 0;
    while at < total {
        let end = (at + CHUNK).min(total);
        let piece = body[at..end].to_vec();
        let range = format!("bytes {}-{}/{}", at, end - 1, total);
        let res = http()?.put(&session).header("Content-Range", range).body(piece).send();
        match res {
            Ok(r) if r.status().is_success() => return Ok(sent),
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
                        return Ok(sent);
                    }
                    if r.status().as_u16() == 308 {
                        at = next_byte(r.headers().get("range").and_then(|h| h.to_str().ok()));
                    }
                }
            }
        }
    }
    Ok(sent)
}

/// The next byte to send after a `Range: bytes=0-N` answer (no header means nothing has arrived yet).
pub(crate) fn next_byte(range: Option<&str>) -> usize {
    range.and_then(|r| r.rsplit('-').next()).and_then(|n| n.trim().parse::<usize>().ok()).map(|n| n + 1).unwrap_or(0)
}

fn download(r: &Remote) -> Result<Vec<u8>, String> {
    let bytes = send(|_| hc().get(format!("{FILES_URL}/{}", r.id)).query(&[("alt", "media")]))?
        .bytes()
        .map_err(|e| net_err(&e))?;
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
pub fn sync(app: &AppHandle, restore: bool) -> Result<Stats, String> {
    if client().is_none() {
        return Err("Google Drive is not available in this build of ChatVault.".into());
    }
    if refresh_token().is_none() {
        return Err("Google Drive is not connected.".into());
    }
    if BUSY.swap(true, Ordering::SeqCst) {
        return Err("A Google Drive sync is already running.".into());
    }
    let result = run_sync(app, restore);
    BUSY.store(false, Ordering::SeqCst);
    match &result {
        Ok(stats) => {
            let st = stats.clone();
            update(|s| {
                s.last_sync_ms = Some(now_ms());
                s.last_error = None;
                s.last_result = Some(st);
            });
        }
        Err(e) => {
            let e = e.clone();
            update(|s| s.last_error = Some(e));
        }
    }
    progress(app, "Done", 0, 0, "");
    result
}

fn run_sync(app: &AppHandle, restore: bool) -> Result<Stats, String> {
    progress(app, "Backing up this PC", 0, 0, "");
    // The local copy first, so the newest chats are the ones uploaded. "Already running" is fine: it is being done.
    let _ = crate::sync::backup(app);
    let root = backup_root().ok_or("No backup folder.")?;
    fs::create_dir_all(&root).map_err(|_| "Could not create the backup folder.")?;
    let me = load().device_id;
    // Keep the device id from the first load for good.
    update(|s| s.device_id = me.clone());

    progress(app, "Checking Google Drive", 0, 0, "");
    let all = list_all()?;
    let remote: Vec<Remote> = all.iter().filter_map(remote_of).collect();
    let mut stats = Stats::default();
    let mut state = load();

    // 1. Upload what changed on this PC.
    let mine: HashMap<&str, &Remote> = remote.iter().filter(|r| r.device == me).map(|r| (r.rel.as_str(), r)).collect();
    let local = local_files(&root);
    let todo: Vec<_> = local
        .iter()
        .filter(|(rel, _, size, mtime)| {
            let in_drive = mine.get(rel.as_str()).is_some_and(|r| r.size == *size && r.mtime == *mtime);
            // A file that came from another PC and was not changed here is already in Drive under that PC.
            let from_elsewhere = state.synced.get(rel) == Some(&(*size, *mtime)) && !mine.contains_key(rel.as_str());
            !(in_drive || from_elsewhere)
        })
        .collect();
    stats.unchanged = local.len() - todo.len();
    for (i, (rel, path, size, mtime)) in todo.iter().enumerate() {
        progress(app, "Uploading", i, todo.len(), rel);
        match upload(mine.get(rel.as_str()).map(|r| r.id.as_str()), rel, &me, path, *size, *mtime) {
            Ok(n) => {
                stats.uploaded += 1;
                stats.bytes_up += n;
                state.synced.insert(rel.clone(), (*size, *mtime));
            }
            Err(e) if e.contains("full") || e.contains("not connected") || e.contains("removed") => {
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

    // 2. Download what the other PCs (or this PC before a reinstall) have that is newer or missing here.
    let mut best: HashMap<&str, &Remote> = HashMap::new();
    for r in remote.iter().filter(|r| r.device != me || !root.join(safe_rel(&r.rel).unwrap_or_default()).exists()) {
        let better = match best.get(r.rel.as_str()) {
            None => true,
            Some(b) => (r.size, r.mtime) > (b.size, b.mtime),
        };
        if better {
            best.insert(r.rel.as_str(), r);
        }
    }
    let names = device_names(&all);
    let candidates: Vec<&Remote> = best.into_values().filter(|r| state.applied.get(&r.rel) != Some(&r.version())).collect();
    for (i, r) in candidates.iter().enumerate() {
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
    publish_device(&all, &me, local.len());
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

    if restore {
        progress(app, "Restoring into the tools", 0, 0, "");
        stats.restored = crate::sync::restore(app).map(|r| r.copied).unwrap_or(0);
    }
    Ok(stats)
}

/// Saves the sync bookkeeping without losing a setting changed meanwhile in the dialog.
fn save_merge(work: &State) {
    let synced = work.synced.clone();
    let applied = work.applied.clone();
    update(|s| {
        s.synced = synced;
        s.applied = applied;
    });
}

fn device_names(all: &[Value]) -> HashMap<String, String> {
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
    let _ = send(|_| {
        let c = hc();
        match existing {
            Some(id) => c.patch(format!("{FILES_URL}/{id}")).json(&json!({ "appProperties": props })),
            None => c.post(FILES_URL).json(&json!({ "name": format!("device-{me}.json"), "parents": ["appDataFolder"], "mimeType": "application/json", "appProperties": props })),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

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
