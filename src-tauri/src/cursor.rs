//! Cursor chats. The IDE keeps older chats in a SQLite file (`composerData` plus one row per message),
//! and the Cursor CLI keeps each chat in its own `store.db`. Newer IDE agent chats are encrypted by
//! Cursor and cannot be read from outside, so they are not listed.

use crate::paths::*;
use crate::providers::{clip, Msg};
use rusqlite::{Connection, OpenFlags};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

pub struct CursorChat {
    pub id: String,
    pub title: String,
    pub cwd: Option<String>,
    pub created_ms: Option<i64>,
    pub last_ms: i64,
    pub size: i64,
    pub path: String,
}

fn appdata_cursor() -> Option<PathBuf> {
    env_path("APPDATA").map(|p| p.join("Cursor").join("User"))
}

fn global_db() -> Option<PathBuf> {
    appdata_cursor().map(|p| p.join("globalStorage").join("state.vscdb"))
}

/// The email Cursor is signed in with. Only that one entry is read; the sign-in tokens next to it are never touched.
pub fn account_email() -> Option<String> {
    let c = open_ro(&global_db()?)?;
    let v: String = c.query_row("SELECT value FROM ItemTable WHERE key='cursorAuth/cachedEmail'", [], |r| r.get(0)).ok()?;
    let v = v.trim().trim_matches('"').to_string();
    v.contains('@').then_some(v)
}

fn ide_dbs() -> Vec<PathBuf> {
    let mut out=Vec::new();
    let Some(user)=appdata_cursor() else{return out};
    let mut users=vec![user.clone()];users.extend(subdirs(&user.join("profiles")));
    for user in users {
        let global=user.join("globalStorage/state.vscdb");
        if global.is_file(){out.push(global);}
        for ws in subdirs(&user.join("workspaceStorage")) {
            let p=ws.join("state.vscdb");if p.is_file(){out.push(p);}
        }
    }
    out
}

/// Where Cursor's chats are read from: (what it is, folder or file, how many chats are in it).
pub fn locations() -> Vec<(String, PathBuf, Option<usize>)> {
    let mut out = Vec::new();
    if let Some(g) = global_db() {
        out.push(("Editor chats (database)".to_string(), g, None));
    }
    if let Some(ws) = appdata_cursor().map(|u| u.join("workspaceStorage")) {
        let n = ide_dbs().iter().filter(|p| p.starts_with(&ws)).count();
        out.push((format!("Editor chats per project ({n} databases)"), ws, None));
    }
    if let Some(r) = cli_root() {
        out.push(("Cursor Agent chats".to_string(), r, Some(cli_stores().len())));
    }
    if let Some(r) = home().map(|h| h.join(".cursor/projects")) {
        out.push(("Saved agent transcripts".to_string(), r, Some(transcript_files().len())));
    }
    out
}

fn cli_root() -> Option<PathBuf> {
    home().map(|p| p.join(".cursor").join("chats"))
}

/// Opens a database that Cursor may be using, without taking any lock on it.
fn open_ro(path: &Path) -> Option<Connection> {
    // Cursor writes new messages to the database's write-ahead log first. Reading normally includes them;
    // "immutable" would ignore the log and miss everything newer than the last checkpoint.
    if let Ok(c) = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY) {
        if c.query_row("SELECT 1 FROM sqlite_master LIMIT 1", [], |r| r.get::<_, i64>(0)).is_ok() {
            return Some(c);
        }
    }
    None
}

