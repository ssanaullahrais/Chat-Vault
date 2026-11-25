//! Where each tool's chats are read from, for the Settings dialog. The lists come from the same helpers the scanner
//! uses, so what is shown is exactly what is read.

use crate::paths::*;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Location {
    pub label: String,
    pub path: String,
    pub is_file: bool,
    pub exists: bool,
    /// How many chats are there, when that is cheap to know.
    pub items: Option<usize>,
    pub bytes: Option<u64>,
}

#[derive(Serialize)]
pub struct Source {
    pub provider: String,
    pub name: String,
    /// False when the tool is switched off in Settings: it is listed here, but not read.
    pub enabled: bool,
    pub locations: Vec<Location>,
}

fn plain(p: &Path) -> String {
    // One kind of slash, so a path reads the same wherever it is shown.
    p.to_string_lossy().trim_start_matches(r"\\?\").replace('/', "\\")
}

/// A folder of chat files. With `count`, the files are counted and measured.
fn folder(label: &str, path: &Path, exts: &[&str], count: bool) -> Location {
    let exists = path.is_dir();
    let (mut items, mut bytes) = (None, None);
    if exists && count {
        let mut files = Vec::new();
        walk_files(path, exts, &mut files);
        bytes = Some(files.iter().filter_map(|f| std::fs::metadata(f).ok()).map(|m| m.len()).sum());
        items = Some(files.len());
    }
    Location { label: label.to_string(), path: plain(path), is_file: false, exists, items, bytes }
}

/// A folder whose chat count is already known.
fn dir(label: &str, path: &Path, items: Option<usize>) -> Location {
    Location { label: label.to_string(), path: plain(path), is_file: false, exists: path.is_dir(), items, bytes: None }
}

fn file(label: &str, path: &Path, items: Option<usize>) -> Location {
    let meta = std::fs::metadata(path).ok();
    Location { label: label.to_string(), path: plain(path), is_file: true, exists: meta.is_some(), items, bytes: meta.map(|m| m.len()) }
}

pub fn all(count_all: bool) -> Vec<Source> {
    let on = crate::integrations::is_enabled;
    let backup = backup_root();
    let in_backup = |sub: &str| backup.as_ref().map(|b| b.join(sub));
    let mut out = Vec::new();

    let count = count_all && on("claude");
    let mut claude = Vec::new();
    for r in claude_project_roots() {
        claude.push(folder("Chats", &r, &["jsonl"], count));
    }
    if let Some(m) = claude_meta() {
        claude.push(dir("Session details (Claude desktop app)", &m, None));
    }
    if let Some(b) = in_backup("claude/projects") {
        claude.push(folder("Backup copy", &b, &["jsonl"], count));
    }
    out.push(Source { provider: "claude".into(), name: "Claude Code".into(), enabled: on("claude"), locations: claude });

    let count = count_all && on("codex");
    let mut codex = Vec::new();
    for r in codex_roots() {
        codex.push(folder("Chats", &r.join("sessions"), &["jsonl"], count));
        codex.push(folder("Archived chats", &r.join("archived_sessions"), &["jsonl", "zst"], count));
    }
    for db in crate::codex_db::database_files() {
        codex.push(file("Database", &db, None));
    }
    if let Some(b) = in_backup("codex/sessions") {
        codex.push(folder("Backup copy", &b, &["jsonl"], count));
    }
    out.push(Source { provider: "codex".into(), name: "Codex".into(), enabled: on("codex"), locations: codex });

    let mut cursor = Vec::new();
    for (label, p, items) in crate::cursor::locations() {
        cursor.push(if p.is_file() { file(&label, &p, items) } else { dir(&label, &p, items) });
    }
    if let Some(b) = in_backup("cursor") {
        cursor.push(dir("Backup copy", &b, None));
    }
    out.push(Source { provider: "cursor".into(), name: "Cursor".into(), enabled: on("cursor"), locations: cursor });

    let count = count_all && on("gemini");
    let mut gemini = Vec::new();
    let gfiles = if count { crate::others::gemini_chat_files() } else { Vec::new() };
    for r in crate::others::gemini_roots() {
        let n = count.then(|| gfiles.iter().filter(|f| f.starts_with(&r)).count());
        gemini.push(dir("Chats", &r, n));
    }
    out.push(Source { provider: "gemini".into(), name: "Gemini CLI".into(), enabled: on("gemini"), locations: gemini });

    let count = count_all && on("opencode");
    let mut opencode = Vec::new();
    let found = crate::others::opencode_found();
    if found.is_empty() {
        // Not installed or never used: show where it would be looked for.
        if let Some(p) = crate::others::opencode_candidates().into_iter().next() {
            opencode.push(file("Database", &p, None));
        }
    }
    for db in found {
        let n = if count { crate::others::opencode_chat_count(&db) } else { None };
        opencode.push(file("Database", &db, n));
    }
    out.push(Source { provider: "opencode".into(), name: "OpenCode".into(), enabled: on("opencode"), locations: opencode });

    let count = count_all && on("cline");
    let mut cline = Vec::new();
    let cfiles = if count { crate::others::cline_chat_files() } else { Vec::new() };
    for (i, r) in crate::others::cline_roots().into_iter().enumerate() {
        // The command-line folder is always listed; the editor extensions only when they are installed.
        if i == 0 || r.is_dir() {
            let n = count.then(|| cfiles.iter().filter(|f| f.starts_with(&r)).count());
            cline.push(dir(if i == 0 { "Chats (command line)" } else { "Chats (editor extension)" }, &r, n));
        }
    }
    out.push(Source { provider: "cline".into(), name: "Cline".into(), enabled: on("cline"), locations: cline });
    out
}

/// Opens one of the listed locations in Explorer. Only paths from the list above are accepted, so this can never be
/// used to open an arbitrary place.
pub fn open(path: &str) -> Result<(), String> {
    let listed = all(false).iter().flat_map(|s| s.locations.iter()).any(|l| l.path == path);
    if !listed {
        return Err("That is not one of the chat locations.".into());
    }
    let p = PathBuf::from(path);
    if p.is_dir() {
        Command::new("explorer").arg(&p).spawn().map_err(|e| e.to_string())?;
    } else if p.is_file() {
        Command::new("explorer").arg(format!("/select,{}", path)).spawn().map_err(|e| e.to_string())?;
    } else {
        return Err("That location does not exist on this PC.".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "reads the real chat folders on this PC"]
    fn print_real_locations() {
        for s in all(true) {
            println!("== {}", s.name);
            for l in s.locations {
                println!("   [{}] {} -> {} ({:?} chats, {:?} bytes)", if l.exists { "found" } else { "missing" }, l.label, l.path, l.items, l.bytes);
            }
        }
    }

    #[test]
    fn every_tool_is_listed_and_only_listed_places_can_be_opened() {
        let all = all(false);
        let ids: Vec<&str> = all.iter().map(|s| s.provider.as_str()).collect();
        assert_eq!(ids, ["claude", "codex", "cursor", "gemini", "opencode", "cline"]);
        assert!(all.iter().all(|s| !s.locations.is_empty()), "each tool shows at least one place");
        assert!(open("C:\\Windows\\System32").is_err(), "an unlisted place is refused");
        assert!(open("").is_err());
    }
}
