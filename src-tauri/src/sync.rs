//! Backs up the chats of Claude Code, Codex, Cursor, Gemini CLI, OpenCode and Cline into Documents, and restores them again.
//! Restoring never overwrites a file that already exists.

use crate::paths::*;
use serde::Serialize;
use serde_json::Value;
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{AppHandle, Emitter};

pub(crate) static BUSY: AtomicBool = AtomicBool::new(false);

#[derive(Serialize, Default)]
pub struct SyncStats {
    pub copied: usize,
    pub skipped: usize,
    pub failed: usize,
    pub bytes: u64,
    pub total: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupStatus {
    pub path: String,
    pub exists: bool,
    pub files: usize,
    pub bytes: u64,
    pub last_sync_ms: Option<i64>,
}

#[derive(Serialize, Clone)]
struct Progress {
    phase: String,
    done: usize,
    total: usize,
    current: String,
}

pub(crate) fn walk_all(root: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(root) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk_all(&p, out);
        } else {
            out.push(p);
        }
    }
}

/// Which files under Claude Code's projects folder are backed up: the chats (`project/chat.jsonl`) and what belongs to a
/// chat, kept in a folder named after it (`project/chat/tool-results/...` pictures and tool output, `.../subagents/...`).
fn claude_keeps(parts: &[String], is_jsonl: bool) -> bool {
    (parts.len() == 2 && is_jsonl) || (parts.len() >= 4 && matches!(parts[2].as_str(), "tool-results" | "subagents"))
}

#[cfg(test)]
mod media_tests {
    use super::claude_keeps;

    fn p(s: &str) -> Vec<String> {
        s.split('/').map(String::from).collect()
    }

    #[test]
    fn chats_and_their_media_are_kept() {
        assert!(claude_keeps(&p("proj/chat.jsonl"), true));
        assert!(claude_keeps(&p("proj/chat/tool-results/shot.png"), false));
        assert!(claude_keeps(&p("proj/chat/tool-results/nested/out.txt"), false));
        assert!(claude_keeps(&p("proj/chat/subagents/agent-1.jsonl"), true));
        assert!(!claude_keeps(&p("proj/notes.txt"), false));
        assert!(!claude_keeps(&p("proj/chat/other/file.png"), false));
    }
}

/// (source, destination) pairs. `forward` = live -> backup, otherwise backup -> live.
fn pairs(forward: bool) -> Vec<(PathBuf, PathBuf)> {
    let Some(b) = backup_root() else { return Vec::new() };
    // (live root, backup root, filter: 0 = everything, 1 = Claude transcripts only, 2 = Cursor transcripts only, 3 = Gemini chats, 4 = OpenCode database)
    let mut maps: Vec<(PathBuf, PathBuf, u8)> = Vec::new();
    let on = crate::integrations::is_enabled;
    if let (true, Some(p)) = (on("claude"), claude_projects()) {
        maps.push((p, b.join("claude").join("projects"), 1));
    }
    if let (true, Some(p)) = (on("claude"), claude_meta()) {
        maps.push((p, b.join("claude").join("sessions-meta"), 0));
    }
    if let (true, Some(c)) = (on("codex"), codex_home()) {
        maps.push((c.join("sessions"), b.join("codex").join("sessions"), 0));
        maps.push((c.join("archived_sessions"), b.join("codex").join("archived_sessions"), 0));
        // Pictures Codex generated and files attached to chats; the pictures you paste are inside the chat files.
        maps.push((c.join("generated_images"), b.join("codex").join("generated_images"), 0));
        maps.push((c.join("attachments"), b.join("codex").join("attachments"), 0));
    }
    // Cursor: the plain-text agent transcripts and the CLI chat stores. The IDE database itself is not copied while Cursor may be writing to it.
    if let (true, Some(h)) = (on("cursor"), home()) {
        maps.push((h.join(".cursor").join("projects"), b.join("cursor").join("projects"), 2));
        maps.push((h.join(".cursor").join("chats"), b.join("cursor").join("chats"), 0));
    }
    // Gemini CLI (chat files per project) and Cline (one folder per task). OpenCode keeps everything in one SQLite file:
    // it is copied into the backup but never written back over the live one.
    if let Some(h) = home() {
        if on("gemini") {
            maps.push((h.join(".gemini").join("tmp"), b.join("gemini").join("tmp"), 3));
        }
        if on("cline") {
            maps.push((h.join(".cline").join("data").join("sessions"), b.join("cline").join("sessions"), 0));
        }
        if forward && on("opencode") {
            let db = h.join(".local").join("share").join("opencode");
            maps.push((db, b.join("opencode"), 4));
        }
    }
    let mut out = Vec::new();
    for (live, back, filter) in maps {
        let (from, to) = if forward { (&live, &back) } else { (&back, &live) };
        let mut files = Vec::new();
        walk_all(from, &mut files);
        for f in files {
            let Ok(rel) = f.strip_prefix(from) else { continue };
            // Claude project folders also hold sub-agent and tool-result files; keep only transcripts.
            let is_jsonl = f.extension().map(|e| e == "jsonl").unwrap_or(false);
            // Claude Code: the chat itself (project/chat.jsonl) plus what belongs to it, kept in a folder named after the
            // chat: screenshots and other results of its tools (tool-results/), and its helper agents' own chats
            // (subagents/).
            if filter == 1 {
                let parts: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().to_string()).collect();
                if !claude_keeps(&parts, is_jsonl) {
                    continue;
                }
            }
            if filter == 2 && !(is_jsonl && rel.to_string_lossy().contains("agent-transcripts")) {
                continue;
            }
            // Gemini: the saved chats (tmp/<project>/chats/*.json and the newer .jsonl), what its tools produced
            // (tool-outputs/), and the file naming the project.
            if filter == 3 {
                let s = rel.to_string_lossy();
                let name = f.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                let chat = s.contains("chats") && (name.ends_with(".json") || name.ends_with(".jsonl"));
                if !(chat || s.contains("tool-outputs") || name == ".project_root") {
                    continue;
                }
            }
            // OpenCode: just its database file.
            if filter == 4 && f.file_name().map(|n| n != "opencode.db").unwrap_or(true) {
                continue;
            }
            out.push((f.clone(), to.join(rel)));
        }
    }
    out
}

