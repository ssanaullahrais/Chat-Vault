//! Gemini CLI, OpenCode and Cline chats. Each keeps plain, readable files, so they are read directly:
//! - Gemini CLI: `~/.gemini/tmp/<project>/chats/session-*.json`
//! - OpenCode: the SQLite file `~/.local/share/opencode/opencode.db` (sessions, messages and parts)
//! - Cline: `~/.cline/data/sessions/<id>/<id>.messages.json` with a `<id>.json` summary beside it

use crate::paths::*;
use crate::providers::{attachment_message, clip, Msg};
use rusqlite::{Connection, OpenFlags};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

pub const TOOLS: [&str; 3] = ["gemini", "opencode", "cline"];

pub struct OtherChat {
    pub provider: &'static str,
    pub id: String,
    pub title: String,
    pub cwd: Option<String>,
    pub created_ms: Option<i64>,
    pub last_ms: i64,
    pub size: i64,
    pub path: String,
    pub model: Option<String>,
}

fn ms_of(iso: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(iso).ok().map(|d| d.timestamp_millis())
}

fn mtime(p: &Path) -> i64 {
    fs::metadata(p).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_millis() as i64).unwrap_or(0)
}

fn open_ro(path: &Path) -> Option<Connection> {
    Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).ok()
}

fn read_json(p: &Path) -> Option<Value> {
    serde_json::from_str(&fs::read_to_string(p).ok()?).ok()
}

// ---------------- Gemini CLI ----------------

/// The folders Gemini CLI keeps its chats in.
pub fn gemini_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    for p in [env_path("GEMINI_CLI_HOME").map(|p| p.join(".gemini/tmp")), home().map(|h| h.join(".gemini/tmp"))].into_iter().flatten() {
        if !roots.contains(&p) {
            roots.push(p);
        }
    }
    roots
}

fn gemini_files() -> Vec<PathBuf> {
    let mut out = Vec::new();
    for tmp in gemini_roots() {
    let Ok(rd) = fs::read_dir(&tmp) else { continue };
    for proj in rd.flatten() {
        let chats = proj.path().join("chats");
        if let Ok(files) = fs::read_dir(&chats) {
            for f in files.flatten() {
                let p = f.path();
                if p.extension().is_some_and(|e| e == "json" || e == "jsonl") {
                    out.push(p);
                }
            }
        }
    }
    }
    out
}

/// The text of a Gemini user turn: the typed text when it is recorded, otherwise the first text part of the content.
fn gemini_user_text(m: &Value) -> String {
    let flat = |v: &Value| -> String {
        match v {
            Value::String(s) => s.clone(),
            Value::Array(a) => a.iter().filter_map(|p| p.get("text").and_then(|t| t.as_str())).collect::<Vec<_>>().join("\n"),
            _ => String::new(),
        }
    };
    let shown = m.get("displayContent").map(flat).unwrap_or_default();
    if !shown.trim().is_empty() {
        return shown;
    }
    m.get("content").map(flat).unwrap_or_default()
}

