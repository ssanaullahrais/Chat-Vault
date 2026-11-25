//! SQLite index of every chat found on disk, so listing and searching stay fast
//! even with a very large number of chats. Only new or changed files are re-read.

use crate::paths::*;
use crate::providers::*;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use crate::relevant;
use tauri::{AppHandle, Emitter};

static BODIES_BUSY: AtomicBool = AtomicBool::new(false);
/// A change arrived while the search index was being updated: run once more when it finishes, so it is not lost.
static BODIES_AGAIN: AtomicBool = AtomicBool::new(false);
/// One refresh at a time. A refresh asked for while another runs waits for it and then runs too, so a change that
/// landed during the first one is still picked up (it used to be skipped, which left new chats and renames missing).
static REFRESH_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ChatSummary {
    pub provider: String,
    pub id: String,
    pub local_id: Option<String>,
    pub title: String,
    pub account: String,
    pub project: String,
    pub cwd: Option<String>,
    pub created_ms: Option<i64>,
    pub last_ms: i64,
    pub archived: bool,
    pub size: i64,
    pub path: String,
    pub model: Option<String>,
    pub has_metadata: bool,
    pub backup: bool,
    /// Moved to the archive inside ChatVault. Nothing outside ChatVault changes.
    pub user_archived: bool,
}

#[derive(Serialize)]
pub struct Page {
    pub items: Vec<ChatSummary>,
    pub total: i64,
}

#[derive(Serialize)]
pub struct AccountCount {
    pub id: String,
    pub count: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Overview {
    pub total: i64,
    pub accounts: Vec<AccountCount>,
    pub latest_ms: Option<i64>,
}

#[derive(Serialize, Clone)]
struct Progress {
    phase: String,
    done: usize,
    total: usize,
}

pub fn open_db() -> Result<Connection, String> {
    let dir = data_dir().ok_or("No data folder")?;
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let c = Connection::open(dir.join("index.db")).map_err(|e| e.to_string())?;
    c.execute_batch(
        "PRAGMA journal_mode=WAL;
         PRAGMA synchronous=NORMAL;
         CREATE TABLE IF NOT EXISTS chats(
           provider TEXT NOT NULL, id TEXT NOT NULL, local_id TEXT, title TEXT NOT NULL,
           account TEXT NOT NULL, project TEXT NOT NULL, cwd TEXT, created_ms INTEGER,
           last_ms INTEGER NOT NULL, archived INTEGER NOT NULL DEFAULT 0, size INTEGER NOT NULL,
           mtime INTEGER NOT NULL, path TEXT NOT NULL, model TEXT, has_meta INTEGER NOT NULL DEFAULT 0,
           backup INTEGER NOT NULL DEFAULT 0, PRIMARY KEY(provider, id));
         CREATE TABLE IF NOT EXISTS user_archive(provider TEXT NOT NULL, id TEXT NOT NULL, PRIMARY KEY(provider, id));
         CREATE TABLE IF NOT EXISTS chat_tags(provider TEXT NOT NULL, id TEXT NOT NULL, tag TEXT NOT NULL, PRIMARY KEY(provider, id, tag));
         CREATE VIRTUAL TABLE IF NOT EXISTS chat_bodies USING fts5(key UNINDEXED, body, tokenize='unicode61 remove_diacritics 2');
         CREATE TABLE IF NOT EXISTS body_state(key TEXT PRIMARY KEY, size INTEGER NOT NULL, mtime INTEGER NOT NULL);
         CREATE TABLE IF NOT EXISTS source_state(name TEXT PRIMARY KEY, value TEXT NOT NULL);
         CREATE TABLE IF NOT EXISTS backed_up(provider TEXT NOT NULL, id TEXT NOT NULL, PRIMARY KEY(provider, id));
         CREATE INDEX IF NOT EXISTS chats_last ON chats(provider, last_ms DESC);
         CREATE INDEX IF NOT EXISTS chats_acct ON chats(provider, account, last_ms DESC);
         CREATE UNIQUE INDEX IF NOT EXISTS chats_path ON chats(path);
         CREATE VIRTUAL TABLE IF NOT EXISTS chats_fts USING fts5(key UNINDEXED, title, project, preview);",
    )
    .map_err(|e| e.to_string())?;
    // The name a chat was given inside its own tool (Claude Code's /rename), kept apart so the desktop app's
    // record does not overwrite a newer name.
    static ADD_CUSTOM_TITLE: std::sync::Once = std::sync::Once::new();
    ADD_CUSTOM_TITLE.call_once(|| {
        let has: bool = c
            .prepare("SELECT 1 FROM pragma_table_info('chats') WHERE name='custom_title'")
            .and_then(|mut s| s.exists([]))
            .unwrap_or(true);
        if !has && c.execute("ALTER TABLE chats ADD COLUMN custom_title TEXT", []).is_ok() {
            // Read every Claude transcript once more so chats renamed before this version get their name.
            let _ = c.execute("UPDATE chats SET mtime=-1 WHERE provider='claude'", []);
        }
    });
    Ok(c)
}

fn mtime_ms(p: &Path) -> (i64, i64) {
    fs::metadata(p)
        .map(|m| {
            let t = m
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);
            (m.len() as i64, t)
        })
        .unwrap_or((0, 0))
}

struct Found {
    provider: &'static str,
    path: PathBuf,
    backup: bool,
}

fn collect_files() -> Vec<Found> {
    let mut v = Vec::new();
    let mut add = |provider: &'static str, root: Option<PathBuf>, backup: bool, depth: u8| {
        let Some(root) = root else { return };
        let mut files = Vec::new();
        if depth == 1 {
            for d in subdirs(&root) {
                if let Ok(rd) = fs::read_dir(&d) {
                    for e in rd.flatten() {
                        let p = e.path();
                        if p.extension().map(|x| x == "jsonl").unwrap_or(false) {
                            files.push(p);
                        }
                    }
                }
            }
        } else {
            walk_files(&root, &["jsonl", "zst"], &mut files);
        }
        v.extend(files.into_iter().map(|path| Found { provider, path, backup }));
    };
    for root in claude_project_roots() { add("claude", Some(root), false, 1); }
    for root in codex_roots() {
        add("codex", Some(root.join("sessions")), false, 9);
        add("codex", Some(root.join("archived_sessions")), false, 9);
    }
    let b = backup_root();
    add("claude", b.as_ref().map(|b| b.join("claude").join("projects")), true, 1);
    add("codex", b.as_ref().map(|b| b.join("codex").join("sessions")), true, 9);
    add("codex", b.as_ref().map(|b| b.join("codex").join("archived_sessions")), true, 9);
    // A tool that is switched off is not looked at.
    v.retain(|f| crate::integrations::is_enabled(f.provider));
    v
}