fn needs_copy(src: &Path, dst: &Path, overwrite_changed: bool) -> bool {
    let Ok(dm) = fs::metadata(dst) else { return true };
    if !overwrite_changed {
        return false;
    }
    let Ok(sm) = fs::metadata(src) else { return false };
    if sm.len() != dm.len() {
        return true;
    }
    match (sm.modified(), dm.modified()) {
        (Ok(s), Ok(d)) => s.duration_since(d).map(|x| x.as_secs() > 2).unwrap_or(false),
        _ => false,
    }
}

fn run(app: &AppHandle, forward: bool) -> Result<SyncStats, String> {
    if BUSY.swap(true, Ordering::SeqCst) {
        return Err("A sync is already running".into());
    }
    let result = (|| {
        let root = backup_root().ok_or("No Documents folder")?;
        fs::create_dir_all(&root).map_err(|e| e.to_string())?;
        let all = pairs(forward);
        let todo: Vec<&(PathBuf, PathBuf)> = all.iter().filter(|(s, d)| needs_copy(s, d, forward)).collect();
        let mut stats = SyncStats { total: todo.len(), skipped: all.len() - todo.len(), ..Default::default() };
        let phase = if forward { "Backing up" } else { "Restoring" };
        let mut stopped = false;
        for (i, (src, dst)) in todo.iter().enumerate() {
            // A Google Drive sync that started this backup was cancelled: stop copying.
            if i % 10 == 0 && crate::gdrive::cancel_requested() {
                stopped = true;
                break;
            }
            if let Some(parent) = dst.parent() {
                let _ = fs::create_dir_all(parent);
            }
            match fs::copy(src, dst) {
                Ok(n) => {
                    stats.copied += 1;
                    stats.bytes += n;
                }
                Err(_) => stats.failed += 1,
            }
            if i % 10 == 0 {
                let _ = app.emit(
                    "sync-progress",
                    Progress {
                        phase: phase.into(),
                        done: i + 1,
                        total: todo.len(),
                        current: src.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
                    },
                );
            }
        }
        merge_codex_index(forward);
        // A backup stopped halfway is not a finished one: "last synced" keeps the time of the last complete backup.
        if forward && !stopped {
            let ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);
            let _ = fs::write(root.join(".chatvault-sync.json"), format!("{{\"lastSyncMs\":{}}}", ms));
        }
        let _ = app.emit(
            "sync-progress",
            Progress { phase: "Done".into(), done: todo.len(), total: todo.len(), current: String::new() },
        );
        Ok(stats)
    })();
    BUSY.store(false, Ordering::SeqCst);
    result
}

/// session_index.jsonl holds Codex thread names. Add any thread the destination does not have yet.
fn merge_codex_index(forward: bool) {
    if !crate::integrations::is_enabled("codex") {
        return;
    }
    let (Some(c), Some(b)) = (codex_home(), backup_root()) else { return };
    let live = c.join("session_index.jsonl");
    let back = b.join("codex").join("session_index.jsonl");
    let (from, to) = if forward { (live, back) } else { (back, live) };
    let Ok(src) = File::open(&from) else { return };
    let have: HashSet<String> = File::open(&to)
        .map(|f| {
            BufReader::new(f)
                .lines()
                .flatten()
                .filter_map(|l| serde_json::from_str::<Value>(&l).ok())
                .filter_map(|v| v.get("id").and_then(|x| x.as_str()).map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let mut add = Vec::new();
    for line in BufReader::new(src).lines().flatten() {
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        if let Some(id) = v.get("id").and_then(|x| x.as_str()) {
            if !have.contains(id) {
                add.push(line);
            }
        }
    }
    if add.is_empty() {
        return;
    }
    if let Some(parent) = to.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(&to) {
        for l in add {
            let _ = writeln!(f, "{}", l);
        }
    }
}

pub fn backup(app: &AppHandle) -> Result<SyncStats, String> {
    run(app, true)
}

pub fn restore(app: &AppHandle) -> Result<SyncStats, String> {
    run(app, false)
}

/// How many files "Restore missing" would copy back into the tools now (files in the backup the tools do not have).
pub fn restore_preview() -> usize {
    pairs(false).iter().filter(|(s, d)| needs_copy(s, d, false)).count()
}

pub fn status() -> BackupStatus {
    let Some(root) = backup_root() else {
        return BackupStatus { path: String::new(), exists: false, files: 0, bytes: 0, last_sync_ms: None };
    };
    let mut files = Vec::new();
    walk_all(&root, &mut files);
    let bytes = files.iter().filter_map(|f| fs::metadata(f).ok()).map(|m| m.len()).sum();
    let last_sync_ms = fs::read_to_string(root.join(".chatvault-sync.json"))
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .and_then(|v| v.get("lastSyncMs").and_then(|x| x.as_i64()));
    BackupStatus {
        path: root.to_string_lossy().to_string(),
        exists: root.exists(),
        files: files.len(),
        bytes,
        last_sync_ms,
    }
}