fn gemini_project(p: &Path) -> Option<String> {
    let root = p.parent()?.parent()?.join(".project_root");
    fs::read_to_string(root).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

fn gemini_session(path: &Path) -> Option<Value> {
    if path.extension().is_none_or(|e| e != "jsonl") {
        return read_json(path);
    }
    let mut session = serde_json::Map::new();
    let mut messages = Vec::new();
    for line in open_lines(path)? {
        let Ok(mut entry) = serde_json::from_str::<Value>(&line) else { continue };
        if let Some(patch) = entry.get_mut("$set").and_then(Value::as_object_mut) {
            if let Some(Value::Array(updated)) = patch.remove("messages") {
                messages = updated;
            }
            session.extend(std::mem::take(patch));
        } else if entry.get("type").is_some() {
            messages.push(entry);
        } else if let Value::Object(fields) = entry {
            session.extend(fields);
        }
    }
    session.insert("messages".into(), Value::Array(messages));
    Some(Value::Object(session))
}

fn gemini_messages(j: &Value) -> Vec<Msg> {
    let mut out = Vec::new();
    for m in j.get("messages").and_then(|m| m.as_array()).cloned().unwrap_or_default() {
        let ts = m.get("timestamp").and_then(|t| t.as_str()).unwrap_or("").to_string();
        match m.get("type").and_then(|t| t.as_str()).unwrap_or("") {
            "user" => {
                let t = gemini_user_text(&m);
                if !t.trim().is_empty() {
                    out.push(Msg::new("user", &ts, t, "text"));
                }
                let media: Vec<_> = m.get("content").and_then(Value::as_array).into_iter().flatten()
                    .filter_map(|part| attachment_message("user", &ts, part)).collect();
                if media.is_empty() {
                    out.extend(m.get("displayContent").and_then(Value::as_array).into_iter().flatten()
                        .filter_map(|part| attachment_message("user", &ts, part)));
                } else {
                    out.extend(media);
                }
            }
            "gemini" => {
                if let Some(thoughts) = m.get("thoughts").and_then(|t| t.as_array()) {
                    let text: Vec<String> = thoughts
                        .iter()
                        .map(|t| format!("{}: {}", t.get("subject").and_then(|x| x.as_str()).unwrap_or(""), t.get("description").and_then(|x| x.as_str()).unwrap_or("")))
                        .collect();
                    if !text.is_empty() {
                        out.push(Msg::new("assistant", &ts, clip(&text.join("\n"), 6000), "reasoning"));
                    }
                }
                if let Some(t) = m.get("content").and_then(Value::as_str).filter(|t| !t.trim().is_empty()) {
                    out.push(Msg::new("assistant", &ts, t.to_owned(), "text"));
                }
                for part in m.get("content").and_then(Value::as_array).into_iter().flatten() {
                    if let Some(t) = part.get("text").and_then(Value::as_str).filter(|t| !t.trim().is_empty()) {
                        out.push(Msg::new("assistant", &ts, t.to_owned(), "text"));
                    } else if let Some(media) = attachment_message("assistant", &ts, part) {
                        out.push(media);
                    }
                }
                for (i, call) in m.get("toolCalls").and_then(Value::as_array).into_iter().flatten().enumerate() {
                    let name = call.get("name").and_then(|n| n.as_str()).unwrap_or("tool").to_string();
                    let id = Some(call.get("id").and_then(Value::as_str).map(String::from).unwrap_or_else(|| format!("gemini-{}-{i}", out.len())));
                    let mut tool = Msg::new("assistant", &ts, format!("{}:", name), "tool");
                    tool.name = Some(name);
                    tool.id = id.clone();
                    tool.error = Some(matches!(call.get("status").and_then(Value::as_str), Some("error" | "failed")));
                    tool.data = call.get("args").cloned(); { let inp = tool.data.clone(); crate::providers::annotate(&mut tool, inp.as_ref()); }
                    out.push(tool);
                    let result = call.get("resultDisplay").and_then(Value::as_str).filter(|s| !s.trim().is_empty()).map(String::from)
                        .or_else(|| call.get("result").map(gemini_tool_text));
                    if let Some(r) = result.filter(|r| !r.trim().is_empty()) {
                        let mut res = Msg::new("assistant", &ts, clip(&r, 8000), "result");
                        res.id = id;
                        out.push(res);
                    }
                    for part in call.get("result").and_then(Value::as_array).into_iter().flatten() {
                        if let Some(media) = attachment_message("assistant", &ts, part) { out.push(media); }
                    }
                }
            }
            "error" => {
                let t = m.get("content").and_then(|c| c.as_str()).unwrap_or("");
                if !t.trim().is_empty() {
                    out.push(crate::providers::notice(&ts, "error", t, true));
                }
            }
            // Gemini CLI's own notes ("Update successful", warnings) and any kind added later: shown as notes, never dropped.
            other => {
                let t = crate::providers::text_of(&m);
                if !t.is_empty() {
                    out.push(crate::providers::notice(&ts, other, &t, other == "warning"));
                }
            }
        }
    }
    out
}

fn gemini_tool_text(result: &Value) -> String {
    match result {
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts.iter().map(gemini_tool_text).filter(|text| !text.is_empty()).collect::<Vec<_>>().join("\n"),
        Value::Object(_) if attachment_message("assistant", "", result).is_some() => String::new(),
        Value::Object(_) => {
            if let Some(value) = result.get("text").or_else(|| result.pointer("/functionResponse/response")).or_else(|| result.get("output")) {
                gemini_tool_text(value)
            } else {
                result.to_string()
            }
        }
        Value::Null => String::new(),
        value => value.to_string(),
    }
}

fn scan_gemini(out: &mut Vec<OtherChat>) {
    for p in gemini_files() {
        let Some(j) = gemini_session(&p) else { continue };
        let msgs = gemini_messages(&j);
        let Some(first) = msgs.iter().find(|m| m.role == "user") else { continue };
        let first_line = first.text.lines().find(|l| !l.trim().is_empty()).unwrap_or("Chat").trim();
        let id = j.get("sessionId").and_then(|s| s.as_str()).map(String::from).unwrap_or_else(|| p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default());
        out.push(OtherChat {
            provider: "gemini",
            id,
            title: clip(first_line, 80),
            cwd: gemini_project(&p),
            created_ms: j.get("startTime").and_then(|s| s.as_str()).and_then(ms_of),
            last_ms: j.get("lastUpdated").and_then(|s| s.as_str()).and_then(ms_of).unwrap_or_else(|| mtime(&p)),
            size: fs::metadata(&p).map(|m| m.len() as i64).unwrap_or(0),
            path: format!("gemini:{}", p.to_string_lossy()),
            model: None,
        });
    }
}

// ---------------- OpenCode ----------------

fn opencode_db() -> Option<PathBuf> {
    opencode_dbs().into_iter().next()
}
/// Where OpenCode's database can be: the places it is looked for, whether or not a file is there.
pub fn opencode_candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    for p in [env_path("OPENCODE_DB"), env_path("XDG_DATA_HOME").map(|p| p.join("opencode/opencode.db")), home().map(|h| h.join(".local/share/opencode/opencode.db"))].into_iter().flatten() {
        if !out.contains(&p) {
            out.push(p);
        }
    }
    out
}

pub fn opencode_found() -> Vec<PathBuf> {
    opencode_dbs()
}

fn opencode_dbs() -> Vec<PathBuf> {
    let mut out=Vec::new();
    for p in [env_path("OPENCODE_DB"),env_path("XDG_DATA_HOME").map(|p|p.join("opencode/opencode.db")),home().map(|h|h.join(".local/share/opencode/opencode.db"))].into_iter().flatten() {
        if p.is_file() && !out.contains(&p) {out.push(p);}
    }
    out
}