struct ClaudeMeta {
    local_id: Option<String>,
    title: Option<String>,
    account: String,
    cwd: Option<String>,
    created: Option<i64>,
    last: Option<i64>,
    archived: bool,
    model: Option<String>,
}

fn load_claude_meta() -> HashMap<String, ClaudeMeta> {
    let mut out = HashMap::new();
    let roots = [claude_meta(), backup_root().map(|b| b.join("claude").join("sessions-meta"))];
    for root in roots.into_iter().flatten() {
        for acct in subdirs(&root) {
            let acct_name = acct.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            for org in subdirs(&acct) {
                let Ok(rd) = fs::read_dir(&org) else { continue };
                for f in rd.flatten() {
                    let p = f.path();
                    let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                    if !(name.starts_with("local_") && name.ends_with(".json")) {
                        continue;
                    }
                    let Ok(txt) = fs::read_to_string(&p) else { continue };
                    let Ok(v) = serde_json::from_str::<Value>(&txt) else { continue };
                    let Some(cli) = v.get("cliSessionId").and_then(|x| x.as_str()) else { continue };
                    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).map(String::from);
                    out.entry(cli.to_string()).or_insert(ClaudeMeta {
                        local_id: s("sessionId"),
                        title: s("title"),
                        account: acct_name.clone(),
                        cwd: s("cwd"),
                        created: v.get("createdAt").and_then(|x| x.as_i64()),
                        last: v.get("lastActivityAt").and_then(|x| x.as_i64()),
                        archived: v.get("isArchived").and_then(|x| x.as_bool()).unwrap_or(false),
                        model: s("model"),
                    });
                }
            }
        }
    }
    out
}

/// Codex thread names. The file is appended to whenever a thread is renamed, so the last line for an id is its current
/// name. The backup copy is read first and the live file last, so the live name wins.
fn load_codex_index() -> HashMap<String, (String, Option<i64>)> {
    let mut out = HashMap::new();
    let files = [
        backup_root().map(|b| b.join("codex").join("session_index.jsonl")),
        codex_home().map(|c| c.join("session_index.jsonl")),
    ];
    for f in files.into_iter().flatten() {
        let Ok(file) = File::open(f) else { continue };
        for line in BufReader::new(file).lines().flatten() {
            let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
            let (Some(id), Some(name)) = (
                v.get("id").and_then(|x| x.as_str()),
                v.get("thread_name").and_then(|x| x.as_str()),
            ) else {
                continue;
            };
            let ms = v
                .get("updated_at")
                .and_then(|x| x.as_str())
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|d| d.timestamp_millis());
            out.insert(id.to_string(), (name.to_string(), ms));
        }
    }
    out
}

/// The newest name given to a Claude Code chat with /rename (or by the desktop app), read from the transcript itself.
fn last_custom_title(path: &Path) -> Option<String> {
    let mut found = None;
    for line in open_lines(path)? {
        if !line.contains("\"custom-title\"") {
            continue;
        }
        if let Ok(v) = serde_json::from_str::<Value>(&line) {
            if v.get("type").and_then(|x| x.as_str()) == Some("custom-title") {
                if let Some(t) = v.get("customTitle").and_then(|x| x.as_str()).map(str::trim).filter(|t| !t.is_empty()) {
                    found = Some(t.to_string());
                }
            }
        }
    }
    found
}

fn parse_iso(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s).ok().map(|d| d.timestamp_millis())
}

/// Claude names a project folder after the path with every separator turned into "-". When the chat itself does not say
/// where it ran, make that folder name readable instead of showing the whole encoded path.
fn friendly_dir(name: &str) -> String {
    if name.contains("Claude-scratch") || name.contains("scratch-workspaces") {
        return "Claude scratch chat".to_string();
    }
    let mut rest = name;
    if let Some(i) = name.find("-Users-") {
        let after = &name[i + "-Users-".len()..];
        // drop the user name too
        rest = after.split_once('-').map(|(_, r)| r).unwrap_or(after);
    }
    let rest = rest.replace("---", " - ");
    if rest.is_empty() { name.to_string() } else { rest }
}

fn project_of(cwd: &Option<String>, fallback: &str) -> String {
    cwd.as_ref()
        .and_then(|c| Path::new(c).file_name())
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| friendly_dir(fallback))
}

struct Row {
    s: ChatSummary,
    mtime: i64,
    preview: String,
    /// The name the chat was given in its own tool, when it has one.
    custom_title: Option<String>,
}