fn mtime_ms(p: &Path) -> i64 {
    fs::metadata(p)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn pct_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() + 0 && i + 2 <= b.len() - 1 {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

fn folder_of(uri: &str) -> String {
    let t = pct_decode(uri.trim_start_matches("file:///"));
    t.replace('/', "\\")
}

fn workspace_folder(id: &str) -> Option<String> {
    let p = appdata_cursor()?.join("workspaceStorage").join(id).join("workspace.json");
    let v: Value = serde_json::from_str(&fs::read_to_string(p).ok()?).ok()?;
    v.get("folder").and_then(|f| f.as_str()).map(folder_of)
}

fn bubble_text(v: &Value) -> String {
    v.get("text").and_then(|t| t.as_str()).unwrap_or("").to_string()
}

fn first_user_bubble(c: &Connection, composer: &str, headers: &[Value]) -> Option<String> {
    for h in headers {
        if h.get("type").and_then(|t| t.as_i64()) != Some(1) {
            continue;
        }
        let bid = h.get("bubbleId").and_then(|b| b.as_str())?;
        let raw: String = c
            .query_row("SELECT value FROM cursorDiskKV WHERE key=?1", [format!("bubbleId:{}:{}", composer, bid)], |r| r.get(0))
            .ok()?;
        let t = bubble_text(&serde_json::from_str::<Value>(&raw).ok()?);
        if !t.trim().is_empty() {
            return Some(clip(t.trim(), 80));
        }
    }
    None
}

fn scan_ide(out: &mut Vec<CursorChat>) {
    for db in ide_dbs() {scan_ide_db(&db,out);}
}
fn scan_ide_db(db:&Path,out:&mut Vec<CursorChat>) {
    let Some(c) = open_ro(db) else { return };
    let mtime = mtime_ms(db);
    // Workspace and update times live in a separate headers table.
    let mut heads = std::collections::HashMap::<String, (Option<String>, Option<i64>)>::new();
    if let Ok(mut st) = c.prepare("SELECT composerId, workspaceId, lastUpdatedAt FROM composerHeaders") {
        if let Ok(rows) = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, Option<i64>>(2)?))) {
            for (id, ws, up) in rows.flatten() {
                heads.insert(id, (ws, up));
            }
        }
    }
    let Ok(mut st) = c.prepare("SELECT key, value FROM cursorDiskKV WHERE key LIKE 'composerData:%'") else { return };
    let Ok(rows) = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))) else { return };
    for (key, raw) in rows.flatten() {
        let Ok(d) = serde_json::from_str::<Value>(&raw) else { continue };
        let headers = d.get("fullConversationHeadersOnly").and_then(|h| h.as_array()).cloned().unwrap_or_default();
        if headers.is_empty() {
            continue;
        }
        let id = key.trim_start_matches("composerData:").to_string();
        let name = d.get("name").and_then(|n| n.as_str()).filter(|n| !n.trim().is_empty()).map(String::from);
        let title = name.or_else(|| first_user_bubble(&c, &id, &headers)).unwrap_or_else(|| "Untitled".into());
        let (ws, updated) = heads.get(&id).cloned().unwrap_or((None, None));
        let created = d.get("createdAt").and_then(|x| x.as_i64());
        let cwd = ws.as_deref().filter(|w| *w != "empty-window").and_then(workspace_folder);
        out.push(CursorChat {
            path: format!("cursor-ide:{}|{}", db.display(), id),
            id,
            title,
            cwd,
            created_ms: created,
            last_ms: d.get("lastUpdatedAt").and_then(|x| x.as_i64()).or(updated).or(created).unwrap_or(mtime),
            size: raw.len() as i64,
        });
    }
}

fn cli_stores() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(root) = cli_root() {
        for a in subdirs(&root) {
            for b in subdirs(&a) {
                let s = b.join("store.db");
                if s.is_file() {
                    out.push(s);
                }
            }
        }
    }
    out
}