fn scan_opencode(out: &mut Vec<OtherChat>) {
    for db in opencode_dbs() { scan_opencode_db(&db,out); }
}
fn has_table(c: &Connection, name: &str) -> bool {
    c.query_row("SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1", [name], |_| Ok(())).is_ok()
}

/// OpenCode keeps its chats in one of two layouts, and a database upgraded from 1.x can hold both:
/// - 1.x: `session` + `message` + `part`;
/// - 2.x: `session_v2` + `session_message` (one row per message, typed) + `session_part` (the pieces of an answer;
///   older 2.x builds kept them inside the message as `content`). Early 2.x builds used `session` with `session_message`.
fn scan_opencode_db(db: &Path, out: &mut Vec<OtherChat>) {
    let Some(c) = open_ro(db) else { return };
    let v2_messages = has_table(&c, "session_message");
    let v1_messages = has_table(&c, "message");
    let mut seen = std::collections::HashSet::new();
    let count_sql = |s: &str| {
        let mut parts = Vec::new();
        if v2_messages {
            parts.push(format!("(SELECT COUNT(*) FROM session_message m WHERE m.session_id={s}.id AND m.type IN ('user','assistant'))"));
        }
        if v1_messages {
            parts.push(format!("(SELECT COUNT(*) FROM message m WHERE m.session_id={s}.id)"));
        }
        if parts.is_empty() { "0".to_string() } else { parts.join(" + ") }
    };
    let first_user = |s: &str| {
        if v2_messages {
            format!("(SELECT json_extract(m.data,'$.text') FROM session_message m WHERE m.session_id={s}.id AND m.type='user' ORDER BY m.seq LIMIT 1)")
        } else {
            "NULL".to_string()
        }
    };
    for table in ["session_v2", "session"] {
        if !has_table(&c, table) {
            continue;
        }
        let sql = format!(
            "SELECT s.id, s.title, s.directory, s.time_created, s.time_updated, {}, {}, s.slug FROM {table} s",
            count_sql("s"),
            first_user("s")
        );
        let Ok(mut st) = c.prepare(&sql) else { continue };
        let rows = st.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, i64>(5)?,
                r.get::<_, Option<String>>(6)?,
                r.get::<_, Option<String>>(7)?,
            ))
        });
        let Ok(rows) = rows else { continue };
        for (id, title, dir, created, updated, n, first, slug) in rows.flatten() {
            if n == 0 || !seen.insert(id.clone()) {
                continue;
            }
            // 2.x sessions may have no title yet: use the first message, then OpenCode's short name for the session.
            let title = title
                .filter(|t| !t.trim().is_empty())
                .or_else(|| first.map(|f| f.lines().next().unwrap_or("").to_string()).filter(|t| !t.trim().is_empty()))
                .or(slug)
                .unwrap_or_else(|| "OpenCode chat".into());
            out.push(OtherChat {
                provider: "opencode",
                id: id.clone(),
                title: clip(title.trim(), 80),
                cwd: dir.filter(|d| d != "/" && !d.is_empty()),
                created_ms: Some(created),
                last_ms: updated.max(created),
                size: n * 2000,
                path: format!("opencode:{}|{}", db.display(), id),
                model: None,
            });
        }
    }
}

/// Text of a 2.x tool result: its text pieces, or the error it ended with.
fn opencode_v2_tool_output(state: &Value) -> Option<String> {
    let mut text = String::new();
    if let Some(items) = state.get("content").and_then(|c| c.as_array()) {
        for it in items {
            if let Some(t) = it.get("text").and_then(|t| t.as_str()) {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(t);
            }
        }
    }
    if text.trim().is_empty() {
        if let Some(output) = state.get("output").and_then(Value::as_str) {
            text = output.to_owned();
        }
    }
    if text.trim().is_empty() {
        if let Some(e) = state.get("error") {
            text = e.get("message").and_then(|m| m.as_str()).map(String::from).unwrap_or_else(|| e.to_string());
        }
    }
    (!text.trim().is_empty()).then_some(text)
}