fn first_user_text(provider: &str, path: &Path, max_lines: usize) -> Option<String> {
    for line in open_lines(path)?.take(max_lines) {
        if !line.contains("\"user\"") {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        if let Some(m) = messages_for(provider, &v).into_iter().find(|m| m.role == "user" && m.kind == "text") {
            return Some(clip(m.text.trim(), 300));
        }
    }
    None
}

/// The folder a Claude chat ran in, read from the transcript itself, for chats the desktop app has no record of.
fn transcript_cwd(path: &Path) -> Option<String> {
    for line in open_lines(path)?.take(80) {
        if !line.contains("\"cwd\"") {
            continue;
        }
        if let Ok(v) = serde_json::from_str::<Value>(&line) {
            if let Some(c) = v.get("cwd").and_then(|x| x.as_str()) {
                return Some(c.to_string());
            }
        }
    }
    None
}

fn build_row(
    f: &Found,
    size: i64,
    mtime: i64,
    cmeta: &HashMap<String, ClaudeMeta>,
    cindex: &HashMap<String, (String, Option<i64>)>,
) -> Option<Row> {
    let stem = f.path.file_stem()?.to_string_lossy().trim_end_matches(".jsonl").to_string();
    let path = f.path.to_string_lossy().to_string();
    if f.provider == "claude" {
        let dir_name = f.path.parent().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let preview = first_user_text("claude", &f.path, 400);
        let custom = last_custom_title(&f.path);
        // A live chat counts as active when its file last changed, even if the desktop app's record is older.
        let last_of = |recorded: Option<i64>| if f.backup { recorded.unwrap_or(mtime) } else { recorded.unwrap_or(0).max(mtime) };
        if let Some(m) = cmeta.get(&stem) {
            let title = custom.clone().or_else(|| m.title.clone()).unwrap_or_else(|| "Untitled".into());
            return Some(Row {
                s: ChatSummary {
                    provider: "claude".into(),
                    id: stem,
                    local_id: m.local_id.clone(),
                    title,
                    account: m.account.clone(),
                    project: project_of(&m.cwd, &dir_name),
                    cwd: m.cwd.clone(),
                    created_ms: m.created,
                    last_ms: last_of(m.last),
                    archived: m.archived,
                    size,
                    path,
                    model: m.model.clone(),
                    has_metadata: true,
                    backup: f.backup,
                    user_archived: false,
                },
                mtime,
                preview: preview.unwrap_or_default(),
                custom_title: custom,
            });
        }
        let title = match &custom {
            Some(t) => t.clone(),
            None => clip(preview.as_deref()?.trim(), 80),
        };
        let cwd = transcript_cwd(&f.path);
        return Some(Row {
            s: ChatSummary {
                provider: "claude".into(),
                id: stem,
                local_id: None,
                title,
                account: "unknown".into(),
                project: project_of(&cwd, &friendly_dir(&dir_name)),
                cwd,
                created_ms: None,
                last_ms: mtime,
                archived: false,
                size,
                path,
                model: None,
                has_metadata: false,
                backup: f.backup,
                    user_archived: false,
            },
            mtime,
            preview: preview.unwrap_or_default(),
            custom_title: custom,
        });
    }

    // Codex rollout: the first line is session_meta.
    let first = open_lines(&f.path)?.next()?;
    let v: Value = serde_json::from_str(&first).ok()?;
    let p = v.get("payload")?;
    let id = p
        .get("id")
        .and_then(|x| x.as_str())
        .map(String::from)
        .unwrap_or_else(|| stem.chars().rev().take(36).collect::<Vec<_>>().into_iter().rev().collect());
    let cwd = p.get("cwd").and_then(|x| x.as_str()).map(String::from);
    let account = p
        .get("creator_account_id")
        .and_then(|x| x.as_str())
        .unwrap_or("unknown")
        .to_string();
    let preview = first_user_text("codex", &f.path, 120);
    let (name, updated) = cindex.get(&id).cloned().unzip();
    let title = name
        .or_else(|| preview.as_ref().map(|t| clip(t.trim(), 80)))
        .unwrap_or_else(|| "Untitled".into());
    let archived = f.path.to_string_lossy().contains("archived_sessions");
    Some(Row {
        s: ChatSummary {
            provider: "codex".into(),
            id,
            local_id: None,
            title,
            account,
            project: project_of(&cwd, "unknown"),
            cwd,
            created_ms: p.get("timestamp").and_then(|x| x.as_str()).and_then(parse_iso),
            last_ms: if f.backup { updated.flatten().unwrap_or(mtime) } else { updated.flatten().unwrap_or(0).max(mtime) },
            archived,
            size,
            path,
            model: p.get("model_provider").and_then(|x| x.as_str()).map(String::from),
            has_metadata: true,
            backup: f.backup,
                    user_archived: false,
        },
        mtime,
        preview: preview.unwrap_or_default(),
        custom_title: None,
    })
}

fn upsert(c: &Connection, r: &Row, replace: bool) -> rusqlite::Result<()> {
    let s = &r.s;
    let key = format!("{}:{}", s.provider, s.id);
    if !replace {
        let exists: Option<String> = c
            .query_row("SELECT path FROM chats WHERE provider=?1 AND id=?2", params![s.provider, s.id], |r| r.get(0))
            .optional()?;
        if let Some(p) = exists {
            if Path::new(&p).exists() {
                return Ok(());
            }
        }
    }
    c.execute("DELETE FROM chats WHERE path=?1 AND NOT (provider=?2 AND id=?3)", params![s.path, s.provider, s.id])?;
    c.execute("DELETE FROM chats_fts WHERE key=?1", params![key])?;
    c.execute(
        "INSERT OR REPLACE INTO chats(provider,id,local_id,title,account,project,cwd,created_ms,last_ms,archived,size,mtime,path,model,has_meta,backup,custom_title)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17)",
        params![
            s.provider, s.id, s.local_id, s.title, s.account, s.project, s.cwd, s.created_ms, s.last_ms,
            s.archived as i64, s.size, r.mtime, s.path, s.model, s.has_metadata as i64, s.backup as i64, r.custom_title
        ],
    )?;
    c.execute(
        "INSERT INTO chats_fts(key,title,project,preview) VALUES(?1,?2,?3,?4)",
        params![key, s.title, s.project, r.preview],
    )?;
    Ok(())
}

/// Brings the index up to date. Only files whose size or modified time changed are read again.
/// How many chat files exist and when the newest changed, so new conversations can be spotted without scanning them.
pub fn files_fingerprint() -> (usize, i64) {
    let files = collect_files();
    let newest = files.iter().map(|f| mtime_ms(&f.path).1).max().unwrap_or(0);
    // Gemini CLI, OpenCode and Cline writes count as changes too, and so do renames: the Claude desktop app's chat
    // records and Codex's thread-name file change without the chat file itself changing.
    (files.len(), newest.max(crate::others::fingerprint()).max(crate::codex_db::fingerprint()).max(names_fingerprint()))
}

/// When a chat name was last changed outside ChatVault (Claude desktop app records, Codex thread names).
fn names_fingerprint() -> i64 {
    let mut newest = codex_home().map(|c| mtime_ms(&c.join("session_index.jsonl")).1).unwrap_or(0);
    if let Some(root) = claude_meta() {
        for acct in subdirs(&root) {
            for org in subdirs(&acct) {
                newest = newest.max(mtime_ms(&org).1);
                let Ok(rd) = fs::read_dir(&org) else { continue };
                for e in rd.flatten() {
                    if let Ok(t) = e.metadata().and_then(|m| m.modified()) {
                        if let Ok(d) = t.duration_since(std::time::UNIX_EPOCH) {
                            newest = newest.max(d.as_millis() as i64);
                        }
                    }
                }
            }
        }
    }
    newest
}

pub fn refresh(app: Option<&AppHandle>) -> Result<(), String> {
    let _guard = REFRESH_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let result = (|| {
        let mut c = open_db()?;
        let source_upgrade = c.query_row("SELECT value FROM source_state WHERE name='local_sources'",[],|r|r.get::<_,String>(0)).ok().as_deref()!=Some("4");
        // Rows saved with a raw encoded folder name are rebuilt once so they get the readable project name.
        let _ = c.execute("DELETE FROM chats WHERE provider='claude' AND project LIKE 'C--%'", []);
        // Chats without desktop-app records are rebuilt once so they pick up the folder found in the transcript.
        let _ = c.execute("DELETE FROM chats WHERE provider='claude' AND has_meta=0 AND cwd IS NULL", []);
        let files = collect_files();
        let known: HashMap<String, (i64, i64)> = {
            let mut st = c.prepare("SELECT path,size,mtime FROM chats").map_err(|e| e.to_string())?;
            let rows = st
                .query_map([], |r| Ok((r.get::<_, String>(0)?, (r.get::<_, i64>(1)?, r.get::<_, i64>(2)?))))
                .map_err(|e| e.to_string())?;
            rows.flatten().collect()
        };
        let seen: HashSet<String> = files.iter().map(|f| f.path.to_string_lossy().to_string()).collect();
        let cmeta = load_claude_meta();
        let cindex = load_codex_index();
        let native_chats = if crate::integrations::is_enabled("codex") { crate::codex_db::scan() } else { Vec::new() };
        let native_ids:HashSet<String>=native_chats.iter().map(|ch|ch.id.clone()).collect();

        let tx = c.transaction().map_err(|e| e.to_string())?;
        for (path, _) in known.iter().filter(|(p, _)| !seen.contains(*p) && !p.starts_with("cursor-") && !p.starts_with("codex-db:") && !Path::new(p).exists()) {
            let _ = tx.execute(
                "DELETE FROM chats_fts WHERE key IN (SELECT provider||':'||id FROM chats WHERE path=?1)",
                params![path],
            );
            let _ = tx.execute("DELETE FROM chats WHERE path=?1", params![path]);
        }
        tx.commit().map_err(|e| e.to_string())?;

        // Remember which chats have a backup copy, so the Backup filter can list all of them, not only the ones whose original is gone.
        {
            let tx = c.transaction().map_err(|e| e.to_string())?;
            let _ = tx.execute("DELETE FROM backed_up", []);
            for f in files.iter().filter(|f| f.backup) {
                let stem = f.path.file_stem().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                let stem = stem.trim_end_matches(".jsonl").to_string();
                let id = if f.provider == "codex" && stem.len() >= 36 { stem[stem.len() - 36..].to_string() } else { stem };
                let _ = tx.execute("INSERT OR IGNORE INTO backed_up(provider,id) VALUES(?1,?2)", params![f.provider, id]);
            }
            // Cursor chats copied into the backup folder: transcripts (named by chat id) and CLI stores (folder named by chat id).
            if let Some(root) = backup_root().map(|b| b.join("cursor")) {
                fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
                    let Ok(rd) = fs::read_dir(dir) else { return };
                    for e in rd.flatten() {
                        let p = e.path();
                        if p.is_dir() { walk(&p, out) } else { out.push(p) }
                    }
                }
                let mut found = Vec::new();
                walk(&root, &mut found);
                for p in found {
                    let id = if p.extension().map(|e| e == "jsonl").unwrap_or(false) {
                        p.file_stem().map(|n| n.to_string_lossy().to_string())
                    } else if p.file_name().map(|n| n == "store.db").unwrap_or(false) {
                        p.parent().and_then(|d| d.file_name()).map(|n| n.to_string_lossy().to_string())
                    } else {
                        None
                    };
                    if let Some(id) = id {
                        let _ = tx.execute("INSERT OR IGNORE INTO backed_up(provider,id) VALUES('cursor',?1)", params![id]);
                    }
                }
            }
            tx.commit().map_err(|e| e.to_string())?;
        }

        // Live files first so a live copy always wins over its backup.
        let mut todo: Vec<&Found> = Vec::new();
        for f in &files {
            if f.provider=="codex" {
                let stem=f.path.file_stem().map(|s|s.to_string_lossy().trim_end_matches(".jsonl").to_string()).unwrap_or_default();
                if stem.len()>=36 && stem.get(stem.len()-36..).is_some_and(|id|native_ids.contains(id)) {continue;}
            }
            let (size, mtime) = mtime_ms(&f.path);
            let key = f.path.to_string_lossy().to_string();
            let unchanged = known.get(&key).map(|k| *k == (size, mtime)).unwrap_or(false);
            if !unchanged {
                todo.push(f);
            }
        }
        todo.sort_by_key(|f| f.backup);
        let total = todo.len();
        let mut tx = c.transaction().map_err(|e| e.to_string())?;
        for (i, f) in todo.iter().enumerate() {
            let (size, mtime) = mtime_ms(&f.path);
            if let Some(row) = build_row(f, size, mtime, &cmeta, &cindex) {
                let _ = upsert(&tx, &row, !f.backup);
            }
            if i % 200 == 199 {
                tx.commit().map_err(|e| e.to_string())?;
                tx = c.transaction().map_err(|e| e.to_string())?;
                if let Some(a) = app { let _ = a.emit("index-progress", Progress { phase: "Indexing".into(), done: i + 1, total }); }
            }
        }
        tx.commit().map_err(|e| e.to_string())?;

        // Current Codex history lives in SQLite, including chats whose old rollout is gone.
        // A live database entry wins over an older transcript/backup with the same id.
        let native_paths: HashSet<String> = native_chats.iter().map(|c|c.path.clone()).collect();
        let tx = c.transaction().map_err(|e|e.to_string())?;
        for path in known.keys().filter(|p|p.starts_with("codex-db:") && !native_paths.contains(*p)) {
            let _ = tx.execute("DELETE FROM chats_fts WHERE key IN (SELECT provider||':'||id FROM chats WHERE path=?1)",[path]);
            let _ = tx.execute("DELETE FROM chats WHERE path=?1",[path]);
        }
        for ch in native_chats {
            if !source_upgrade && known.get(&ch.path)==Some(&(0,ch.updated)) {continue;}
            let row = Row { s: ChatSummary { provider:"codex".into(),id:ch.id,local_id:None,title:ch.title.clone(),account:ch.account,
                project:project_of(&ch.cwd,"Codex"),cwd:ch.cwd,created_ms:Some(ch.created),last_ms:ch.updated,archived:ch.archived,
                size:0,path:ch.path,model:ch.model,has_metadata:true,backup:false,user_archived:false }, mtime:ch.updated,preview:ch.title,custom_title:None };
            let _ = upsert(&tx,&row,true);
        }
        tx.commit().map_err(|e|e.to_string())?;

        // Claude titles live in metadata files that change without the transcript changing. A name set inside the chat
        // itself (/rename) still wins, and a live chat stays as recent as its file.
        let tx = c.transaction().map_err(|e| e.to_string())?;
        for (id, m) in &cmeta {
            let before: Option<String> = tx.query_row("SELECT title FROM chats WHERE provider='claude' AND id=?1", params![id], |r| r.get(0)).optional().ok().flatten();
            let Some(before) = before else { continue };
            let _ = tx.execute(
                "UPDATE chats SET title=COALESCE(custom_title,?1,title), account=?2,
                   last_ms=MAX(COALESCE(?3,last_ms), CASE WHEN backup=0 THEN mtime ELSE 0 END),
                   archived=?4, local_id=COALESCE(?5,local_id), has_meta=1
                 WHERE provider='claude' AND id=?6",
                params![m.title, m.account, m.last, m.archived as i64, m.local_id, id],
            );
            let after: Option<String> = tx.query_row("SELECT title FROM chats WHERE provider='claude' AND id=?1", params![id], |r| r.get(0)).optional().ok().flatten();
            // The searchable copy of the title follows a rename too.
            if let Some(after) = after.filter(|a| *a != before) {
                let _ = tx.execute("UPDATE chats_fts SET title=?1 WHERE key='claude:'||?2", params![after, id]);
            }
        }
        // Codex names change in session_index.jsonl without the chat file changing (renaming a thread in Codex).
        for (id, (name, _)) in &cindex {
            let changed = tx
                .execute(
                    "UPDATE chats SET title=?1 WHERE provider='codex' AND id=?2 AND path NOT LIKE 'codex-db:%' AND title<>?1",
                    params![name, id],
                )
                .unwrap_or(0);
            if changed > 0 {
                let _ = tx.execute("UPDATE chats_fts SET title=?1 WHERE key='codex:'||?2", params![name, id]);
            }
        }
        tx.commit().map_err(|e| e.to_string())?;
                // Cursor chats are rebuilt only when Cursor's files changed.
        // Gemini CLI, OpenCode and Cline are rebuilt whenever one of their files changed.
        {
            let ofp = crate::others::fingerprint();
            let olast: i64 = c.query_row("SELECT COALESCE(MAX(mtime),0) FROM chats WHERE provider IN ('gemini','opencode','cline')", [], |r| r.get(0)).unwrap_or(0);
            if source_upgrade || (ofp > 0 && ofp != olast) {
                let chats: Vec<_> = crate::others::scan().into_iter().filter(|c| crate::integrations::is_enabled(c.provider)).collect();
                let tx = c.transaction().map_err(|e| e.to_string())?;
                let _ = tx.execute("DELETE FROM chats_fts WHERE key IN (SELECT provider||':'||id FROM chats WHERE provider IN ('gemini','opencode','cline'))", []);
                let _ = tx.execute("DELETE FROM chats WHERE provider IN ('gemini','opencode','cline')", []);
                for ch in chats {
                    let row = Row {
                        s: ChatSummary {
                            provider: ch.provider.into(),
                            id: ch.id,
                            local_id: None,
                            title: ch.title.clone(),
                            account: if ch.provider == "gemini" { crate::others::gemini_account().unwrap_or_else(|| "local".into()) } else { "local".into() },
                            project: project_of(&ch.cwd, ch.provider),
                            cwd: ch.cwd,
                            created_ms: ch.created_ms,
                            last_ms: ch.last_ms,
                            archived: false,
                            size: ch.size,
                            path: ch.path,
                            model: ch.model,
                            has_metadata: true,
                            backup: false,
                            user_archived: false,
                        },
                        mtime: ofp,
                        preview: ch.title,
                        custom_title: None,
                    };
                    let _ = upsert(&tx, &row, true);
                }
                tx.commit().map_err(|e| e.to_string())?;
            }
        }

        let fp = crate::cursor::fingerprint();
        let last: i64 = c.query_row("SELECT COALESCE(MAX(mtime),0) FROM chats WHERE provider='cursor'", [], |r| r.get(0)).unwrap_or(0);
        if source_upgrade || (fp > 0 && fp != last) {
            let chats = if crate::integrations::is_enabled("cursor") { crate::cursor::scan() } else { Vec::new() };
            let cursor_account = crate::cursor::account_email();
            let tx = c.transaction().map_err(|e| e.to_string())?;
            let _ = tx.execute("DELETE FROM chats_fts WHERE key IN (SELECT provider||':'||id FROM chats WHERE provider='cursor')", []);
            let _ = tx.execute("DELETE FROM chats WHERE provider='cursor'", []);
            for ch in chats {
                let row = Row {
                    s: ChatSummary {
                        provider: "cursor".into(),
                        id: ch.id,
                        local_id: None,
                        title: ch.title.clone(),
                        account: cursor_account.clone().unwrap_or_else(|| "local".into()),
                        project: project_of(&ch.cwd, "Cursor"),
                        cwd: ch.cwd,
                        created_ms: ch.created_ms,
                        last_ms: ch.last_ms,
                        archived: false,
                        size: ch.size,
                        path: ch.path,
                        model: None,
                        has_metadata: true,
                        backup: false,
                        user_archived: false,
                    },
                    mtime: fp,
                    preview: ch.title,
                    custom_title: None,
                };
                let _ = upsert(&tx, &row, true);
            }
            tx.commit().map_err(|e| e.to_string())?;
        }
        if let Some(a) = app { let _ = a.emit("index-progress", Progress { phase: "Done".into(), done: total, total }); }
        c.execute("INSERT OR REPLACE INTO source_state(name,value) VALUES('local_sources','4')",[]).map_err(|e|e.to_string())?;
        Ok(())
    })();
    result
}

