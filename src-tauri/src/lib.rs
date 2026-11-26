mod backuploc;
mod cursor;
mod cli;
mod codex_db;
mod index;
mod integrations;
mod others;
mod pageindex;
mod paths;
mod providers;
mod search;
mod gdrive;
mod backups;
mod sources;
mod sync;

use index::{ChatSummary, Overview, Page};
use paths::*;
use providers::{clip, is_readable, messages_for, Msg};
use serde::Serialize;
use serde_json::Value;
use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tauri::AppHandle;

#[derive(Serialize)]
struct SearchHit {
    id: String,
    snippet: String,
}

#[derive(Serialize)]
struct MsgPage {
    messages: Vec<Msg>,
    total: usize,
    start: usize,
}

/// Cursor and the other tools whose chats live in a database or in one JSON file, not in a transcript line by line.
fn is_virtual(p: &str, path: &str) -> bool {
    p == "cursor" || others::TOOLS.contains(&p) || (p == "codex" && path.starts_with("codex-db:"))
}

fn read_virtual(provider: &str, path: &str) -> Result<Vec<Msg>, String> {
    if provider == "codex" && path.starts_with("codex-db:") {
        codex_db::read(path)
    } else if provider == "cursor" {
        cursor::read(path)
    } else {
        others::read(path)
    }
}

fn provider_ok(p: &str) -> Result<(), String> {
    if p == "claude" || p == "codex" || p == "cursor" || others::TOOLS.contains(&p) {
        Ok(())
    } else {
        Err("Unknown provider".into())
    }
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// Lines worth parsing. Skipping the rest keeps very large transcripts fast.
pub(crate) fn relevant(provider: &str, line: &str) -> bool {
    if provider == "codex" {
        // Messages and steps, plus the notes Codex shows: compaction, a stopped turn, an error, voice transcripts.
        line.contains("\"response_item\"")
            || line.contains("\"type\":\"compacted\"")
            || line.contains("\"turn_aborted\"")
            || line.contains("\"type\":\"realtime_item\"")
            || (line.contains("\"type\":\"event_msg\"") && (line.contains("\"type\":\"error\"") || line.contains("\"type\":\"stream_error\"")))
    } else {
        // Messages typed while Claude is working are saved as "queued_command" attachments, not as user lines; notes
        // such as "Conversation compacted" are "system" lines; @-mentioned files are "file" attachments.
        line.contains("\"type\":\"user\"")
            || line.contains("\"type\":\"assistant\"")
            || line.contains("\"queued_command\"")
            || line.contains("\"type\":\"system\"")
            || line.contains("\"type\":\"pr-link\"")
            || line.contains("\"attachment\":{\"type\":\"file\"")
    }
}

fn clip_msg(mut m: Msg, max: usize) -> Msg {
    if (m.role != "user" || m.kind != "text") && m.text.chars().count() > max {
        m.text = clip(&m.text, max);
    }
    m
}

#[tauri::command]
async fn refresh_index(app: AppHandle) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || index::refresh(Some(&app)))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn query_chats(provider: String, account: Option<String>, q: String, offset: i64, limit: i64, sort: Option<String>, scope: Option<String>) -> Result<Page, String> {
    // "all" lists the chats of every tool together.
    if provider != "all" {
        provider_ok(&provider)?;
    }
    tauri::async_runtime::spawn_blocking(move || index::query(&provider, account, &q, offset, limit.clamp(1, 200), sort.as_deref().unwrap_or("recent"), scope.as_deref().unwrap_or("all")))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn provider_overview(provider: String) -> Result<Overview, String> {
    provider_ok(&provider)?;
    tauri::async_runtime::spawn_blocking(move || index::overview(&provider))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn overview_stats(days: i64) -> Result<index::Stats, String> {
    tauri::async_runtime::spawn_blocking(move || index::stats(days.clamp(1, 90)))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn chats_by_ids(provider: String, ids: Vec<String>) -> Result<Vec<ChatSummary>, String> {
    provider_ok(&provider)?;
    tauri::async_runtime::spawn_blocking(move || index::by_ids(&provider, &ids[..ids.len().min(300)]))
        .await
        .map_err(|e| e.to_string())?
}

/// A window of messages. `before` is a message index; without it the newest `limit` messages come back.
#[derive(serde::Serialize)]
struct OutlineTick {
    ts: String,
    text: String,
}

/// A light outline of the whole chat for the side timeline: only your messages, each cut short, and at most 240 of them.
/// Only the lines that hold your messages are parsed, and nothing is sent to the page except these few ticks.
#[tauri::command]
async fn chat_outline(provider: String, path: String) -> Result<Vec<OutlineTick>, String> {
    provider_ok(&provider)?;
    tauri::async_runtime::spawn_blocking(move || {
        if provider == "codex" && path.starts_with("codex-db:") {
            let users:Vec<_>=codex_db::read(&path)?.into_iter().filter(|m|m.role=="user" && m.kind=="text").collect();
            let step=users.len().div_ceil(240).max(1);
            return Ok(users.into_iter().step_by(step).map(|m|OutlineTick{ts:m.ts,text:clip(&m.text,200)}).collect());
        }
        if is_virtual(&provider, &path) {
            return Err("not supported".to_string());
        }
        let p = check_transcript(&path)?;
        let lines = open_lines(&p).ok_or("Cannot open that chat")?;
        let mut ticks: Vec<OutlineTick> = Vec::new();
        for line in lines {
            let user_line = if provider == "codex" {
                line.contains("\"response_item\"") && line.contains("\"role\":\"user\"")
            } else {
                line.contains("\"type\":\"user\"") || line.contains("\"queued_command\"")
            };
            if !user_line {
                continue;
            }
            let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
            for m in messages_for(&provider, &v) {
                if m.role == "user" && m.kind == "text" && !m.text.trim().is_empty() {
                    ticks.push(OutlineTick { ts: m.ts.clone(), text: clip(m.text.trim(), 140) });
                }
            }
        }
        if ticks.len() > 240 {
            let n = ticks.len();
            let keep: Vec<usize> = (0..240).map(|k| k * (n - 1) / 239).collect();
            ticks = keep.into_iter().map(|i| OutlineTick { ts: ticks[i].ts.clone(), text: ticks[i].text.clone() }).collect();
        }
        Ok(ticks)
    })
    .await
    .map_err(|e| e.to_string())?
}

fn new_session_id() -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    let mut b = [0u8; 16];
    for half in 0..2 {
        let mut h = RandomState::new().build_hasher();
        h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
        h.write_u32(std::process::id());
        b[half * 8..half * 8 + 8].copy_from_slice(&h.finish().to_le_bytes());
    }
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let x: Vec<String> = b.iter().map(|v| format!("{v:02x}")).collect();
    format!("{}-{}-{}-{}-{}", x[0..4].concat(), x[4..6].concat(), x[6..8].concat(), x[8..10].concat(), x[10..16].concat())
}

#[derive(serde::Serialize)]
struct Forked {
    id: String,
    path: String,
}

/// Makes a copy of a chat with its own session id, next to the original, so it shows up as a new chat at once.
/// Nothing is opened and the original file is never touched.
#[tauri::command]
async fn fork_chat(provider: String, path: String) -> Result<Forked, String> {
    provider_ok(&provider)?;
    tauri::async_runtime::spawn_blocking(move || {
        use std::io::Write;
        if is_virtual(&provider, &path) {
            return Err("This chat is stored in its tool's database. Use the tool's fork command or Continue in Claude or Codex.".to_string());
        }
        let src = check_transcript(&path)?;
        let stem = src.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
        let stem = stem.strip_suffix(".jsonl").unwrap_or(&stem).to_string();
        let dir = src.parent().ok_or("Cannot find the chat folder")?.to_path_buf();
        let new_id = new_session_id();
        // Claude names the file by its id; Codex puts the id at the end of "rollout-<time>-<id>".
        let (old_id, new_name) = if provider == "codex" {
            if stem.len() < 36 {
                return Err("Cannot read that Codex chat id".to_string());
            }
            let (head, id) = stem.split_at(stem.len() - 36);
            (id.to_string(), format!("{head}{new_id}.jsonl"))
        } else {
            (stem.clone(), format!("{new_id}.jsonl"))
        };
        let dest = dir.join(&new_name);
        let lines = open_lines(&src).ok_or("Cannot open that chat")?;
        let tmp = dir.join(format!("{new_name}.tmp"));
        {
            let mut w = std::io::BufWriter::new(std::fs::File::create(&tmp).map_err(|e| e.to_string())?);
            for line in lines {
                let out = if line.contains(&old_id) { line.replace(&old_id, &new_id) } else { line };
                w.write_all(out.as_bytes()).map_err(|e| e.to_string())?;
                w.write_all(b"\n").map_err(|e| e.to_string())?;
            }
            w.flush().map_err(|e| e.to_string())?;
        }
        std::fs::rename(&tmp, &dest).map_err(|e| e.to_string())?;
        Ok(Forked { id: new_id, path: dest.to_string_lossy().to_string() })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// (file count, newest change) across the chat files of every tool, for spotting new conversations as they start.
#[tauri::command]
fn files_fingerprint() -> (usize, i64) {
    index::files_fingerprint()
}

/// Changes whenever Cursor writes to its chat storage, so the list can follow a Cursor chat as it happens.
#[tauri::command]
fn cursor_fingerprint() -> i64 {
    cursor::fingerprint()
}

struct Started {
    child: std::process::Child,
    err_path: std::path::PathBuf,
}

/// Agents that ChatVault started in the background, by "provider:id", so they can be stopped and their state shown.
static RUNNING: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, Started>>> = std::sync::OnceLock::new();
/// What an agent printed to its error stream when it ended badly, kept until the page asks for it.
static LAST_ERR: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, String>>> = std::sync::OnceLock::new();

fn running() -> &'static std::sync::Mutex<std::collections::HashMap<String, Started>> {
    RUNNING.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}
fn last_err() -> &'static std::sync::Mutex<std::collections::HashMap<String, String>> {
    LAST_ERR.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// The newest installed Cursor agent: its own node and script, run directly so the message never passes through a shell.
fn cursor_agent_entry() -> Option<(std::path::PathBuf, std::path::PathBuf)> {
    let root = std::path::PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("cursor-agent").join("versions");
    let mut dirs: Vec<_> = std::fs::read_dir(root).ok()?.flatten().filter(|e| e.path().join("index.js").exists() && e.path().join("node.exe").exists()).collect();
    dirs.sort_by_key(|e| e.file_name());
    let d = dirs.pop()?.path();
    Some((d.join("node.exe"), d.join("index.js")))
}

fn finish(map: &mut std::collections::HashMap<String, Started>, key: &str) {
    if let Some(mut st) = map.remove(key) {
        let ok = st.child.try_wait().ok().flatten().map(|s| s.success()).unwrap_or(true);
        if !ok {
            if let Ok(txt) = std::fs::read_to_string(&st.err_path) {
                let t = txt.trim();
                if !t.is_empty() {
                    if let Ok(mut e) = last_err().lock() {
                        e.insert(key.to_string(), t.lines().last().unwrap_or(t).chars().take(300).collect());
                    }
                }
            }
        }
        let _ = std::fs::remove_file(&st.err_path);
    }
}

/// Sends a message into an existing Claude Code, Codex or Cursor session, with no terminal. The agent runs hidden in the chat's
/// own folder and appends to the same session, so the reply shows up live here and in the other app. It runs with full access.
#[tauri::command]
fn send_message(provider: String, id: String, cwd: Option<String>, message: String) -> Result<(), String> {
    use std::io::Write;
    use std::os::windows::process::CommandExt;
    use std::process::Stdio;
    provider_ok(&provider)?;
    if message.trim().is_empty() {
        return Err("Write a message first.".into());
    }
    let key = format!("{provider}:{id}");
    let mut map = running().lock().map_err(|_| "busy".to_string())?;
    if let Some(st) = map.get_mut(&key) {
        if matches!(st.child.try_wait(), Ok(None)) {
            return Err("The agent is still working on the previous message.".into());
        }
        finish(&mut map, &key);
    }
    if let Ok(mut e) = last_err().lock() {
        e.remove(&key);
    }
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let dir = cwd.filter(|d| Path::new(d).is_dir());
    let (mut cmd, name, via_stdin) = match provider.as_str() {
        "codex" => {
            let mut c = Command::new("codex");
            c.args(["exec", "resume", &id, "--dangerously-bypass-approvals-and-sandbox", "-"]);
            (c, "Codex", true)
        }
        "cursor" => {
            let (node, entry) = cursor_agent_entry().ok_or("Cursor's agent is not installed. Install it from cursor.com/cli.")?;
            let chat_id = id.rsplit(['\\', '/', ':']).next().unwrap_or(&id).to_string();
            let mut c = Command::new(node);
            c.arg(entry).args(["-p", "--resume", &chat_id, "--force", "--trust"]);
            if let Some(d) = &dir {
                c.arg("--workspace").arg(d);
            }
            c.arg(&message);
            c.env("CURSOR_INVOKED_AS", "cursor-agent.cmd");
            (c, "Cursor", false)
        }
        _ => {
            let mut c = Command::new("claude");
            c.args(["-p", "--resume", &id, "--dangerously-skip-permissions"]);
            (c, "Claude Code", true)
        }
    };
    if let Some(d) = &dir {
        cmd.current_dir(d);
    }
    let err_path = std::env::temp_dir().join(format!("chatvault-agent-{}.err", key.replace(':', "_")));
    let err_file = std::fs::File::create(&err_path).map_err(|e| e.to_string())?;
    let mut child = cmd
        .stdin(if via_stdin { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::null())
        .stderr(Stdio::from(err_file))
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|e| format!("Could not start {name}: {e}. Is it installed and on your PATH?"))?;
    if via_stdin {
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(message.as_bytes());
        }
    }
    map.insert(key, Started { child, err_path });
    Ok(())
}

/// Starts the tool's own sign-in (it opens your browser), so messages sent from ChatVault are allowed to run.
#[tauri::command]
fn agent_login(provider: String) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    use std::process::Stdio;
    provider_ok(&provider)?;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let mut cmd = match provider.as_str() {
        "codex" => {
            let mut c = Command::new("codex");
            c.arg("login");
            c
        }
        "cursor" => {
            let (node, entry) = cursor_agent_entry().ok_or("Cursor's agent is not installed. Install it from cursor.com/cli.")?;
            let mut c = Command::new(node);
            c.arg(entry).arg("login");
            c.env("CURSOR_INVOKED_AS", "cursor-agent.cmd");
            c
        }
        _ => {
            let mut c = Command::new("claude");
            c.args(["auth", "login"]);
            c
        }
    };
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).creation_flags(CREATE_NO_WINDOW).spawn().map_err(|e| format!("Could not start the sign-in: {e}"))?;
    Ok(())
}

/// Whether an agent started from ChatVault is still working on this chat.
#[tauri::command]
fn agent_running(provider: String, id: String) -> bool {
    let key = format!("{provider}:{id}");
    let Ok(mut map) = running().lock() else { return false };
    let alive = map.get_mut(&key).map(|st| matches!(st.child.try_wait(), Ok(None)));
    match alive {
        Some(true) => true,
        Some(false) => {
            finish(&mut map, &key);
            false
        }
        None => false,
    }
}

/// The last error an agent from ChatVault ended with for this chat, once.
#[tauri::command]
fn take_agent_error(provider: String, id: String) -> Option<String> {
    last_err().lock().ok()?.remove(&format!("{provider}:{id}"))
}

/// Stops the agent that ChatVault started for this chat.
#[tauri::command]
fn stop_agent(provider: String, id: String) -> bool {
    let key = format!("{provider}:{id}");
    let Ok(mut map) = running().lock() else { return false };
    if let Some(mut st) = map.remove(&key) {
        let _ = st.child.kill();
        let _ = st.child.wait();
        let _ = std::fs::remove_file(&st.err_path);
        return true;
    }
    false
}

/// When each listed chat file last changed (0 when unknown), so the list can show which chats an agent is writing to.
#[tauri::command]
fn chat_stamps(items: Vec<(String, String)>) -> Vec<u64> {
    items
        .into_iter()
        .map(|(provider, path)| {
            if provider == "codex" && path.starts_with("codex-db:") { return codex_db::stamp(&path).map(|s|s.1).unwrap_or(0); }
            if is_virtual(&provider, &path) {
                return 0;
            }
            let Ok(p) = check_transcript(&path) else { return 0 };
            std::fs::metadata(&p)
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0)
        })
        .collect()
}