/// The messages of a chat stored the 2.x way, or None when this chat has none there (a 1.x chat).
fn opencode_v2_messages(c: &Connection, id: &str) -> Option<Vec<Msg>> {
    if !has_table(c, "session_message") {
        return None;
    }
    let mut st = c.prepare("SELECT id, type, time_created, data FROM session_message WHERE session_id=?1 ORDER BY seq").ok()?;
    let rows: Vec<(String, String, i64, String)> = st.query_map([id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).ok()?.flatten().collect();
    if rows.is_empty() {
        return None;
    }
    let mut part_st = if has_table(c, "session_part") { c.prepare("SELECT data FROM session_part WHERE message_id=?1 ORDER BY ordinal").ok() } else { None };
    let mut out = Vec::new();
    for (mid, kind, created, data) in rows {
        let Ok(v) = serde_json::from_str::<Value>(&data) else { continue };
        let ts = chrono::DateTime::from_timestamp_millis(created).map(|d| d.to_rfc3339()).unwrap_or_default();
        match kind.as_str() {
            "user" => {
                let t = v.get("text").and_then(|t| t.as_str()).unwrap_or("");
                if !t.trim().is_empty() {
                    out.push(Msg::new("user", &ts, t.to_string(), "text"));
                }
                let parts: Vec<Value> = part_st.as_mut()
                    .and_then(|ps| ps.query_map([&mid], |r| r.get::<_, String>(0)).ok()
                        .map(|rows| rows.flatten().filter_map(|raw| serde_json::from_str(&raw).ok()).collect()))
                    .filter(|parts: &Vec<Value>| !parts.is_empty())
                    .or_else(|| v.get("content").and_then(Value::as_array).cloned()).unwrap_or_default();
                for part in &parts {
                    if let Some(media) = attachment_message("user", &ts, part) {
                        out.push(media);
                    } else if t.trim().is_empty() {
                        if let Some(text) = part.get("text").and_then(Value::as_str).filter(|s| !s.trim().is_empty()) {
                            out.push(Msg::new("user", &ts, text.to_owned(), "text"));
                        }
                    }
                }
            }
            "shell" => {
                let cmd = v.get("command").and_then(|t| t.as_str()).unwrap_or("").to_string();
                let mut tool = Msg::new("assistant", &ts, "bash:".into(), "tool");
                tool.name = Some("bash".into());
                tool.id = Some(mid.clone());
                tool.data = Some(serde_json::json!({ "command": cmd }));
                out.push(tool);
                let output = match v.get("output") {
                    Some(Value::String(s)) => s.clone(),
                    Some(o) => o.get("text").and_then(|t| t.as_str()).map(String::from).unwrap_or_default(),
                    None => String::new(),
                };
                if !output.trim().is_empty() {
                    let mut res = Msg::new("assistant", &ts, clip(&output, 8000), "result");
                    res.id = Some(mid.clone());
                    out.push(res);
                }
            }
            "compaction" => {
                if let Some(s) = v.get("summary").and_then(|t| t.as_str()).filter(|s| !s.trim().is_empty()) {
                    out.push(Msg::new("assistant", &ts, s.to_string(), "summary"));
                }
            }
            "assistant" => {
                let mut parts: Vec<Value> = part_st
                    .as_mut()
                    .and_then(|ps| ps.query_map([&mid], |r| r.get::<_, String>(0)).ok().map(|r| r.flatten().filter_map(|s| serde_json::from_str(&s).ok()).collect()))
                    .unwrap_or_default();
                if parts.is_empty() {
                    parts = v.get("content").and_then(|c| c.as_array()).cloned().unwrap_or_default();
                }
                for p in parts {
                    if let Some(media) = attachment_message("assistant", &ts, &p) {
                        out.push(media);
                        continue;
                    }
                    match p.get("type").and_then(|t| t.as_str()).unwrap_or("") {
                        "text" => {
                            let t = p.get("text").and_then(|t| t.as_str()).unwrap_or("");
                            if !t.trim().is_empty() {
                                out.push(Msg::new("assistant", &ts, t.to_string(), "text"));
                            }
                        }
                        "reasoning" => {
                            let t = p.get("text").and_then(|t| t.as_str()).unwrap_or("");
                            if !t.trim().is_empty() {
                                out.push(Msg::new("assistant", &ts, clip(t, 6000), "reasoning"));
                            }
                        }
                        "tool" => {
                            let name = p.get("name").and_then(|t| t.as_str()).unwrap_or("tool").to_string();
                            let cid = p.get("id").and_then(|t| t.as_str()).map(String::from);
                            let mut tool = Msg::new("assistant", &ts, format!("{}:", name), "tool");
                            tool.name = Some(name);
                            tool.id = cid.clone();
                            let state = p.get("state").cloned().unwrap_or(Value::Null);
                            tool.error = Some(matches!(state.get("status").and_then(Value::as_str), Some("error" | "failed")));
                            tool.data = state.get("input").cloned(); { let inp = tool.data.clone(); crate::providers::annotate(&mut tool, inp.as_ref()); }
                            out.push(tool);
                            if let Some(o) = opencode_v2_tool_output(&state) {
                                let mut res = Msg::new("assistant", &ts, clip(&o, 8000), "result");
                                res.id = cid;
                                out.push(res);
                            }
                            for key in ["content", "attachments"] {
                                out.extend(state.get(key).and_then(Value::as_array).into_iter().flatten()
                                    .filter_map(|part| attachment_message("assistant", &ts, part)));
                            }
                        }
                        _ => {}
                    }
                }
            }
            // Agent, model and folder switches, system notes, skills and idle markers are not part of the conversation.
            _ => {}
        }
    }
    Some(out)
}

fn opencode_messages(key: &str) -> Vec<Msg> {
    let (db,id)=if let Some((path,id))=key.rsplit_once('|') {
        let db=PathBuf::from(path);
        if !opencode_dbs().contains(&db) {return Vec::new();}
        (db,id)
    } else { let Some(db)=opencode_db() else{return Vec::new()};(db,key) };
    let Some(c) = open_ro(&db) else { return Vec::new() };
    if let Some(v2) = opencode_v2_messages(&c, id) {
        return v2;
    }
    if !has_table(&c, "message") {
        return Vec::new();
    }
    let Ok(mut st) = c.prepare("SELECT m.id, m.time_created, m.data FROM message m WHERE m.session_id=?1 ORDER BY m.time_created") else { return Vec::new() };
    let msgs: Vec<(String, i64, String)> = st.query_map([id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).map(|r| r.flatten().collect()).unwrap_or_default();
    let mut out = Vec::new();
    for (mid, created, data) in msgs {
        let role = serde_json::from_str::<Value>(&data).ok().and_then(|v| v.get("role").and_then(|r| r.as_str()).map(String::from)).unwrap_or_else(|| "assistant".into());
        let role = if role == "user" { "user" } else { "assistant" };
        let ts = chrono::DateTime::from_timestamp_millis(created).map(|d| d.to_rfc3339()).unwrap_or_default();
        let Ok(mut ps) = c.prepare("SELECT data FROM part WHERE message_id=?1 ORDER BY time_created") else { continue };
        let parts: Vec<String> = ps.query_map([&mid], |r| r.get(0)).map(|r| r.flatten().collect()).unwrap_or_default();
        for pd in parts {
            let Ok(p) = serde_json::from_str::<Value>(&pd) else { continue };
            match p.get("type").and_then(|t| t.as_str()).unwrap_or("") {
                "text" => {
                    if p.get("synthetic").and_then(|s| s.as_bool()) == Some(true) {
                        continue;
                    }
                    let t = p.get("text").and_then(|t| t.as_str()).unwrap_or("");
                    if !t.trim().is_empty() {
                        out.push(Msg::new(role, &ts, t.to_string(), "text"));
                    }
                }
                "reasoning" => {
                    let t = p.get("text").and_then(|t| t.as_str()).unwrap_or("");
                    if !t.trim().is_empty() {
                        out.push(Msg::new("assistant", &ts, clip(t, 6000), "reasoning"));
                    }
                }
                "tool" => {
                    let name = p.get("tool").and_then(|t| t.as_str()).unwrap_or("tool").to_string();
                    let cid = p.get("callID").and_then(|t| t.as_str()).map(String::from);
                    let mut tool = Msg::new("assistant", &ts, format!("{}:", name), "tool");
                    tool.name = Some(name);
                    tool.id = cid.clone();
                    tool.data = p.get("state").and_then(|s| s.get("input")).cloned(); { let inp = tool.data.clone(); crate::providers::annotate(&mut tool, inp.as_ref()); }
                    out.push(tool);
                    if let Some(o) = p.get("state").and_then(|s| s.get("output")).and_then(|o| o.as_str()).filter(|o| !o.trim().is_empty()) {
                        let mut res = Msg::new("assistant", &ts, clip(o, 8000), "result");
                        res.id = cid;
                        // OpenCode marks a failed tool with status "error".
                        if p.get("state").and_then(|s| s.get("status")).and_then(|s| s.as_str()) == Some("error") {
                            res.error = Some(true);
                        }
                        out.push(res);
                    } else if let Some(e) = p.get("state").filter(|s| s.get("status").and_then(|x| x.as_str()) == Some("error")).and_then(|s| s.get("error")).and_then(|e| e.as_str()) {
                        let mut res = Msg::new("assistant", &ts, clip(e, 8000), "result");
                        res.id = cid;
                        res.error = Some(true);
                        out.push(res);
                    }
                }
                "file" | "image" => {
                    if let Some(media) = attachment_message(role, &ts, &p) { out.push(media); }
                }
                "compaction" => out.push(crate::providers::notice(&ts, "compact", "Conversation compacted", false)),
                // OpenCode's own bookkeeping, not shown in its app either.
                "step-start" | "step-finish" | "snapshot" | "patch" | "agent" | "retry" => {}
                // A part type added later: shown as a note with its text, never dropped.
                other => {
                    let t = crate::providers::text_of(&p);
                    if !t.is_empty() {
                        out.push(crate::providers::notice(&ts, other, &t, false));
                    }
                }
            }
        }
    }
    out
}

// ---------------- Cline ----------------

/// The folders Cline keeps its tasks in: its own command line, and the extension inside each editor.
/// The Google account Gemini CLI is signed in with (its list of accounts holds emails only, no tokens).
pub fn gemini_account() -> Option<String> {
    let f = home()?.join(".gemini/google_accounts.json");
    let v: Value = serde_json::from_str(&fs::read_to_string(f).ok()?).ok()?;
    v.get("active")?.as_str().filter(|e| e.contains('@')).map(String::from)
}

pub fn cline_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(h) = home() {
        roots.push(h.join(".cline/data/sessions"));
    }
    if let Some(app) = env_path("APPDATA") {
        for editor in ["Code", "Code - Insiders", "Cursor", "Windsurf", "VSCodium"] {
            let user = app.join(editor).join("User");
            roots.push(user.join("globalStorage/saoudrizwan.claude-dev/tasks"));
            for profile in subdirs(&user.join("profiles")) {
                roots.push(profile.join("globalStorage/saoudrizwan.claude-dev/tasks"));
            }
        }
    }
    roots
}

pub fn gemini_chat_files() -> Vec<PathBuf> {
    gemini_files()
}

pub fn cline_chat_files() -> Vec<PathBuf> {
    cline_sessions()
}

/// How many chats an OpenCode database holds.
pub fn opencode_chat_count(db: &Path) -> Option<usize> {
    open_ro(db)?;
    let mut chats = Vec::new();
    scan_opencode_db(db, &mut chats);
    Some(chats.len())
}

fn cline_sessions() -> Vec<PathBuf> {
    let mut out = Vec::new();
    let roots = cline_roots();
    for root in roots {
    let Ok(rd) = fs::read_dir(&root) else { continue };
    for d in rd.flatten() {
        if let Ok(files) = fs::read_dir(d.path()) {
            for f in files.flatten() {
                let p = f.path();
                if p.file_name().map(|n| n.to_string_lossy().ends_with(".messages.json") || n=="api_conversation_history.json").unwrap_or(false) {
                    out.push(p);
                }
            }
        }
    }
    }
    out
}

/// Cline stores each message's content as a list of blocks (text, thinking, tool calls and tool results).
fn blocks(v: &Value) -> Vec<Value> {
    match v {
        Value::Array(a) => a.clone(),
        Value::String(s) => serde_json::from_str::<Value>(s).ok().and_then(|x| x.as_array().cloned()).unwrap_or_else(|| vec![serde_json::json!({"type":"text","text":s})]),
        _ => Vec::new(),
    }
}

fn clean_cline_text(t: &str) -> String {
    let t = t.trim();
    // user turns are wrapped like <user_input mode="act">…</user_input>
    if let Some(a) = t.find("<user_input") {
        if let Some(open_end) = t[a..].find('>') {
            let inner = &t[a + open_end + 1..];
            return inner.split("</user_input>").next().unwrap_or(inner).trim().to_string();
        }
    }
    t.to_string()
}

fn cline_messages(p: &Path) -> Vec<Msg> {
    let Some(j) = read_json(p) else { return Vec::new() };
    let mut out = Vec::new();
    for m in j.as_array().or_else(||j.get("messages").and_then(|m|m.as_array())).cloned().unwrap_or_default() {
        let role = if m.get("role").and_then(|r| r.as_str()) == Some("user") { "user" } else { "assistant" };
        let ts = m.get("ts").and_then(|t| t.as_str().map(String::from).or_else(|| t.as_i64().map(|n| n.to_string()))).and_then(|s| s.parse::<i64>().ok()).and_then(chrono::DateTime::from_timestamp_millis).map(|d| d.to_rfc3339()).unwrap_or_default();
        for b in blocks(m.get("content").unwrap_or(&Value::Null)) {
            if let Some(media) = attachment_message(role, &ts, &b) {
                out.push(media);
                continue;
            }
            match b.get("type").and_then(|t| t.as_str()).unwrap_or("") {
                "text" => {
                    let t = clean_cline_text(b.get("text").and_then(|t| t.as_str()).unwrap_or(""));
                    if !t.is_empty() {
                        out.push(Msg::new(role, &ts, t, "text"));
                    }
                }
                "thinking" => {
                    let t = b.get("thinking").and_then(|t| t.as_str()).unwrap_or("");
                    if !t.trim().is_empty() {
                        out.push(Msg::new("assistant", &ts, clip(t, 6000), "reasoning"));
                    }
                }
                "tool_use" => {
                    let name = b.get("name").and_then(|n| n.as_str()).unwrap_or("tool").to_string();
                    let mut tool = Msg::new("assistant", &ts, format!("{}:", name), "tool");
                    tool.name = Some(name);
                    tool.id = b.get("id").and_then(|n| n.as_str()).map(String::from);
                    tool.data = b.get("input").cloned(); { let inp = tool.data.clone(); crate::providers::annotate(&mut tool, inp.as_ref()); }
                    out.push(tool);
                }
                "tool_result" => {
                    let raw = b.get("content").cloned().unwrap_or(Value::Null);
                    let text = match &raw {
                        Value::String(s) => s.clone(),
                        Value::Array(a) => a.iter().filter_map(|x| x.get("text").and_then(|t| t.as_str())).collect::<Vec<_>>().join("\n"),
                        other => other.to_string(),
                    };
                    if !text.trim().is_empty() {
                        let mut res = Msg::new("assistant", &ts, clip(&text, 8000), "result");
                        res.id = b.get("tool_use_id").and_then(|n| n.as_str()).map(String::from);
                        res.error = b.get("is_error").and_then(Value::as_bool);
                        out.push(res);
                    }
                    out.extend(raw.as_array().into_iter().flatten().filter_map(|part| attachment_message("assistant", &ts, part)));
                }
                _ => {}
            }
        }
    }
    out
}

fn scan_cline(out: &mut Vec<OtherChat>) {
    for p in cline_sessions() {
        let msgs = cline_messages(&p);
        let Some(first) = msgs.iter().find(|m| m.role == "user") else { continue };
        let meta = if p.file_name().is_some_and(|n|n=="api_conversation_history.json") {p.with_file_name("task_metadata.json")} else {p.with_file_name(p.file_name().map(|n| n.to_string_lossy().replace(".messages.json", ".json")).unwrap_or_default())};
        let mj = read_json(&meta);
        let started = mj.as_ref().and_then(|m| m.get("started_at")).and_then(|s| s.as_str()).and_then(ms_of);
        let ended = mj.as_ref().and_then(|m| m.get("ended_at")).and_then(|s| s.as_str()).and_then(ms_of);
        let id = p.parent().and_then(|d| d.file_name()).map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        out.push(OtherChat {
            provider: "cline",
            id,
            title: clip(first.text.lines().find(|l| !l.trim().is_empty()).unwrap_or("Chat").trim(), 80),
            cwd: mj.as_ref().and_then(|m| m.get("cwd")).and_then(|s| s.as_str()).map(String::from),
            created_ms: started,
            last_ms: ended.or(started).unwrap_or_else(|| mtime(&p)),
            size: fs::metadata(&p).map(|m| m.len() as i64).unwrap_or(0),
            path: format!("cline:{}", p.to_string_lossy()),
            model: mj.as_ref().and_then(|m| m.get("model")).and_then(|s| s.as_str()).map(String::from),
        });
    }
}

// ---------------- shared ----------------

pub fn scan() -> Vec<OtherChat> {
    let mut out = Vec::new();
    scan_gemini(&mut out);
    scan_opencode(&mut out);
    scan_cline(&mut out);
    out
}

/// Changes whenever any of these tools writes a chat, so the index can skip a rescan when nothing changed.
pub fn fingerprint() -> i64 {
    let mut m = 0;
    for p in gemini_files() {
        m = m.max(mtime(&p));
    }
    for p in cline_sessions() {
        m = m.max(mtime(&p));
    }
    for db in opencode_dbs() {
        m = m.max(mtime(&db));
        let mut wal = db.into_os_string();
        wal.push("-wal");
        m = m.max(mtime(Path::new(&wal)));
    }
    m
}

pub fn read(path: &str) -> Result<Vec<Msg>, String> {
    if let Some(p) = path.strip_prefix("gemini:") {
        return Ok(gemini_session(Path::new(p)).as_ref().map(gemini_messages).unwrap_or_default());
    }
    if let Some(id) = path.strip_prefix("opencode:") {
        return Ok(opencode_messages(id));
    }
    if let Some(p) = path.strip_prefix("cline:") {
        return Ok(cline_messages(Path::new(p)));
    }
    Err("Unknown chat".into())
}

pub fn is_virtual(path: &str) -> bool {
    TOOLS.iter().any(|t| path.starts_with(&format!("{t}:")))
}

/// What deleting a Gemini or Cline chat removes: only a file or folder that the scan itself lists, so a path
/// from outside can never point this at anything else. A Cline task is a folder, so the whole folder goes.
pub fn deletable(path: &str) -> Option<PathBuf> {
    let same = |a: &Path, b: &Path| matches!((a.canonicalize(), b.canonicalize()), (Ok(x), Ok(y)) if x == y);
    if let Some(p) = path.strip_prefix("gemini:") {
        let p = PathBuf::from(p);
        return gemini_files().into_iter().find(|f| same(f, &p)).map(|_| p);
    }
    if let Some(p) = path.strip_prefix("cline:") {
        let p = PathBuf::from(p);
        let listed = cline_sessions().into_iter().find(|f| same(f, &p))?;
        let task = listed.parent().filter(|d| listed.file_name().is_some_and(|n| n == "api_conversation_history.json") && d.parent().and_then(|t| t.file_name()).is_some_and(|n| n == "tasks"));
        return Some(task.map(Path::to_path_buf).unwrap_or(listed));
    }
    None
}

/// The real file behind a virtual path, for "show file".
pub fn real_file(path: &str) -> Option<PathBuf> {
    if let Some(p) = path.strip_prefix("gemini:").or_else(|| path.strip_prefix("cline:")) {
        let p = PathBuf::from(p);
        return p.exists().then_some(p);
    }
    if path.starts_with("opencode:") {
        if let Some((db,_))=path.strip_prefix("opencode:")?.rsplit_once('|') {
            return opencode_dbs().into_iter().find(|p|p==Path::new(db));
        }
        return opencode_db();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn extension_and_cli_cline_transcripts_both_parse() {
        let sample=serde_json::json!([{"role":"user","content":[{"type":"text","text":"fixture question"}]},{"role":"assistant","content":[{"type":"text","text":"fixture answer"}]}]);
        for value in [sample.clone(),serde_json::json!({"messages":sample})] {
            let path=crate::cli::script_file("").unwrap().with_extension("json");
            fs::write(&path,value.to_string()).unwrap();
            let messages=cline_messages(&path);
            assert_eq!(messages.len(),2);assert_eq!(messages[0].role,"user");assert_eq!(messages[1].text,"fixture answer");
        }
    }
    #[test]
    fn database_paths_with_unicode_spaces_hash_and_percent_are_read_only() {
        let dir=crate::cli::script_file("").unwrap().with_extension("fixture");fs::create_dir_all(&dir).unwrap();
        let db=dir.join("other user's 非ASCII #100%.db");
        {let c=Connection::open(&db).unwrap();c.execute_batch("CREATE TABLE fixture(value TEXT); INSERT INTO fixture VALUES('readable')").unwrap();}
        let c=open_ro(&db).unwrap();
        assert_eq!(c.query_row("SELECT value FROM fixture",[],|r|r.get::<_,String>(0)).unwrap(),"readable");
        assert!(c.execute("INSERT INTO fixture VALUES('no write')",[]).is_err());
    }
    #[test]
    fn only_chats_the_scan_lists_can_be_deleted() {
        let root = crate::cli::script_file("").unwrap().with_extension("geminihome");
        let chats = root.join(".gemini/tmp/project/chats");
        fs::create_dir_all(&chats).unwrap();
        let listed = chats.join("session-1.json");
        fs::write(&listed, r#"{"sessionId":"s1","messages":[{"type":"user","content":"hello"}]}"#).unwrap();
        let stranger = root.join("not-a-chat.json");
        fs::write(&stranger, "{}").unwrap();
        // SAFETY: only this test reads GEMINI_CLI_HOME, and it sets and clears it within the test.
        unsafe { std::env::set_var("GEMINI_CLI_HOME", &root) };
        let found = deletable(&format!("gemini:{}", listed.display()));
        assert_eq!(found.map(|p| p.canonicalize().unwrap()), Some(listed.canonicalize().unwrap()));
        assert!(deletable(&format!("gemini:{}", stranger.display())).is_none(), "a file the scan does not list must be refused");
        assert!(deletable(&format!("gemini:{}", chats.join("..").join("..").join("..").join("not-a-chat.json").display())).is_none());
        assert!(deletable("opencode:whatever|id").is_none());
        unsafe { std::env::remove_var("GEMINI_CLI_HOME") };
        let _ = fs::remove_dir_all(&root);
    }
    /// An OpenCode 2.x database (session_v2 + session_message + session_part) and a 1.x chat in the same file, as left
    /// by an upgrade: both are listed, and 2.x answers come from session_part or, in older 2.x builds, from `content`.
    #[test]
    fn opencode_v2_and_v1_chats_are_listed_and_read() {
        let dir = crate::cli::script_file("").unwrap().with_extension("opencode2");
        fs::create_dir_all(&dir).unwrap();
        let db = dir.join("opencode.db");
        let _ = fs::remove_file(&db);
        {
            let c = Connection::open(&db).unwrap();
            c.execute_batch(r#"
                CREATE TABLE session (id TEXT PRIMARY KEY, title TEXT NOT NULL, directory TEXT NOT NULL, slug TEXT NOT NULL, time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL);
                CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT NOT NULL, time_created INTEGER NOT NULL, data TEXT NOT NULL);
                CREATE TABLE part (id TEXT PRIMARY KEY, message_id TEXT NOT NULL, time_created INTEGER NOT NULL, data TEXT NOT NULL);
                CREATE TABLE session_v2 (id TEXT PRIMARY KEY, title TEXT, directory TEXT NOT NULL, slug TEXT NOT NULL, time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL);
                CREATE TABLE session_message (id TEXT PRIMARY KEY, session_id TEXT NOT NULL, type TEXT NOT NULL, seq INTEGER NOT NULL, time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL, data TEXT NOT NULL);
                CREATE TABLE session_part (message_id TEXT NOT NULL, session_id TEXT NOT NULL, ordinal INTEGER NOT NULL, type TEXT NOT NULL, call_id TEXT, time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL, data TEXT NOT NULL);
                INSERT INTO session VALUES('ses_old','Old chat','C:\proj','old',1000,2000);
                INSERT INTO message VALUES('msg_o1','ses_old',1000,'{"role":"user"}');
                INSERT INTO part VALUES('prt_o1','msg_o1',1000,'{"type":"text","text":"old question"}');
                INSERT INTO session_v2 VALUES('ses_new',NULL,'C:\proj','brave-otter',3000,4000);
                INSERT INTO session_message VALUES('msg_a','ses_new','model-switched',1,3000,3000,'{"time":{"created":3000}}');
                INSERT INTO session_message VALUES('msg_b','ses_new','user',2,3001,3001,'{"text":"How do I add a route?","time":{"created":3001}}');
                INSERT INTO session_message VALUES('msg_c','ses_new','assistant',3,3002,3002,'{"agent":"build","time":{"created":3002}}');
                INSERT INTO session_part VALUES('msg_c','ses_new',0,'reasoning',NULL,3002,3002,'{"type":"reasoning","text":"thinking"}');
                INSERT INTO session_part VALUES('msg_c','ses_new',1,'tool','call_1',3002,3002,'{"type":"tool","id":"call_1","name":"read","state":{"status":"completed","input":{"path":"routes.ts"},"content":[{"type":"text","text":"file body"}]},"time":{"created":3002}}');
                INSERT INTO session_part VALUES('msg_c','ses_new',2,'text',NULL,3002,3002,'{"type":"text","text":"Add it in routes.ts."}');
                INSERT INTO session_message VALUES('msg_d','ses_new','user',4,3003,3003,'{"text":"Thanks","time":{"created":3003}}');
                INSERT INTO session_message VALUES('msg_e','ses_new','assistant',5,3004,3004,'{"agent":"build","content":[{"type":"text","text":"You are welcome."}],"time":{"created":3004}}');
            "#).unwrap();
        }
        let mut chats = Vec::new();
        scan_opencode_db(&db, &mut chats);
        let titles: Vec<_> = chats.iter().map(|c| (c.id.as_str(), c.title.as_str())).collect();
        assert!(titles.contains(&("ses_new", "How do I add a route?")), "{titles:?}");
        assert!(titles.contains(&("ses_old", "Old chat")), "{titles:?}");
        let c = open_ro(&db).unwrap();
        let msgs = opencode_v2_messages(&c, "ses_new").unwrap();
        let kinds: Vec<_> = msgs.iter().map(|m| (m.role.as_str(), m.kind.as_str(), m.text.as_str())).collect();
        assert_eq!(
            kinds,
            vec![
                ("user", "text", "How do I add a route?"),
                ("assistant", "reasoning", "thinking"),
                ("assistant", "tool", "read:"),
                ("assistant", "result", "file body"),
                ("assistant", "text", "Add it in routes.ts."),
                ("user", "text", "Thanks"),
                ("assistant", "text", "You are welcome."),
            ]
        );
        assert!(opencode_v2_messages(&c, "ses_old").is_none(), "a 1.x chat falls back to the 1.x reader");
        drop(c);
        let _ = fs::remove_dir_all(&dir);
    }
    #[test]
    #[ignore="read-only inspection of local provider stores"]
    fn real_provider_stores() {
        let chats=scan();
        for tool in TOOLS {
            let selected:Vec<_>=chats.iter().filter(|c|c.provider==tool).collect();
            println!("{tool}: {} readable chats",selected.len());
            for chat in selected {assert!(!read(&chat.path).unwrap().is_empty());}
        }
    }
}