/// The full-text expression for what was typed (words, "phrases", -left-out words, OR); see search.rs.
fn fts_expr(q: &str) -> Option<String> {
    crate::search::fts_expr(&crate::search::parse(q))
}

pub(crate) fn row_to_summary(r: &rusqlite::Row) -> rusqlite::Result<ChatSummary> {
    Ok(ChatSummary {
        provider: r.get(0)?,
        id: r.get(1)?,
        local_id: r.get(2)?,
        title: r.get(3)?,
        account: r.get(4)?,
        project: r.get(5)?,
        cwd: r.get(6)?,
        created_ms: r.get(7)?,
        last_ms: r.get(8)?,
        archived: r.get::<_, i64>(9)? != 0,
        size: r.get(10)?,
        path: r.get(11)?,
        model: r.get(12)?,
        has_metadata: r.get::<_, i64>(13)? != 0,
        backup: r.get::<_, i64>(14)? != 0,
        user_archived: r.get::<_, i64>(15)? != 0,
    })
}

pub(crate) const COLS: &str = "provider,id,local_id,title,account,project,cwd,created_ms,last_ms,archived,size,path,model,has_meta,backup,EXISTS(SELECT 1 FROM user_archive u WHERE u.provider=chats.provider AND u.id=chats.id)";

pub fn query(provider: &str, account: Option<String>, q: &str, offset: i64, limit: i64, sort: &str, scope: &str) -> Result<Page, String> {
    let c = open_db()?;
    let acct = account.filter(|a| a != "all");
    let like = format!("%{}%", q.trim().replace('%', "").replace('_', ""));
    let fts = fts_expr(q);
    let mut tag_sql = String::new();
    let scope_sql = match scope {
        "live" => " AND backup=0 AND NOT (archived=1 OR (provider||':'||id) IN (SELECT provider||':'||id FROM user_archive))",
        // Every chat that has a copy in the backup folder, whether or not the original still exists.
        "backup" => " AND (backup=1 OR (provider||':'||id) IN (SELECT provider||':'||id FROM backed_up))",
        "archived" => " AND (archived=1 OR (provider||':'||id) IN (SELECT provider||':'||id FROM user_archive))",
        t if t.starts_with("tag:") => {
            let tag: String = t[4..].chars().filter(|c| c.is_alphanumeric() || *c == ' ' || *c == '-').collect();
            tag_sql = format!(" AND (provider||':'||id) IN (SELECT provider||':'||id FROM chat_tags WHERE tag='{}')", tag.replace('\'', ""));
            tag_sql.as_str()
        }
        // The normal lists leave out chats you archived in ChatVault.
        _ => " AND (provider||':'||id) NOT IN (SELECT provider||':'||id FROM user_archive)",
    };
    let order_sql = match sort {
        "oldest" => "last_ms ASC",
        "title" => "title COLLATE NOCASE ASC",
        "size" => "size DESC",
        _ => "last_ms DESC",
    };
    let where_sql = format!("(?1='all' OR provider=?1) AND (?2 IS NULL OR account=?2) AND (?3 = '' OR title LIKE ?4 OR project LIKE ?4 OR (provider||':'||id) IN (SELECT key FROM chats_fts WHERE chats_fts MATCH ?5)){scope_sql}");
    let fts_param = fts.unwrap_or_else(|| "\"\"".into());
    let trimmed = q.trim().to_string();
    let total: i64 = c
        .query_row(&format!("SELECT COUNT(*) FROM chats WHERE {where_sql}"), params![provider, acct, trimmed, like, fts_param], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    let mut st = c
        .prepare(&format!("SELECT {COLS} FROM chats WHERE {where_sql} ORDER BY {order_sql} LIMIT ?6 OFFSET ?7"))
        .map_err(|e| e.to_string())?;
    let items = st
        .query_map(params![provider, acct, trimmed, like, fts_param, limit, offset], row_to_summary)
        .map_err(|e| e.to_string())?
        .flatten()
        .collect();
    Ok(Page { items, total })
}

pub fn overview(provider: &str) -> Result<Overview, String> {
    let c = open_db()?;
    let total: i64 = c.query_row("SELECT COUNT(*) FROM chats WHERE provider=?1", params![provider], |r| r.get(0)).map_err(|e| e.to_string())?;
    let latest_ms: Option<i64> = c.query_row("SELECT MAX(last_ms) FROM chats WHERE provider=?1", params![provider], |r| r.get(0)).map_err(|e| e.to_string())?;
    let mut st = c
        .prepare("SELECT account,COUNT(*) FROM chats WHERE provider=?1 GROUP BY account ORDER BY COUNT(*) DESC")
        .map_err(|e| e.to_string())?;
    let accounts = st
        .query_map(params![provider], |r| Ok(AccountCount { id: r.get(0)?, count: r.get(1)? }))
        .map_err(|e| e.to_string())?
        .flatten()
        .collect();
    Ok(Overview { total, accounts, latest_ms })
}

pub fn recent_paths(provider: &str, limit: i64) -> Result<Vec<(String, String)>, String> {
    let c = open_db()?;
    let mut st = c
        .prepare("SELECT id,path FROM chats WHERE provider=?1 ORDER BY last_ms DESC LIMIT ?2")
        .map_err(|e| e.to_string())?;
    let v = st
        .query_map(params![provider, limit], |r| Ok((r.get(0)?, r.get(1)?)))
        .map_err(|e| e.to_string())?
        .flatten()
        .collect();
    Ok(v)
}

pub fn by_ids(provider: &str, ids: &[String]) -> Result<Vec<ChatSummary>, String> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let c = open_db()?;
    let marks = vec!["?"; ids.len()].join(",");
    let sql = format!("SELECT {COLS} FROM chats WHERE provider=? AND id IN ({marks}) ORDER BY last_ms DESC");
    let mut st = c.prepare(&sql).map_err(|e| e.to_string())?;
    let mut args: Vec<&dyn rusqlite::ToSql> = vec![&provider];
    for i in ids {
        args.push(i);
    }
    let v = st
        .query_map(args.as_slice(), row_to_summary)
        .map_err(|e| e.to_string())?
        .flatten()
        .collect();
    Ok(v)
}