fn transcript_files() -> Vec<PathBuf> {
    let mut out=Vec::new();
    if let Some(root)=home().map(|h|h.join(".cursor/projects")) {
        for project in subdirs(&root) {walk_files(&project.join("agent-transcripts"), &["jsonl"], &mut out);}
    }
    out
}
fn transcript_messages(path:&Path) -> Vec<Msg> {
    let Some(lines)=open_lines(path) else {return Vec::new()};
    let mut out=Vec::new();
    for line in lines {
        let Ok(mut v)=serde_json::from_str::<Value>(&line) else {continue};
        let role=v.get("role").and_then(Value::as_str).unwrap_or("").to_string();
        v["type"]=Value::String(role);
        for mut m in crate::providers::claude_messages(&v) {
            if m.role=="user" && m.kind=="text" {m.text=user_query(&m.text);}
            out.push(m);
        }
    }
    out
}
fn scan_transcripts(out:&mut Vec<CursorChat>) {
    for p in transcript_files() {
        let msgs=transcript_messages(&p);
        let Some(first)=msgs.iter().find(|m|m.role=="user" && m.kind=="text") else {continue};
        out.push(CursorChat {id:p.file_stem().map(|s|s.to_string_lossy().to_string()).unwrap_or_default(),title:clip(&first.text,80),cwd:None,created_ms:None,last_ms:mtime_ms(&p),size:fs::metadata(&p).map(|m|m.len() as i64).unwrap_or(0),path:format!("cursor-transcript:{}",p.display())});
    }
}

fn content_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(a) => a.iter().filter_map(|x| x.get("text").and_then(|t| t.as_str())).collect::<Vec<_>>().join("\n"),
        _ => String::new(),
    }
}

/// The CLI wraps the real request in <user_query> after a block of environment information.
fn user_query(text: &str) -> String {
    if let (Some(a), Some(b)) = (text.find("<user_query>"), text.find("</user_query>")) {
        if b > a {
            return text[a + 12..b].trim().to_string();
        }
    }
    text.trim().to_string()
}

fn cursor_ide_parts(bubble: &Value, role: &str, ts: &str, out: &mut Vec<Msg>) {
    use crate::providers::{attachment_message, image_message, local_attachment_path};
    let images = bubble.get("images").and_then(Value::as_array).filter(|images| !images.is_empty())
        .or_else(|| bubble.pointer("/context/selectedImages").and_then(Value::as_array));
    for item in images.into_iter().flatten() {
        let source = item.as_str().or_else(|| item.get("dataUrl").and_then(Value::as_str));
        out.push(attachment_message(role, ts, item)
            .unwrap_or_else(|| image_message(role, ts, None, source)));
    }
    for file in bubble.pointer("/context/fileSelections").and_then(Value::as_array).into_iter().flatten() {
        let path = file.pointer("/uri/_fsPath").or_else(|| file.pointer("/uri/path")).and_then(Value::as_str);
        if let Some(name) = path.and_then(|p| Path::new(p).file_name()).and_then(|n| n.to_str()) {
            let mut msg = Msg::new(role, ts, name.to_owned(), "attachment");
            msg.data = path.and_then(local_attachment_path).map(|path| serde_json::json!({ "path": path }));
            out.push(msg);
        }
    }
    let Some(tool) = bubble.get("toolFormerData") else { return };
    let Some(name) = tool.get("name").and_then(Value::as_str) else { return };
    let mut message = Msg::new("assistant", ts, format!("{name}:"), "tool");
    message.name = Some(name.to_owned());
    message.id = tool.get("toolCallId").and_then(Value::as_str).map(String::from);
    let params = tool.get("params").or_else(|| tool.get("rawArgs"));
    message.data = params.map(|p| {
        p.as_str().and_then(|s| serde_json::from_str(s).ok()).unwrap_or_else(|| p.clone())
    });
    let input = message.data.clone();
    crate::providers::annotate(&mut message, input.as_ref());
    message.error = Some(matches!(tool.get("status").and_then(Value::as_str), Some("error" | "failed")));
    if let Some(output) = tool.get("result").filter(|v| !v.is_null()) {
        let text = output.as_str().map(String::from)
            .or_else(|| output.get("output").or_else(|| output.get("text")).and_then(Value::as_str).map(String::from))
            .unwrap_or_else(|| output.to_string());
        let mut result = Msg::new("assistant", ts, clip(&text, 8000), "result");
        result.error = message.error;
        message.result = Some(Box::new(result));
    }
    out.push(message);
}