/// Size and modified time of a chat file, so the open chat can be checked for new messages without reading it.
#[tauri::command]
fn chat_stamp(provider: String, path: String) -> Result<(u64, u64), String> {
    provider_ok(&provider)?;
    if provider=="codex" && path.starts_with("codex-db:") {
        return codex_db::stamp(&path);
    }
    let p = check_transcript(&path)?;
    let md = std::fs::metadata(&p).map_err(|e| e.to_string())?;
    let ms = md.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_millis() as u64).unwrap_or(0);
    Ok((md.len(), ms))
}

/// The newest messages of a chat, read from the end of the file, so a chat can show its latest messages at once
/// while the full page (with the total count) is still being worked out.
#[tauri::command]
async fn read_chat_tail(provider: String, path: String, limit: usize) -> Result<Vec<Msg>, String> {
    provider_ok(&provider)?;
    tauri::async_runtime::spawn_blocking(move || {
        use std::io::{Read, Seek, SeekFrom};
        if provider == "codex" && path.starts_with("codex-db:") { return codex_db::page(&path,None,limit.clamp(1,400)).map(|p|p.0); }
        if is_virtual(&provider, &path) {
            return Err("not supported".to_string());
        }
        let p = check_transcript(&path)?;
        if p.extension().map(|e| e == "zst").unwrap_or(false) {
            return Err("not supported".to_string());
        }
        let limit = limit.clamp(1, 400);
        let mut f = std::fs::File::open(&p).map_err(|e| e.to_string())?;
        let len = f.metadata().map_err(|e| e.to_string())?.len();
        let mut pos = len;
        let mut buf: Vec<u8> = Vec::new();
        let chunk: u64 = 512 * 1024;
        loop {
            let take = chunk.min(pos);
            pos -= take;
            f.seek(SeekFrom::Start(pos)).map_err(|e| e.to_string())?;
            let mut part = vec![0u8; take as usize];
            f.read_exact(&mut part).map_err(|e| e.to_string())?;
            part.extend_from_slice(&buf);
            buf = part;
            let text = String::from_utf8_lossy(&buf);
            // Unless this reaches the start of the file, the first line may be cut in half.
            let from = if pos > 0 { text.find('\n').map(|i| i + 1).unwrap_or(text.len()) } else { 0 };
            let mut out: Vec<Msg> = Vec::new();
            for line in text[from..].lines() {
                if !relevant(&provider, line) {
                    continue;
                }
                let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
                for m in messages_for(&provider, &v) {
                    out.push(clip_msg(m, 30_000));
                }
            }
            if out.len() >= limit || pos == 0 {
                let skip = out.len().saturating_sub(limit);
                return Ok(out.split_off(skip));
            }
        }
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn read_chat_page(provider: String, path: String, before: Option<usize>, limit: usize) -> Result<MsgPage, String> {
    provider_ok(&provider)?;
    tauri::async_runtime::spawn_blocking(move || {
        if provider == "codex" && path.starts_with("codex-db:") {
            let (messages,total,start) = codex_db::page(&path,before,limit.clamp(1,2000))?;
            return Ok(MsgPage { messages:messages.into_iter().map(|m|clip_msg(m,30_000)).collect(),total,start });
        }
        if is_virtual(&provider, &path) {
            let all: Vec<Msg> = read_virtual(&provider, &path)?.into_iter().map(|m| clip_msg(m, 30_000)).collect();
            let total = all.len();
            let limit = limit.clamp(1, 2000);
            let end = before.map(|b| b.min(total)).unwrap_or(total);
            let start = end.saturating_sub(limit);
            return Ok(MsgPage { messages: all[start..end].to_vec(), total, start });
        }
        let p = check_transcript(&path)?;
        let limit = limit.clamp(1, 2000);
        // A plain transcript is indexed once, so a page only reads its own lines. A compressed one is read through.
        if p.extension().map(|e| e != "zst").unwrap_or(true) {
            let index = pageindex::get(&provider, &p)?;
            let total = index.total();
            return Ok(match before {
                None => {
                    let start = total.saturating_sub(limit);
                    MsgPage { messages: pageindex::read_range(&provider, &p, &index, start, total)?, total, start }
                }
                Some(b) => {
                    let lo = b.saturating_sub(limit);
                    MsgPage { messages: pageindex::read_range(&provider, &p, &index, lo, b)?, total, start: lo }
                }
            });
        }
        let lines = open_lines(&p).ok_or("Cannot open that chat")?;
        let mut idx = 0usize;
        let mut ring: VecDeque<Msg> = VecDeque::new();
        let mut picked: Vec<Msg> = Vec::new();
        let lo = before.map(|b| b.saturating_sub(limit)).unwrap_or(0);
        for line in lines {
            if !relevant(&provider, &line) {
                continue;
            }
            let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
            for m in messages_for(&provider, &v) {
                let m = clip_msg(m, 30_000);
                match before {
                    None => {
                        ring.push_back(m);
                        if ring.len() > limit {
                            ring.pop_front();
                        }
                    }
                    Some(b) => {
                        if idx >= lo && idx < b {
                            picked.push(m);
                        }
                    }
                }
                idx += 1;
            }
        }
        Ok(match before {
            None => MsgPage { start: idx - ring.len(), total: idx, messages: ring.into_iter().collect() },
            Some(_) => MsgPage { start: lo, total: idx, messages: picked },
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Every readable message (no tool steps), used for exports and handoff files.
#[tauri::command]
async fn read_chat_all(provider: String, path: String) -> Result<Vec<Msg>, String> {
    provider_ok(&provider)?;
    tauri::async_runtime::spawn_blocking(move || {
        if is_virtual(&provider, &path) {
            return Ok(read_virtual(&provider, &path)?.into_iter().filter(is_readable).map(|m| clip_msg(m, 20_000)).collect());
        }
        let p = check_transcript(&path)?;
        let lines = open_lines(&p).ok_or("Cannot open that chat")?;
        let mut out = Vec::new();
        for line in lines {
            if !relevant(&provider, &line) {
                continue;
            }
            if let Ok(v) = serde_json::from_str::<Value>(&line) {
                out.extend(messages_for(&provider, &v).into_iter().filter(is_readable).map(|m| clip_msg(m, 20_000)));
            }
        }
        Ok(out)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Searches inside every chat of one tool using the full-text index, so it answers at once.
#[tauri::command]
async fn deep_search(provider: String, query: String) -> Result<Vec<SearchHit>, String> {
    provider_ok(&provider)?;
    tauri::async_runtime::spawn_blocking(move || {
        if query.trim().chars().count() < 2 {
            return Ok(Vec::new());
        }
        Ok(index::search_bodies(&provider, &query, 300)?.into_iter().map(|(id, snippet)| SearchHit { id, snippet: snippet.replace('\n', " ") }).collect())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Searches every chat at once, best matches first (see search.rs for how a query is read and ranked).
/// `renamed` is the list of names given to chats inside ChatVault, so they can be found by those names too.
#[tauri::command]
async fn search_chats(
    query: String,
    provider: Option<String>,
    renamed: Option<std::collections::HashMap<String, String>>,
    include_archived: Option<bool>,
    limit: Option<usize>,
) -> Result<Vec<search::Found>, String> {
    if let Some(p) = provider.as_deref().filter(|p| *p != "all") {
        provider_ok(p)?;
    }
    tauri::async_runtime::spawn_blocking(move || {
        if query.trim().chars().count() < 2 {
            return Ok(Vec::new());
        }
        search::search(&query, provider.as_deref(), &renamed.unwrap_or_default(), include_archived.unwrap_or(true), limit.unwrap_or(200).clamp(1, 500))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Builds or updates the search index in the background.
#[tauri::command]
async fn build_search_index(app: AppHandle) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || index::index_bodies(Some(&app)))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
fn search_index_status() -> Result<(i64, i64), String> {
    index::body_status()
}

#[tauri::command]
fn reveal_in_folder(path: String) -> Result<(), String> {
    let p = if path.starts_with("codex-db:") {
        codex_db::real_file(&path)?
    } else if path.starts_with("cursor-") {
        cursor::real_file(&path).ok_or("That Cursor file is not available")?
    } else if others::is_virtual(&path) {
        others::real_file(&path).ok_or("That file is not available")?
    } else {
        check_transcript(&path)?
    };
    let p = p.to_string_lossy().trim_start_matches(r"\\?\").to_string();
    Command::new("explorer").arg(format!("/select,{}", p)).spawn().map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn open_folder(path: String) -> Result<(), String> {
    if !Path::new(&path).is_dir() {
        return Err("That folder no longer exists".into());
    }
    Command::new("explorer").arg(&path).spawn().map_err(|e| e.to_string())?;
    Ok(())
}

/// Sends a file or a folder to the Recycle Bin, so it can still be restored from there.
fn recycle(target: &Path) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    let plain = target.to_string_lossy().trim_start_matches(r"\\?\").to_string();
    let script = if target.is_dir() {
        "Add-Type -AssemblyName Microsoft.VisualBasic; [Microsoft.VisualBasic.FileIO.FileSystem]::DeleteDirectory($env:CHATVAULT_PATH, 'OnlyErrorDialogs', 'SendToRecycleBin')"
    } else {
        "Add-Type -AssemblyName Microsoft.VisualBasic; [Microsoft.VisualBasic.FileIO.FileSystem]::DeleteFile($env:CHATVAULT_PATH, 'OnlyErrorDialogs', 'SendToRecycleBin')"
    };
    let status = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .env("CHATVAULT_PATH", &plain)
        .creation_flags(0x0800_0000)
        .status()
        .map_err(|e| e.to_string())?;
    if !status.success() || Path::new(&plain).exists() {
        return Err("It could not be moved to the Recycle Bin".into());
    }
    Ok(())
}

/// Moves a chat transcript to the Recycle Bin, so it can still be restored from there.
#[tauri::command]
async fn delete_chat(path: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        // OpenCode keeps its sessions in its own database, so the session is deleted by OpenCode itself and
        // ChatVault never edits that database. Unlike a file, this cannot be restored from the Recycle Bin.
        if let Some(rest) = path.strip_prefix("opencode:") {
            let id = rest.rsplit_once('|').map(|(_, id)| id).unwrap_or(rest);
            if id.is_empty() || id.len() > 80 || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
                return Err("Invalid chat id".to_string());
            }
            let status = cli::status("opencode")?;
            if !status.ready {
                return Err(status.error.unwrap_or_else(|| "OpenCode is not installed".to_string()));
            }
            let exe = status.path.ok_or("OpenCode path is missing")?;
            use std::os::windows::process::CommandExt;
            let out = Command::new(exe)
                .args(["session", "delete", id])
                .stdin(std::process::Stdio::null())
                .creation_flags(0x0800_0000)
                .output()
                .map_err(|e| format!("Could not run OpenCode: {e}"))?;
            if !out.status.success() {
                let why = String::from_utf8_lossy(&out.stderr);
                let why = why.lines().find(|l| !l.trim().is_empty()).unwrap_or("it returned an error");
                return Err(format!("OpenCode could not delete the chat: {why}"));
            }
            let _ = index::forget(&path);
            return Ok(());
        }
        // Gemini, Cline and Cursor keep chats as files or folders: those go to the Recycle Bin like Claude and Codex.
        let target = if path.starts_with("gemini:") || path.starts_with("cline:") {
            others::deletable(&path)
        } else if path.starts_with("cursor-transcript:") || path.starts_with("cursor-cli:") {
            cursor::deletable(&path)
        } else {
            None
        };
        if let Some(target) = target {
            recycle(&target)?;
            let _ = index::forget(&path);
            return Ok(());
        }
        if others::is_virtual(&path) || path.starts_with("codex-db:") {
            return Err("This chat lives in the tool's own storage, so it can only be deleted there".to_string());
        }
        if path.starts_with("cursor-") {
            return Err("Cursor chats live inside Cursor's own database, so they can only be deleted in Cursor".to_string());
        }
        // A file that is already gone (deleted outside ChatVault) only needs to leave the list.
        if !Path::new(&path).exists() {
            index::forget(&path)?;
            return Ok(());
        }
        let p = check_transcript(&path)?;
        let plain = p.to_string_lossy().trim_start_matches(r"\\?\").to_string();
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let status = Command::new("powershell")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "Add-Type -AssemblyName Microsoft.VisualBasic; [Microsoft.VisualBasic.FileIO.FileSystem]::DeleteFile($env:CHATVAULT_PATH, 'OnlyErrorDialogs', 'SendToRecycleBin')",
            ])
            .env("CHATVAULT_PATH", &plain)
            .creation_flags(CREATE_NO_WINDOW)
            .status()
            .map_err(|e| e.to_string())?;
        if !status.success() || Path::new(&plain).exists() {
            return Err("The chat file could not be moved to the Recycle Bin".into());
        }
        index::forget(&path)?;
        index::forget(&plain)?;
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
fn open_in_cursor(path: String) -> Result<(), String> {
    if !Path::new(&path).is_dir() {
        return Err("That folder no longer exists".into());
    }
    Command::new("cursor").arg(&path).spawn().map_err(|_| "Cursor could not be started. Is it installed?".to_string())?;
    Ok(())
}

#[tauri::command]
fn open_backup_folder() -> Result<(), String> {
    let b = backup_root().ok_or("No Documents folder")?;
    fs::create_dir_all(&b).map_err(|e| e.to_string())?;
    Command::new("explorer").arg(&b).spawn().map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn open_in_claude(local_id: String) -> Result<(), String> {
    if !local_id.starts_with("local_") || !local_id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
        return Err("Invalid session id".into());
    }
    Command::new("cmd")
        .args(["/C", "start", "", &format!("claude://claude.ai/epitaxy/{}", local_id)])
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn set_chat_tags(provider: String, id: String, tags: Vec<String>) -> Result<(), String> {
    provider_ok(&provider)?;
    index::set_tags(&provider, &id, &tags)
}

#[tauri::command]
fn chat_tags(provider: String, id: String) -> Result<Vec<String>, String> {
    provider_ok(&provider)?;
    index::tags_of(&provider, &id)
}

#[tauri::command]
fn all_tags(provider: String) -> Result<Vec<(String, i64)>, String> {
    provider_ok(&provider)?;
    index::all_tags(&provider)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FileView {
    path: String,
    content: String,
    size: u64,
    truncated: bool,
    modified_ms: Option<i64>,
}

/// Looks for a file by name below `dir`, a few folders deep, skipping dependency and build folders.
fn find_named(dir: &Path, name: &std::ffi::OsStr, depth: u8, budget: &mut u32) -> Option<PathBuf> {
    let rd = fs::read_dir(dir).ok()?;
    let mut subs = Vec::new();
    for e in rd.flatten() {
        if *budget == 0 {
            return None;
        }
        *budget -= 1;
        let p = e.path();
        if p.is_dir() {
            let n = e.file_name().to_string_lossy().to_lowercase();
            if depth > 0 && !n.starts_with('.') && !["node_modules", "target", "dist", "build", "__pycache__", "vendor", "appdata"].contains(&n.as_str()) {
                subs.push(p);
            }
        } else if e.file_name().eq_ignore_ascii_case(name) {
            return Some(p);
        }
    }
    subs.into_iter().find_map(|d| find_named(&d, name, depth - 1, budget))
}

/// Finds the file a chat mentions. A relative path is looked up in the chat's project folder, then under the folders of full
/// paths the chat mentions. Only files inside the project folder or your user folder are ever opened.
fn resolve_file(path: &str, base: &Option<String>, hints: Option<Vec<String>>) -> Result<PathBuf, String> {
    let wanted = path.trim().trim_matches(|c| c == '"' || c == '\'' || c == '`').to_string();
    // a trailing :12 or :12-30 is a line reference
    let wanted = match wanted.rsplit_once(':') {
        Some((head, tail)) if head.len() > 1 && tail.chars().all(|c| c.is_ascii_digit() || c == '-') && !tail.is_empty() => head.to_string(),
        _ => wanted,
    };
    let raw = PathBuf::from(wanted.replace('/', "\\"));
    let basedir = base.as_ref().map(PathBuf::from).filter(|b| b.is_dir());
    let full = if raw.is_absolute() {
        raw
    } else {
        let mut found = basedir.as_ref().map(|b| b.join(&raw)).filter(|p| p.is_file());
        if found.is_none() {
            'hints: for h in hints.unwrap_or_default().iter().take(300) {
                for anc in Path::new(h).ancestors().skip(1) {
                    let cand = anc.join(&raw);
                    if cand.is_file() {
                        found = Some(cand);
                        break 'hints;
                    }
                }
            }
        }
        // A bare file name (no folder): look through the project and the usual places files are saved to.
        if found.is_none() && raw.components().count() == 1 {
            if let Some(name) = raw.file_name() {
                let mut budget = 20_000u32;
                let mut spots: Vec<(PathBuf, u8)> = Vec::new();
                if let Some(b) = basedir.as_ref() {
                    spots.push((b.clone(), 4));
                }
                if let Some(h) = home() {
                    for d in ["Downloads", "Desktop", "Documents", "Pictures", "Videos", "Music"] {
                        spots.push((h.join(d), 2));
                    }
                }
                found = spots.into_iter().find_map(|(d, depth)| find_named(&d, name, depth, &mut budget));
            }
        }
        match found {
            Some(f) => f,
            None if basedir.is_none() => return Err("This chat has no project folder to look in".into()),
            None => basedir.as_ref().unwrap().join(&raw),
        }
    };
    let canon = full.canonicalize().map_err(|_| format!("Could not find {} in the project or in your Downloads, Desktop, Documents or Pictures folders", wanted))?;
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Some(b) = basedir.as_ref().and_then(|b| b.canonicalize().ok()) {
        roots.push(b);
    }
    if let Some(h) = home().and_then(|h| h.canonicalize().ok()) {
        roots.push(h);
    }
    if !roots.iter().any(|r| canon.starts_with(r)) {
        return Err("Only files in the project folder or your user folder can be shown".into());
    }
    if !fs::metadata(&canon).map(|m| m.is_file()).unwrap_or(false) {
        return Err("That is a folder, not a file".into());
    }
    Ok(canon)
}

fn plain(p: &Path) -> String {
    p.to_string_lossy().trim_start_matches("\\?\\").to_string()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FileInfo {
    path: String,
    size: u64,
    /// text, image, video, audio, pdf or other
    kind: String,
    ext: String,
    modified_ms: Option<i64>,
}

/// What a file mentioned in a chat is, so the viewer can show it the right way.
#[tauri::command]
async fn file_info(path: String, base: Option<String>, hints: Option<Vec<String>>) -> Result<FileInfo, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let canon = resolve_file(&path, &base, hints)?;
        let meta = fs::metadata(&canon).map_err(|e| e.to_string())?;
        let ext = canon.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        let kind = match ext.as_str() {
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "bmp" | "ico" | "avif" => "image",
            "mp4" | "webm" | "mov" | "m4v" | "ogv" => "video",
            "mp3" | "wav" | "ogg" | "oga" | "m4a" | "aac" | "flac" | "opus" => "audio",
            "pdf" => "pdf",
            "zip" | "rar" | "7z" | "gz" | "tar" | "exe" | "dll" | "msi" | "iso" | "docx" | "xlsx" | "pptx" | "doc" | "xls" | "ppt" | "db" | "sqlite" | "bin" | "woff" | "woff2" | "ttf" | "otf" | "mkv" | "avi" | "wmv" => "other",
            _ => {
                // Unknown type: text if the start of the file has no binary bytes.
                let mut head = vec![0u8; 8000];
                let n = fs::File::open(&canon).and_then(|mut f| std::io::Read::read(&mut f, &mut head)).unwrap_or(0);
                if head[..n].contains(&0) { "other" } else { "text" }
            }
        };
        Ok(FileInfo { path: plain(&canon), size: meta.len(), kind: kind.into(), ext, modified_ms: meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_millis() as i64) })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// The raw bytes of a picture, video, sound or PDF, for showing it in the viewer. Up to 300 MB.
#[tauri::command]
async fn read_file_bytes(path: String, base: Option<String>) -> Result<tauri::ipc::Response, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let canon = resolve_file(&path, &base, None)?;
        let len = fs::metadata(&canon).map_err(|e| e.to_string())?.len();
        if len > 300_000_000 {
            return Err("This file is too large to show here".into());
        }
        let bytes = fs::read(&canon).map_err(|e| e.to_string())?;
        Ok(tauri::ipc::Response::new(bytes))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Shows a file from a chat selected in Explorer.
#[tauri::command]
fn reveal_file(path: String, base: Option<String>) -> Result<(), String> {
    let canon = resolve_file(&path, &base, None)?;
    Command::new("explorer").arg(format!("/select,{}", plain(&canon))).spawn().map_err(|e| e.to_string())?;
    Ok(())
}

/// Opens a file from a chat with the app Windows uses for it. Programs and scripts are never started this way.
#[tauri::command]
fn open_file_default(path: String, base: Option<String>) -> Result<(), String> {
    let canon = resolve_file(&path, &base, None)?;
    let ext = canon.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    const BLOCKED: [&str; 22] = ["exe", "com", "bat", "cmd", "ps1", "psm1", "vbs", "vbe", "js", "jse", "wsf", "msi", "msp", "scr", "lnk", "hta", "reg", "cpl", "dll", "jar", "pif", "appx"];
    if BLOCKED.contains(&ext.as_str()) {
        return Err("Programs and scripts are not opened from here. Use Show in folder instead.".into());
    }
    // Explorer opens the file with its default app and reads the name as plain text, so characters such as & or % in
    // a file name are never treated as commands, as they are by `cmd /C start`.
    Command::new("explorer").arg(plain(&canon)).spawn().map_err(|e| e.to_string())?;
    Ok(())
}

/// Reads a text file mentioned in a chat so it can be shown in the viewer: at most 1.5 MB, text only.
#[tauri::command]
async fn read_text_file(path: String, base: Option<String>, hints: Option<Vec<String>>) -> Result<FileView, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let canon = resolve_file(&path, &base, hints)?;
        let meta = fs::metadata(&canon).map_err(|e| e.to_string())?;
        let size = meta.len();
        // Only the first 1.5 MB is read, so a huge file named in a chat cannot fill the memory.
        let mut bytes = Vec::new();
        {
            use std::io::Read;
            fs::File::open(&canon).map_err(|e| e.to_string())?.take(1_500_001).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
        }
        let truncated = bytes.len() > 1_500_000;
        bytes.truncate(1_500_000);
        if bytes.iter().take(8000).any(|b| *b == 0) {
            return Err("This looks like a binary file, so it can't be shown as text".into());
        }
        let modified_ms = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_millis() as i64);
        Ok(FileView { path: plain(&canon), content: String::from_utf8_lossy(&bytes).to_string(), size, truncated, modified_ms })
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
fn set_chat_archived(provider: String, id: String, archived: bool) -> Result<(), String> {
    provider_ok(&provider)?;
    index::set_archived(&provider, &id, archived)
}

/// Saves a picture from a chat to the place the user picked in the Save dialog. Only picture file types are written.
#[tauri::command]
fn write_image_file(path: String, bytes: Vec<u8>) -> Result<String, String> {
    let p = PathBuf::from(&path);
    let ok = p.extension().map(|e| ["png", "jpg", "jpeg", "gif", "webp", "svg", "bmp"].iter().any(|x| e.eq_ignore_ascii_case(x))).unwrap_or(false);
    if !ok {
        return Err("Only picture files can be saved here".into());
    }
    fs::write(&p, bytes).map_err(|e| e.to_string())?;
    Ok(path)
}

/// True while a bulk export runs, so a second one cannot start on top of it.
static EXPORTING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ExportProgress {
    done: usize,
    total: usize,
    current: String,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ExportDone {
    written: usize,
    skipped: usize,
    folder: String,
    seconds: u64,
    error: Option<String>,
}

const EXPORT_TOOLS: [(&str, &str); 6] = [("claude", "Claude Code"), ("codex", "Codex"), ("cursor", "Cursor"), ("gemini", "Gemini CLI"), ("opencode", "OpenCode"), ("cline", "Cline")];

/// Reads one chat and returns its Markdown, or None when it has nothing readable. File names depend only on the chat, so
/// exporting again to the same folder replaces the earlier files instead of piling up copies.
fn chat_markdown(tool: &str, tool_name: &str, id: &str, path: &str) -> Option<(String, String)> {
    let msgs: Vec<Msg> = if is_virtual(tool, path) {
        read_virtual(tool, path).unwrap_or_default().into_iter().filter(is_readable).collect()
    } else {
        let lines = open_lines(Path::new(path))?;
        let mut out = Vec::new();
        for line in lines {
            if !relevant(tool, &line) {
                continue;
            }
            if let Ok(v) = serde_json::from_str::<Value>(&line) {
                out.extend(messages_for(tool, &v).into_iter().filter(is_readable));
            }
        }
        out
    };
    if msgs.is_empty() {
        return None;
    }
    let title = msgs.iter().find(|m| m.role == "user").map(|m| clip(m.text.lines().next().unwrap_or("Chat").trim(), 60)).unwrap_or_else(|| "Chat".into());
    let safe: String = title.chars().map(|c| if c.is_alphanumeric() || c == ' ' || c == '-' { c } else { '_' }).collect();
    let mut md = format!("# {}\n\n- Tool: {}\n- Chat id: {}\n\n", title, tool_name, id);
    for m in &msgs {
        let who = if m.role == "user" { "You" } else { "Assistant" };
        md.push_str(&format!("## {} {}\n\n{}\n\n", who, m.ts, m.text.trim()));
    }
    // A fixed fingerprint of the whole id: two chats with similar ids (Codex ids start alike) never share a file name.
    let short = id.bytes().fold(0x811c9dc5u32, |h, b| (h ^ b as u32).wrapping_mul(0x0100_0193));
    Some((format!("{}-{:08x}.md", safe.trim().trim_end_matches('…'), short), md))
}

/// Starts saving every chat as a Markdown file in the chosen folder, one subfolder per tool, and returns at once. The work
/// runs on its own thread and reports through `export-progress` and `export-done`, so the window never waits for it.
#[tauri::command]
fn export_all_markdown(app: AppHandle, folder: String) -> Result<(), String> {
    use std::sync::atomic::Ordering;
    use tauri::Emitter;
    let root = PathBuf::from(&folder);
    if !root.is_dir() {
        return Err("That folder does not exist".into());
    }
    if EXPORTING.swap(true, Ordering::SeqCst) {
        return Err("An export is already running".into());
    }
    std::thread::spawn(move || {
        let started = std::time::Instant::now();
        let run = || -> Result<(usize, usize), String> {
            let mut jobs: Vec<(&str, &str, String, String)> = Vec::new();
            for (tool, name) in EXPORT_TOOLS {
                for (id, path) in index::recent_paths(tool, 1_000_000)? {
                    jobs.push((tool, name, id, path));
                }
            }
            let total = jobs.len();
            let (mut written, mut skipped) = (0usize, 0usize);
            let mut last_emit = std::time::Instant::now() - std::time::Duration::from_secs(1);
            let _ = app.emit("export-progress", ExportProgress { done: 0, total, current: String::new() });
            for (i, (tool, name, id, path)) in jobs.iter().enumerate() {
                match chat_markdown(tool, name, id, path) {
                    Some((file, md)) => {
                        let dir = root.join(name);
                        let target = dir.join(&file);
                        let temp = dir.join(format!("{}.tmp", file));
                        // Written to a temporary file first, so an interrupted export never leaves a half file behind.
                        if fs::create_dir_all(&dir).is_ok() && fs::write(&temp, md).is_ok() && fs::rename(&temp, &target).is_ok() {
                            written += 1;
                        } else {
                            let _ = fs::remove_file(&temp);
                            skipped += 1;
                        }
                    }
                    None => skipped += 1,
                }
                if last_emit.elapsed() >= std::time::Duration::from_millis(200) || i + 1 == total {
                    last_emit = std::time::Instant::now();
                    let _ = app.emit("export-progress", ExportProgress { done: i + 1, total, current: clip(&file_stem_of(path), 60) });
                }
            }
            Ok((written, skipped))
        };
        let result = run();
        EXPORTING.store(false, Ordering::SeqCst);
        let seconds = started.elapsed().as_secs();
        let done = match result {
            Ok((written, skipped)) => ExportDone { written, skipped, folder, seconds, error: None },
            Err(e) => ExportDone { written: 0, skipped: 0, folder, seconds, error: Some(e) },
        };
        let _ = app.emit("export-done", done);
    });
    Ok(())
}

fn file_stem_of(path: &str) -> String {
    Path::new(path).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()
}

/// Whether a bulk export is running right now, so a reopened dialog can show it.
#[tauri::command]
fn export_running() -> bool {
    EXPORTING.load(std::sync::atomic::Ordering::SeqCst)
}

/// Writes a text file to a path the user picked in the system Save dialog. Only .md, .txt, .html and .json files are written.
#[tauri::command]
fn write_text_file(path: String, content: String) -> Result<String, String> {
    let p = PathBuf::from(&path);
    let ok = p
        .extension()
        .map(|e| ["md", "txt", "html", "json"].iter().any(|x| e.eq_ignore_ascii_case(x)))
        .unwrap_or(false);
    if !ok {
        return Err("Only .md, .txt, .html and .json files can be saved here".into());
    }
    fs::write(&p, content).map_err(|e| e.to_string())?;
    Ok(path)
}

#[tauri::command]
fn save_markdown(title: String, markdown: String) -> Result<String, String> {
    let dir = home().ok_or("No user profile")?.join("Desktop").join("Claude Chats");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let safe: String = title.chars().map(|c| if c.is_alphanumeric() || c == ' ' || c == '-' { c } else { '_' }).collect();
    let file = dir.join(format!("{}.md", clip(safe.trim(), 60).trim_end_matches('…')));
    fs::write(&file, markdown).map_err(|e| e.to_string())?;
    Command::new("explorer").arg(format!("/select,{}", file.to_string_lossy())).spawn().map_err(|e| e.to_string())?;
    Ok(file.to_string_lossy().to_string())
}

#[tauri::command]
fn save_handoff(title: String, content: String) -> Result<String, String> {
    let dir = home().ok_or("No user profile")?.join("Documents").join("ChatVault Handoffs");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let safe: String = title.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).collect();
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let file = dir.join(format!("{}-{}.md", clip(&safe, 40).trim_end_matches('…'), stamp));
    fs::write(&file, content).map_err(|e| e.to_string())?;
    Ok(file.to_string_lossy().to_string())
}

const HANDOFF_PROMPT: &str = "It is the transcript of an earlier AI coding session on this project. Continue the work from exactly where it stopped, and follow the project rules it mentions.";

/// PowerShell ends a single-quoted string at any of these, not only at the plain apostrophe.
fn is_ps_quote(c: char) -> bool {
    matches!(c, '\'' | '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}')
}

/// Text made safe to sit inside a single-quoted PowerShell string: every kind of quote is doubled, and line breaks
/// become spaces, so nothing typed or pasted can end the string and run as a command.
fn ps_single_quoted_inner(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if is_ps_quote(c) {
            out.push(c);
            out.push(c);
        } else if c == '\r' || c == '\n' {
            out.push(' ');
        } else {
            out.push(c);
        }
    }
    out
}

/// Opens a new PowerShell window in the chat's folder and starts Claude Code or Codex there.
/// agent: claude | claude-fork | claude-handoff | codex-resume | codex-fork | codex-handoff
#[tauri::command]
async fn launch_agent(agent: String, id: String, cwd: Option<String>, handoff: Option<String>, prompt: Option<String>) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || launch_agent_inner(agent, id, cwd, handoff, prompt)).await.map_err(|e| e.to_string())?
}

fn launch_agent_inner(agent: String, id: String, cwd: Option<String>, handoff: Option<String>, prompt: Option<String>) -> Result<(), String> {
    let dir = cwd
        .filter(|c| Path::new(c).is_dir())
        .map(PathBuf::from)
        .or_else(home)
        .ok_or("No folder to start in")?;
    let handoff_path = |h: Option<String>| -> Result<String, String> {
        let path = h.ok_or("Missing handoff file")?;
        if path.chars().any(|c| is_ps_quote(c) || c.is_control()) || !Path::new(&path).is_file() {
            return Err("Invalid handoff file".into());
        }
        Ok(path)
    };
    // The prompt ChatVault built (same text as "Copy prompt"), as the inside of a single-quoted PowerShell string.
    let text = |path: &str| -> String {
        match &prompt {
            Some(p) if !p.trim().is_empty() => ps_single_quoted_inner(p),
            _ => format!("Read the file {} completely. {}", path, HANDOFF_PROMPT),
        }
    };
    let script = match agent.as_str() {
        "claude" | "claude-fork" => {
            if !valid_id(&id) {
                return Err("Invalid chat id".into());
            }
            let fork = if agent == "claude-fork" { " --fork-session" } else { "" };
            format!("claude --resume {}{}", id, fork)
        }
        "claude-handoff" => {
            let path = handoff_path(handoff)?;
            let folder = Path::new(&path).parent().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
            format!("claude '{}' --add-dir '{}'", text(&path), folder)
        }
        "codex-resume" | "codex-fork" => {
            if !valid_id(&id) {
                return Err("Invalid chat id".into());
            }
            format!("codex {} {}", if agent == "codex-fork" { "fork" } else { "resume" }, id)
        }
        "codex-handoff" => {
            let path = handoff_path(handoff)?;
            format!("codex '{}'", text(&path))
        }
        "gemini-handoff" => {
            let path = handoff_path(handoff)?;
            let folder = Path::new(&path).parent().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
            format!("gemini --prompt-interactive '{}' --include-directories '{}'", text(&path), folder)
        }
        "opencode-handoff" => {
            let path = handoff_path(handoff)?;
            format!("opencode --prompt '{}'", text(&path))
        }
        "cline-handoff" => {
            let path = handoff_path(handoff)?;
            format!("cline --tui --auto-approve false '{}'", text(&path))
        }
        "cursor-handoff" => {
            let path = handoff_path(handoff)?;
            format!("cursor-agent '{}'", text(&path))
        }
        _ => return Err("Unknown agent".into()),
    };
    let tool = agent.split('-').next().ok_or("Unknown agent")?;
    let status = cli::status(tool)?;
    if !status.ready { return Err(status.error.unwrap_or_else(|| "CLI is not installed. Use Install in the Continue menu.".into())); }
    let path = status.path.ok_or("CLI path is missing")?;
    let (_, arguments) = script.split_once(' ').ok_or("Missing CLI arguments")?;
    // A script file avoids cmd.exe's short command-line limit for handoff prompts.
    let script_path = cli::script_file("")?;
    let receipt = script_path.with_extension("launch-error.txt");
    fs::write(&script_path, format!("\u{feff}{}", cli::launch_script(&path, arguments, &receipt.to_string_lossy()))).map_err(|e| e.to_string())?;
    use std::os::windows::process::CommandExt;
    // The terminal is opened through `cmd /C start`, which creates its console properly. Starting PowerShell directly
    // from this window-only app gave the new console no usable input or output, so full-screen tools such as
    // OpenCode ran but drew nothing, and could not be typed into. It also keeps the terminal independent of ChatVault.
    let shell = cli::powershell();
    Command::new("cmd")
        .arg("/C")
        .arg("start")
        .arg("")
        .arg(shell.get_program())
        .args(["-NoLogo", "-NoProfile", "-ExecutionPolicy", "Bypass", "-NoExit", "-File"])
        .arg(&script_path)
        .current_dir(dir)
        .creation_flags(0x0800_0000)
        .spawn()
        .map_err(|e| format!("Could not open terminal: {e}"))?;
    for _ in 0..15 {
        if let Ok(error) = fs::read_to_string(&receipt) { return Err(error); }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AccountInfo {
    id: String,
    orgs: usize,
    desktop_current: bool,
    cli_current: bool,
    email: Option<String>,
    name: Option<String>,
    plan: Option<String>,
}

/// What can be learned locally about each Claude account folder. Only the account the
/// Claude Code CLI is signed in to has an email on disk.
#[tauri::command]
fn account_info() -> Vec<AccountInfo> {
    let desktop_current = env_path("APPDATA")
        .map(|p| p.join("Claude").join("config.json"))
        .and_then(|p| fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .and_then(|v| v.get("lastKnownAccountUuid").and_then(|x| x.as_str()).map(String::from));
    let cli = home()
        .map(|p| p.join(".claude.json"))
        .and_then(|p| fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .and_then(|v| v.get("oauthAccount").cloned());
    let cli_id = cli.as_ref().and_then(|o| o.get("accountUuid")).and_then(|x| x.as_str()).map(String::from);
    let get = |k: &str| cli.as_ref().and_then(|o| o.get(k)).and_then(|x| x.as_str()).map(String::from);
    let Some(root) = claude_meta() else { return Vec::new() };
    subdirs(&root)
        .into_iter()
        .filter_map(|d| {
            let id = d.file_name()?.to_string_lossy().to_string();
            let is_cli = cli_id.as_deref() == Some(id.as_str());
            Some(AccountInfo {
                orgs: subdirs(&d).len(),
                desktop_current: desktop_current.as_deref() == Some(id.as_str()),
                cli_current: is_cli,
                email: if is_cli { get("emailAddress") } else { None },
                name: if is_cli { get("displayName") } else { None },
                plan: if is_cli { get("organizationType") } else { None },
                id,
            })
        })
        .collect()
}

#[tauri::command]
async fn sync_chats(app: AppHandle) -> Result<sync::SyncStats, String> {
    tauri::async_runtime::spawn_blocking(move || gdrive::exclusive(|| sync::backup(&app))).await.map_err(|e| e.to_string())?
}

#[tauri::command]
async fn restore_chats(app: AppHandle) -> Result<sync::SyncStats, String> {
    tauri::async_runtime::spawn_blocking(move || gdrive::exclusive(|| sync::restore(&app))).await.map_err(|e| e.to_string())?
}

#[tauri::command]
async fn chat_sources() -> Result<Vec<sources::Source>, String> {
    tauri::async_runtime::spawn_blocking(|| sources::all(true)).await.map_err(|e| e.to_string())
}

#[tauri::command]
fn open_chat_source(path: String) -> Result<(), String> {
    sources::open(&path)
}

#[tauri::command]
fn disabled_integrations() -> Vec<String> {
    integrations::disabled()
}

/// Switches a tool on or off. Off: its chats leave ChatVault's list and it is skipped by scans and backups. On: the list
/// is rebuilt on the next refresh. The tool's own chats are never touched.
#[tauri::command]
async fn set_integration(provider: String, enabled: bool) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        integrations::set(&provider, enabled)?;
        if enabled {
            index::mark_stale()
        } else {
            index::purge_provider(&provider)
        }
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
fn backup_location_info() -> backuploc::Info {
    backuploc::info()
}

#[tauri::command]
async fn check_backup_location(path: String) -> Result<backuploc::Check, String> {
    tauri::async_runtime::spawn_blocking(move || backuploc::check(&path)).await.map_err(|e| e.to_string())?
}

#[tauri::command]
async fn set_backup_location(app: AppHandle, path: String, move_files: bool) -> Result<backuploc::MoveResult, String> {
    tauri::async_runtime::spawn_blocking(move || gdrive::exclusive(|| backuploc::set(&app, &path, move_files))).await.map_err(|e| e.to_string())?
}

#[tauri::command]
fn cancel_backup_move() {
    backuploc::cancel();
}

#[tauri::command]
async fn delete_previous_backup() -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(backuploc::delete_previous).await.map_err(|e| e.to_string())?
}

// ---- Google Drive (see gdrive.rs). Every call runs off the main thread; errors are plain sentences, never tokens.

#[tauri::command]
async fn gdrive_status() -> Result<gdrive::Status, String> {
    tauri::async_runtime::spawn_blocking(gdrive::status).await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn gdrive_connect(app: AppHandle) -> Result<gdrive::Status, String> {
    tauri::async_runtime::spawn_blocking(move || gdrive::connect(&app)).await.map_err(|e| e.to_string())?
}

#[tauri::command]
fn gdrive_cancel_connect() {
    gdrive::cancel_sign_in();
}

#[tauri::command]
async fn gdrive_disconnect(delete_backup: bool) -> Result<gdrive::Status, String> {
    tauri::async_runtime::spawn_blocking(move || gdrive::disconnect(delete_backup)).await.map_err(|e| e.to_string())?
}

#[tauri::command]
async fn gdrive_sync(app: AppHandle, restore: bool, scope: Option<String>) -> Result<gdrive::Stats, String> {
    tauri::async_runtime::spawn_blocking(move || gdrive::sync(&app, restore, scope.as_deref())).await.map_err(|e| e.to_string())?
}

#[tauri::command]
async fn backup_entries(
    drive: bool, refresh: Option<bool>, stream: bool, scope: String,
    preview: tauri::ipc::Channel<Vec<backups::Entry>>,
) -> Result<Vec<backups::Entry>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        if gdrive::backup_scope().0 != scope {
            return Err("The backup source changed. Refresh the list to continue.".into());
        }
        if drive && refresh.unwrap_or(false) {
            gdrive::refresh_listing();
        }
        let rows = backups::entries(drive, stream.then_some(move |rows| { let _ = preview.send(rows); }))?;
        if gdrive::backup_scope().0 != scope {
            return Err("The backup source changed. Refresh the list to continue.".into());
        }
        Ok(rows)
    }).await.map_err(|e| e.to_string())?
}

type BackupDeleteJob = (String, std::sync::Arc<std::sync::atomic::AtomicBool>);
static BACKUP_JOBS: std::sync::Mutex<Vec<BackupDeleteJob>> = std::sync::Mutex::new(Vec::new());

#[tauri::command]
async fn backup_delete(
    app: AppHandle,
    units: Vec<backups::UnitRef>,
    local: bool,
    drive: bool,
    job_id: Option<String>,
    retry_ids: Option<Vec<String>>,
    scope: Option<String>,
) -> Result<gdrive::Removed, String> {
    use tauri::Emitter;
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    if let Some(id) = &job_id {
        register_backup_job(id, &stop)?;
    }
    let retry = retry_ids.map(|ids| ids.into_iter().collect());
    let progress_id = job_id.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let report = |done: usize, total: usize| {
            if let Some(id) = &progress_id {
                let _ = app.emit("backup-delete-progress", serde_json::json!({ "id": id, "done": done, "total": total }));
            }
        };
        report(0, 0);
        backups::delete(&units, local, drive, &stop, retry.as_ref(), scope.as_deref(), report)
    }).await;
    if let Some(id) = &job_id { unregister_backup_job(id); }
    result.map_err(|e| e.to_string())?
}

fn register_backup_job(id: &str, stop: &std::sync::Arc<std::sync::atomic::AtomicBool>) -> Result<(), String> {
    let mut active = BACKUP_JOBS.lock().unwrap_or_else(|e| e.into_inner());
    if active.iter().any(|(current, _)| current == id) {
        return Err("This backup job is already running.".into());
    }
    active.push((id.to_owned(), stop.clone()));
    Ok(())
}

fn unregister_backup_job(id: &str) {
    BACKUP_JOBS.lock().unwrap_or_else(|e| e.into_inner()).retain(|(current, _)| current != id);
}

#[tauri::command]
fn stop_backup_delete(job_id: String) {
    let active = BACKUP_JOBS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, stop)) = active.iter().find(|(id, _)| *id == job_id) {
        stop.store(true, std::sync::atomic::Ordering::Release);
    }
}

#[tauri::command]
async fn backup_upload(app: AppHandle, units: Vec<backups::UnitRef>, job_id: String, scope: String) -> Result<gdrive::Stats, String> {
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    register_backup_job(&job_id, &stop)?;
    let id = job_id.clone();
    let units = units.into_iter().map(|u| (u.provider, u.key)).collect();
    let result = tauri::async_runtime::spawn_blocking(move || gdrive::selected_upload(&app, &units, &id, &stop, &scope)).await;
    unregister_backup_job(&job_id);
    result.map_err(|e| e.to_string())?
}

#[tauri::command]
async fn backup_scope() -> Result<(String, bool), String> {
    tauri::async_runtime::spawn_blocking(gdrive::backup_scope).await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn backup_transfer(app: AppHandle, units: Vec<backups::UnitRef>, up: bool) -> Result<gdrive::Stats, String> {
    let set = units.into_iter().map(|u| (u.provider, u.key)).collect();
    tauri::async_runtime::spawn_blocking(move || gdrive::transfer(&app, &set, up)).await.map_err(|e| e.to_string())?
}

/// Checks with Google that the Drive connection still works, then returns the status.
#[tauri::command]
async fn gdrive_check() -> Result<gdrive::Status, String> {
    tauri::async_runtime::spawn_blocking(gdrive::check).await.map_err(|e| e.to_string())
}

#[tauri::command]
fn gdrive_cancel_sync() {
    gdrive::cancel_sync();
}

#[tauri::command]
fn gdrive_set_auto(on: bool) {
    gdrive::set_auto(on);
}

#[tauri::command]
async fn backup_status() -> Result<sync::BackupStatus, String> {
    tauri::async_runtime::spawn_blocking(sync::status).await.map_err(|e| e.to_string())
}

/// Brings the main window to the front: shown if it was closed to the tray, restored if minimized, then focused.
fn show_main(app: &tauri::AppHandle) {
    use tauri::Manager;
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

/// Set once the window has been shown for the first time after start.
static FIRST_SHOWN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The window starts hidden and the start-up screen calls this once it has painted "Getting your chats ready…",
/// so the first thing anyone sees is that screen, never an empty window.
#[tauri::command]
fn boot_ready(app: AppHandle) {
    if !FIRST_SHOWN.swap(true, std::sync::atomic::Ordering::SeqCst) {
        show_main(&app);
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // Only one ChatVault runs at a time. Starting it again (Start menu, shortcut, installer) brings the open one forward
        // instead of adding another copy and another tray icon. This plugin has to be registered first.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            FIRST_SHOWN.store(true, std::sync::atomic::Ordering::SeqCst);
            show_main(app);
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .setup(|app| {
            use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
            use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
            use tauri::Emitter;
            // Last-resort safety net: if the page never reports in and never finishes loading, show the window anyway.
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_secs(30));
                if !FIRST_SHOWN.swap(true, std::sync::atomic::Ordering::SeqCst) {
                    show_main(&handle);
                }
            });
            let open = MenuItem::with_id(app, "open", "Open ChatVault", true, None::<&str>)?;
            let search = MenuItem::with_id(app, "search", "Search chats", true, Some("Ctrl+K"))?;
            let sync = MenuItem::with_id(app, "sync", "Sync & backup…", true, None::<&str>)?;
            let rescan = MenuItem::with_id(app, "rescan", "Scan for new chats", true, None::<&str>)?;
            let settings = MenuItem::with_id(app, "settings", "Settings…", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit ChatVault", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&open, &search, &PredefinedMenuItem::separator(app)?, &sync, &rescan, &settings, &PredefinedMenuItem::separator(app)?, &quit])?;
            let mut tray = TrayIconBuilder::with_id("main")
                .tooltip("ChatVault")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, e| match e.id.as_ref() {
                    "open" => show_main(app),
                    "sync" => {
                        show_main(app);
                        let _ = app.emit("tray-sync", ());
                    }
                    "rescan" => {
                        let _ = app.emit("tray-rescan", ());
                    }
                    "search" => {
                        show_main(app);
                        let _ = app.emit("tray-search", ());
                    }
                    "settings" => {
                        show_main(app);
                        let _ = app.emit("tray-settings", ());
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, e| {
                    if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = e {
                        show_main(tray.app_handle());
                    }
                });
            if let Some(icon) = app.default_window_icon() {
                tray = tray.icon(icon.clone());
            }
            tray.build(app)?;
            Ok(())
        })
        // Safety net for the start-up screen: once the page has loaded it has painted, so the window can be shown even if
        // its call to boot_ready was lost. Showing it earlier (on a timer) could show an empty black window.
        .on_page_load(|webview, payload| {
            if payload.event() == tauri::webview::PageLoadEvent::Finished
                && webview.label() == "main"
                && !FIRST_SHOWN.swap(true, std::sync::atomic::Ordering::SeqCst)
            {
                show_main(tauri::Manager::app_handle(webview));
            }
        })
        // Closing the window keeps ChatVault running in the tray, so backups and live sync carry on. Quit from the tray menu.
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            boot_ready,
            refresh_index,
            query_chats,
            provider_overview,
            chats_by_ids,
            overview_stats,
            read_chat_page,
            read_chat_tail,
            chat_stamp,
            chat_stamps,
            chat_outline,
            cursor_fingerprint,
            files_fingerprint,
            send_message,
            write_text_file,
            export_all_markdown,
            export_running,
            write_image_file,
            set_chat_archived,
            read_text_file,
            file_info,
            read_file_bytes,
            reveal_file,
            open_file_default,
            set_chat_tags,
            chat_tags,
            all_tags,
            agent_running,
            take_agent_error,
            agent_login,
            stop_agent,
            fork_chat,
            read_chat_all,
            deep_search,
            search_chats,
            build_search_index,
            search_index_status,
            reveal_in_folder,
            open_folder,
            open_backup_folder,
            open_in_cursor,
            delete_chat,
            open_in_claude,
            save_markdown,
            save_handoff,
            launch_agent,
            cli::cli_status,
            cli::install_cli,
            cli::set_cli_path,
            account_info,
            sync_chats,
            restore_chats,
            backup_status,
            gdrive_status,
            gdrive_connect,
            gdrive_cancel_connect,
            gdrive_disconnect,
            gdrive_sync,
            gdrive_set_auto,
            gdrive_cancel_sync,
            backup_entries,
            backup_delete,
            stop_backup_delete,
            backup_transfer,
            backup_upload,
            backup_scope,
            gdrive_check,
            chat_sources,
            open_chat_source,
            backup_location_info,
            disabled_integrations,
            set_integration,
            check_backup_location,
            set_backup_location,
            cancel_backup_move,
            delete_previous_backup
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}


#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn keeps_full_user_text_for_pasted_attachments() {
        let text = format!("{}\nEnd of paste\n", "日".repeat(31_000));
        let input = Msg::new("user", "", text.clone(), "text");
        let reply = Msg::new("assistant", "", text.clone(), "text");
        assert_eq!(clip_msg(input, 30_000).text, text);
        assert_eq!(clip_msg(reply, 30_000).text, clip(&text, 30_000));
    }

    #[test]
    fn indexes_and_reads_real_chats() {
        let t = Instant::now();
        index::refresh(None).unwrap();
        println!("refresh: {:?}", t.elapsed());
        let t = Instant::now();
        index::refresh(None).unwrap();
        println!("second refresh (no changes): {:?}", t.elapsed());
        for prov in ["claude", "codex", "cursor"] {
            let o = index::overview(prov).unwrap();
            println!("{prov}: {} chats, accounts {:?}", o.total, o.accounts.iter().map(|a| (a.id.chars().take(8).collect::<String>(), a.count)).collect::<Vec<_>>());
            assert!(o.total > 0, "{prov} has chats");
            let t = Instant::now();
            let page = index::query(prov, None, "", 0, 20, "recent", "all").unwrap();
            println!("  page of {} in {:?}, first: {:?}", page.items.len(), t.elapsed(), page.items.first().map(|c| c.title.clone()));
            let page = index::query(prov, None, "fuel", 0, 5, "recent", "all").unwrap();
            println!("  search 'fuel': {} total", page.total);
        }
        // The biggest codex rollout must still page quickly.
        let big = index::query("codex", None, "", 0, 200, "recent", "all").unwrap().items.into_iter().max_by_key(|c| c.size).unwrap();
        println!("biggest codex chat: {} ({} MB)", big.title, big.size / 1_000_000);
        let t = Instant::now();
        let mut n = 0;
        let mut text = 0;
        for line in open_lines(Path::new(&big.path)).unwrap() {
            if !relevant("codex", &line) { continue; }
            if let Ok(v) = serde_json::from_str::<Value>(&line) {
                for m in messages_for("codex", &v) { n += 1; if m.kind == "text" { text += 1; } }
            }
        }
        println!("  parsed {} messages ({} text) in {:?}", n, text, t.elapsed());
        assert!(text > 0);
    }
}

#[cfg(test)]
mod export_tests {
    use super::*;

    /// Exports the newest chats twice into the same folder: the second run must replace the files, not add more.
    #[test]
    #[ignore = "reads the real chat index on this PC"]
    fn export_twice_overwrites() {
        let dir = std::env::temp_dir().join("chatvault-export-test");
        let _ = fs::remove_dir_all(&dir);
        let mut counts = Vec::new();
        for _ in 0..2 {
            let started = std::time::Instant::now();
            let mut written = 0;
            for (tool, name) in EXPORT_TOOLS {
                let Ok(rows) = index::recent_paths(tool, 150) else { continue };
                for (id, path) in rows {
                    if let Some((file, md)) = chat_markdown(tool, name, &id, &path) {
                        let d = dir.join(name);
                        fs::create_dir_all(&d).unwrap();
                        let t = d.join(format!("{}.tmp", file));
                        fs::write(&t, md).unwrap();
                        fs::rename(&t, d.join(&file)).unwrap();
                        written += 1;
                    }
                }
            }
            let files = walkdir_count(&dir);
            println!("run: wrote {} chats, {} files on disk, {:?}", written, files, started.elapsed());
            counts.push(files);
        }
        assert_eq!(counts[0], counts[1], "the second export must replace the files");
        assert!(counts[0] > 0);
        let _ = fs::remove_dir_all(&dir);
    }

    fn walkdir_count(p: &Path) -> usize {
        let mut v = Vec::new();
        walk_files(p, &["md", "tmp"], &mut v);
        v.len()
    }
}

#[cfg(test)]
mod launch_quote_tests {
    use super::*;

    #[test]
    fn every_powershell_quote_is_doubled() {
        assert_eq!(ps_single_quoted_inner("don't"), "don''t");
        assert_eq!(ps_single_quoted_inner("don\u{2019}t"), "don\u{2019}\u{2019}t");
        assert_eq!(ps_single_quoted_inner("a\u{2018}b\u{201A}c\u{201B}d"), "a\u{2018}\u{2018}b\u{201A}\u{201A}c\u{201B}\u{201B}d");
    }

    #[test]
    fn line_breaks_cannot_start_a_new_command() {
        assert_eq!(ps_single_quoted_inner("one\r\ntwo\nthree"), "one  two three");
    }

    #[test]
    fn plain_text_is_unchanged() {
        assert_eq!(ps_single_quoted_inner("Read the file C:/a b/c.md"), "Read the file C:/a b/c.md");
    }
}