/// Drops a chat from the index after its file was removed.
/// Moves a chat to the archive (or back). The chat file and the other apps are never touched.
pub fn set_archived(provider: &str, id: &str, archived: bool) -> Result<(), String> {
    let c = open_db()?;
    if archived {
        c.execute("INSERT OR IGNORE INTO user_archive(provider,id) VALUES(?1,?2)", params![provider, id]).map_err(|e| e.to_string())?;
    } else {
        c.execute("DELETE FROM user_archive WHERE provider=?1 AND id=?2", params![provider, id]).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Your own labels for a chat, kept in ChatVault only.
pub fn set_tags(provider: &str, id: &str, tags: &[String]) -> Result<(), String> {
    let c = open_db()?;
    c.execute("DELETE FROM chat_tags WHERE provider=?1 AND id=?2", params![provider, id]).map_err(|e| e.to_string())?;
    for t in tags {
        let clean: String = t.chars().filter(|c| c.is_alphanumeric() || *c == ' ' || *c == '-').collect::<String>().trim().chars().take(30).collect();
        if !clean.is_empty() {
            c.execute("INSERT OR IGNORE INTO chat_tags(provider,id,tag) VALUES(?1,?2,?3)", params![provider, id, clean]).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

pub fn tags_of(provider: &str, id: &str) -> Result<Vec<String>, String> {
    let c = open_db()?;
    let mut st = c.prepare("SELECT tag FROM chat_tags WHERE provider=?1 AND id=?2 ORDER BY tag").map_err(|e| e.to_string())?;
    let v = st.query_map(params![provider, id], |r| r.get(0)).map_err(|e| e.to_string())?.flatten().collect();
    Ok(v)
}

/// Every tag with how many chats of this tool carry it.
pub fn all_tags(provider: &str) -> Result<Vec<(String, i64)>, String> {
    let c = open_db()?;
    let mut st = c.prepare("SELECT tag, COUNT(*) FROM chat_tags WHERE provider=?1 GROUP BY tag ORDER BY tag").map_err(|e| e.to_string())?;
    let v = st.query_map(params![provider], |r| Ok((r.get(0)?, r.get(1)?))).map_err(|e| e.to_string())?.flatten().collect();
    Ok(v)
}

/// Reads the words of every changed chat into a full-text index, so searching inside chats is instant.
/// It runs after a scan, a little at a time, and only touches chats that are new or changed.
pub fn index_bodies(app: Option<&AppHandle>) -> Result<(), String> {
    if BODIES_BUSY.swap(true, Ordering::SeqCst) {
        BODIES_AGAIN.store(true, Ordering::SeqCst);
        return Ok(());
    }
    let mut result = index_bodies_once(app);
    // Chats that changed while this ran are indexed now instead of waiting for the next change.
    while result.is_ok() && BODIES_AGAIN.swap(false, Ordering::SeqCst) {
        result = index_bodies_once(app);
    }
    BODIES_BUSY.store(false, Ordering::SeqCst);
    result
}

/// Text kept per chat in the search index. Long chats used to be cut at 400 KB, so anything said later in a long chat
/// (usually the newest part) could not be found.
const BODY_LIMIT: usize = 12_000_000;

fn index_bodies_once(app: Option<&AppHandle>) -> Result<(), String> {
    let result = (|| {
        let c = open_db()?;
        // Version 2 indexes whole chats: chats indexed by an older version are read again once, in the background.
        let version: Option<String> = c.query_row("SELECT value FROM source_state WHERE name='bodies_version'", [], |r| r.get(0)).optional().map_err(|e| e.to_string())?;
        if version.as_deref() != Some("2") {
            c.execute("DELETE FROM body_state", []).map_err(|e| e.to_string())?;
            c.execute("INSERT OR REPLACE INTO source_state(name,value) VALUES('bodies_version','2')", []).map_err(|e| e.to_string())?;
        }
        let todo: Vec<(String, String, String, i64, i64)> = {
            let mut st = c
                .prepare(
                    "SELECT c.provider, c.id, c.path, c.size, c.mtime FROM chats c
                     LEFT JOIN body_state b ON b.key = c.provider||':'||c.id
                     WHERE b.key IS NULL OR b.size != c.size OR b.mtime != c.mtime
                     ORDER BY c.last_ms DESC",
                )
                .map_err(|e| e.to_string())?;
            let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))).map_err(|e| e.to_string())?;
            rows.flatten().collect()
        };
        let total = todo.len();
        let mut tx = c.unchecked_transaction().map_err(|e| e.to_string())?;
        for (i, (provider, id, path, size, mtime)) in todo.iter().enumerate() {
            let key = format!("{provider}:{id}");
            let mut body = String::new();
            let mut msgs: Vec<Msg> = Vec::new();
            if provider == "codex" && path.starts_with("codex-db:") {
                msgs = crate::codex_db::read(path).unwrap_or_default();
            } else if provider == "cursor" {
                msgs = crate::cursor::read(path).unwrap_or_default();
            } else if matches!(provider.as_str(), "gemini" | "opencode" | "cline") {
                msgs = crate::others::read(path).unwrap_or_default();
            } else if let Some(lines) = open_lines(Path::new(path)) {
                for line in lines {
                    if !relevant(provider, &line) {
                        continue;
                    }
                    if let Ok(v) = serde_json::from_str::<Value>(&line) {
                        msgs.extend(messages_for(provider, &v));
                    }
                }
            }
            for m in msgs.iter().filter(|m| matches!(m.kind.as_str(), "text" | "summary" | "tool")) {
                if body.len() > BODY_LIMIT {
                    break;
                }
                // Tool steps add the command, file or search they ran (kept short), so those can be found too.
                if m.kind == "tool" {
                    body.push_str(&clip(m.text.trim(), 300));
                } else {
                    body.push_str(m.text.trim());
                }
                body.push('\n');
            }
            let _ = tx.execute("DELETE FROM chat_bodies WHERE key=?1", params![key]);
            if !body.is_empty() {
                let _ = tx.execute("INSERT INTO chat_bodies(key, body) VALUES(?1, ?2)", params![key, body]);
            }
            let _ = tx.execute("INSERT OR REPLACE INTO body_state(key,size,mtime) VALUES(?1,?2,?3)", params![key, size, mtime]);
            if (i + 1) % 40 == 0 {
                tx.commit().map_err(|e| e.to_string())?;
                tx = c.unchecked_transaction().map_err(|e| e.to_string())?;
                if let Some(a) = app {
                    let _ = a.emit("search-index-progress", (i + 1, total));
                }
            }
        }
        tx.commit().map_err(|e| e.to_string())?;
        // chats that no longer exist
        let _ = c.execute("DELETE FROM chat_bodies WHERE key NOT IN (SELECT provider||':'||id FROM chats)", []);
        let _ = c.execute("DELETE FROM body_state WHERE key NOT IN (SELECT provider||':'||id FROM chats)", []);
        if let Some(a) = app {
            let _ = a.emit("search-index-progress", (total, total));
        }
        Ok(())
    })();
    result
}

/// (chats with their text indexed, all chats): lets the page say when search is still catching up.
pub fn body_status() -> Result<(i64, i64), String> {
    let c = open_db()?;
    let indexed: i64 = c.query_row("SELECT COUNT(*) FROM body_state", [], |r| r.get(0)).unwrap_or(0);
    let total: i64 = c.query_row("SELECT COUNT(*) FROM chats", [], |r| r.get(0)).unwrap_or(0);
    Ok((indexed, total))
}

/// Instant search inside the text of every chat of one tool. Returns the chat id and a short snippet around the match.
pub fn search_bodies(provider: &str, q: &str, limit: i64) -> Result<Vec<(String, String)>, String> {
    let Some(expr) = fts_expr(q) else { return Ok(Vec::new()) };
    let c = open_db()?;
    let mut st = c
        .prepare(
            "SELECT substr(key, length(?1)+2), snippet(chat_bodies, 1, '', '', '…', 24)
             FROM chat_bodies WHERE chat_bodies MATCH ?2 AND key LIKE ?1||':%'
             ORDER BY rank LIMIT ?3",
        )
        .map_err(|e| e.to_string())?;
    let v = st
        .query_map(params![provider, expr, limit], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .map_err(|e| e.to_string())?
        .flatten()
        .collect();
    Ok(v)
}

/// Drops every chat whose file is inside `folder` from the list, for example after the backup folder moved.
pub fn forget_under(folder: &str) -> Result<(), String> {
    let c = open_db()?;
    let n = folder.chars().count() as i64;
    c.execute(
        "DELETE FROM chats_fts WHERE key IN (SELECT provider||':'||id FROM chats WHERE substr(path,1,?2)=?1)",
        params![folder, n],
    )
    .map_err(|e| e.to_string())?;
    c.execute("DELETE FROM chats WHERE substr(path,1,?2)=?1", params![folder, n]).map_err(|e| e.to_string())?;
    Ok(())
}

/// Removes a tool's chats from ChatVault's list (the list is only a copy of what the tool keeps; the tool's own chats
/// are not touched, and notes such as tags and pins are kept).
pub fn purge_provider(provider: &str) -> Result<(), String> {
    let c = open_db()?;
    c.execute("DELETE FROM chats_fts WHERE key IN (SELECT provider||':'||id FROM chats WHERE provider=?1)", params![provider]).map_err(|e| e.to_string())?;
    c.execute("DELETE FROM chats WHERE provider=?1", params![provider]).map_err(|e| e.to_string())?;
    Ok(())
}

/// Makes the next refresh read every source again, for example after a tool was switched back on.
pub fn mark_stale() -> Result<(), String> {
    let c = open_db()?;
    c.execute("INSERT OR REPLACE INTO source_state(name,value) VALUES('local_sources','0')", []).map_err(|e| e.to_string())?;
    Ok(())
}

pub fn forget(path: &str) -> Result<(), String> {
    let c = open_db()?;
    c.execute(
        "DELETE FROM chats_fts WHERE key IN (SELECT provider||':'||id FROM chats WHERE path=?1)",
        params![path],
    )
    .map_err(|e| e.to_string())?;
    c.execute("DELETE FROM chats WHERE path=?1", params![path]).map_err(|e| e.to_string())?;
    Ok(())
}

#[derive(Serialize)]
pub struct DayCount {
    pub day: String,
    pub provider: String,
    pub count: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectStat {
    pub name: String,
    pub count: i64,
    pub claude: i64,
    pub codex: i64,
    pub cursor: i64,
    pub last_ms: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Stats {
    pub daily: Vec<DayCount>,
    pub projects: Vec<ProjectStat>,
    pub size_claude: i64,
    pub size_codex: i64,
    /// Bytes per tool, for every tool that has chats.
    pub sizes: std::collections::HashMap<String, i64>,
}

/// Activity for the overview page: chats per day, busiest projects and disk use.
pub fn stats(days: i64) -> Result<Stats, String> {
    let c = open_db()?;
    let since = chrono_now_ms() - days * 86_400_000;
    let mut st = c
        .prepare("SELECT date(last_ms/1000,'unixepoch','localtime') d, provider, COUNT(*) FROM chats WHERE last_ms>=?1 GROUP BY d, provider")
        .map_err(|e| e.to_string())?;
    let daily = st
        .query_map(params![since], |r| Ok(DayCount { day: r.get(0)?, provider: r.get(1)?, count: r.get(2)? }))
        .map_err(|e| e.to_string())?
        .flatten()
        .collect();
    let mut st = c
        .prepare(
            "SELECT project, COUNT(*) n, SUM(provider='claude'), SUM(provider='codex'), SUM(provider='cursor'), MAX(last_ms)
             FROM chats GROUP BY project ORDER BY n DESC, project LIMIT 6",
        )
        .map_err(|e| e.to_string())?;
    let projects = st
        .query_map([], |r| {
            Ok(ProjectStat { name: r.get(0)?, count: r.get(1)?, claude: r.get(2)?, codex: r.get(3)?, cursor: r.get(4)?, last_ms: r.get(5)? })
        })
        .map_err(|e| e.to_string())?
        .flatten()
        .collect();
    let size = |p: &str| -> i64 {
        c.query_row("SELECT COALESCE(SUM(size),0) FROM chats WHERE provider=?1", params![p], |r| r.get(0)).unwrap_or(0)
    };
    let mut sizes = std::collections::HashMap::new();
    if let Ok(mut st) = c.prepare("SELECT provider, COALESCE(SUM(size),0) FROM chats GROUP BY provider") {
        if let Ok(rows) = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))) {
            sizes.extend(rows.flatten());
        }
    }
    Ok(Stats { daily, projects, size_claude: size("claude"), size_codex: size("codex"), sizes })
}

fn chrono_now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