fn cli_messages(path: &Path) -> Vec<Msg> {
    let mut out = Vec::new();
    let Some(c) = open_ro(path) else { return out };
    let Ok(mut st) = c.prepare("SELECT data FROM blobs ORDER BY rowid") else { return out };
    let Ok(rows) = st.query_map([], |r| r.get::<_, Vec<u8>>(0)) else { return out };
    let ts = mtime_ms(path);
    let iso = chrono_iso(ts);
    for data in rows.flatten() {
        let Ok(v) = serde_json::from_slice::<Value>(&data) else { continue };
        let role = v.get("role").and_then(|r| r.as_str()).unwrap_or("");
        let content = v.get("content").cloned().unwrap_or(Value::Null);
        match role {
            "user" => {
                let t = user_query(&content_text(&content));
                if !t.is_empty() && !t.starts_with("<user_info>") {
                    out.push(Msg::new("user", &iso, t, "text"));
                }
            }
            "assistant" => {
                let t = content_text(&content);
                if !t.trim().is_empty() {
                    out.push(Msg::new("assistant", &iso, t, "text"));
                }
            }
            _ => {}
        }
        if let Some(parts) = content.as_array() {
            let blocks: Vec<_> = parts.iter().filter(|part| part.get("type").and_then(Value::as_str) != Some("text")).cloned().collect();
            if !blocks.is_empty() && matches!(role, "user" | "assistant") {
                out.extend(crate::providers::claude_messages(&serde_json::json!({
                    "type": role, "timestamp": iso, "message": {"role": role, "content": blocks}
                })));
            }
        }
    }
    out
}

fn chrono_iso(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms).map(|d| d.to_rfc3339()).unwrap_or_default()
}

fn scan_cli(out: &mut Vec<CursorChat>) {
    for store in cli_stores() {
        let msgs = cli_messages(&store);
        let Some(first) = msgs.iter().find(|m| m.role == "user") else { continue };
        let id = store.parent().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let mtime = mtime_ms(&store);
        // The workspace path is written into the first user block.
        let cwd = open_ro(&store)
            .and_then(|c| {
                let mut st = c.prepare("SELECT data FROM blobs ORDER BY rowid LIMIT 4").ok()?;
                let rows = st.query_map([], |r| r.get::<_, Vec<u8>>(0)).ok()?;
                rows.flatten().find_map(|d| {
                    let v = serde_json::from_slice::<Value>(&d).ok()?;
                    let t = content_text(v.get("content")?);
                    let k = t.find("Workspace Path: ")?;
                    Some(t[k + 16..].lines().next()?.trim().to_string())
                })
            });
        out.push(CursorChat {
            path: format!("cursor-cli:{}", store.to_string_lossy()),
            id: format!("cli-{}", id),
            title: clip(first.text.lines().next().unwrap_or("Untitled"), 80),
            cwd,
            created_ms: None,
            last_ms: mtime,
            size: fs::metadata(&store).map(|m| m.len() as i64).unwrap_or(0),
        });
    }
}

pub fn scan() -> Vec<CursorChat> {
    let mut out = Vec::new();
    scan_ide(&mut out);
    scan_cli(&mut out);
    scan_transcripts(&mut out);
    out.sort_by_key(|chat|std::cmp::Reverse(chat.last_ms));
    let mut ids=std::collections::HashSet::new();
    out.retain(|chat|ids.insert(chat.id.clone()));
    out
}

/// A fingerprint of the Cursor files, so the index can skip a rescan when nothing changed.
pub fn fingerprint() -> i64 {
    let a = ide_dbs().into_iter()
        .map(|p| {
            let mut wal = p.clone().into_os_string();
            wal.push("-wal");
            mtime_ms(&p).max(mtime_ms(Path::new(&wal)))
        })
        .max().unwrap_or(0);
    let b = cli_stores().iter().map(|p| mtime_ms(p)).max().unwrap_or(0);
    a.max(b).max(transcript_files().iter().map(|p|mtime_ms(p)).max().unwrap_or(0))
}

/// The real file behind a virtual cursor path, for "show file".
pub fn real_file(path: &str) -> Option<PathBuf> {
    if let Some(file)=path.strip_prefix("cursor-transcript:") {
        let canon=Path::new(file).canonicalize().ok()?;
        return transcript_files().into_iter().find(|p|p.canonicalize().ok().as_ref()==Some(&canon));
    }
    if let Some(key)=path.strip_prefix("cursor-ide:") {
        if let Some((db,_))=key.rsplit_once('|') {return ide_dbs().into_iter().find(|p|p==Path::new(db));}
        return global_db();
    }
    let p = PathBuf::from(path.strip_prefix("cursor-cli:")?);
    let root = cli_root()?.canonicalize().ok()?;
    let canon = p.canonicalize().ok()?;
    canon.starts_with(root).then_some(canon)
}

/// What deleting a Cursor chat removes: a transcript file, or the folder of a Cursor Agent chat. Chats kept inside
/// Cursor's own database are not touched, so those return nothing.
pub fn deletable(path: &str) -> Option<PathBuf> {
    if path.starts_with("cursor-transcript:") {
        return real_file(path);
    }
    if path.starts_with("cursor-cli:") {
        let store = real_file(path)?;
        let folder = store.parent()?.to_path_buf();
        let root = cli_root()?.canonicalize().ok()?;
        return (folder != root && folder.starts_with(&root)).then_some(folder);
    }
    None
}

pub fn read(path: &str) -> Result<Vec<Msg>, String> {
    if path.starts_with("cursor-transcript:") {return Ok(transcript_messages(&real_file(path).ok_or("That Cursor transcript is not available")?));}
    if let Some(id) = path.strip_prefix("cursor-ide:") {
        let db = real_file(path).ok_or("Cursor is not installed")?;
        let id=id.rsplit_once('|').map(|(_,id)|id).unwrap_or(id);
        let c = open_ro(&db).ok_or("Cannot open the Cursor database")?;
        let raw: String = c
            .query_row("SELECT value FROM cursorDiskKV WHERE key=?1", [format!("composerData:{}", id)], |r| r.get(0))
            .map_err(|_| "That Cursor chat no longer exists".to_string())?;
        let d: Value = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for h in d.get("fullConversationHeadersOnly").and_then(|h| h.as_array()).cloned().unwrap_or_default() {
            let Some(bid) = h.get("bubbleId").and_then(|b| b.as_str()) else { continue };
            let Ok(raw): Result<String, _> = c.query_row("SELECT value FROM cursorDiskKV WHERE key=?1", [format!("bubbleId:{}:{}", id, bid)], |r| r.get(0)) else {
                continue;
            };
            let Ok(b) = serde_json::from_str::<Value>(&raw) else { continue };
            let role = if b.get("type").and_then(|t| t.as_i64()) == Some(1) { "user" } else { "assistant" };
            let ts = b.get("createdAt").and_then(|x| x.as_str()).unwrap_or("").to_string();
            let text = bubble_text(&b);
            if !text.trim().is_empty() {
                out.push(Msg::new(role, &ts, text, "text"));
            }
            cursor_ide_parts(&b, role, &ts, &mut out);
        }
        return Ok(out);
    }
    let store = real_file(path).ok_or("That is not a Cursor chat")?;
    Ok(cli_messages(&store))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exported_agent_transcript_reads_both_roles() {
        let path=crate::cli::script_file("").unwrap().with_extension("jsonl");
        fs::write(&path,"{\"role\":\"user\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"<user_query>fixture question</user_query>\"}]}}\n{\"role\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"fixture answer\"}]}}\n").unwrap();
        let msgs=transcript_messages(&path);assert_eq!(msgs.len(),2);assert_eq!(msgs[0].text,"fixture question");assert_eq!(msgs[1].role,"assistant");
        assert!(real_file("cursor-transcript:C:\\outside\\test.jsonl").is_none());
    }
}
