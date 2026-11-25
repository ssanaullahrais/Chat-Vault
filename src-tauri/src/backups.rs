//! The Backups tab: every chat in the backup folder and in Google Drive as one list, what is where, and the actions to
//! manage it (upload, download, delete). Files are grouped into "entries": one chat with its pictures and files, or one
//! of the few shared files (chat names, generated pictures, the OpenCode database).

use crate::gdrive::{self, Remote};
use crate::paths::*;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

/// One backup entry: which tool it is from and its key inside that tool.
#[derive(Deserialize, Clone, Debug)]
pub struct UnitRef {
    pub provider: String,
    pub key: String,
}

fn stem(name: &str) -> &str {
    let mut s = name;
    for ext in [".zst", ".jsonl", ".json"] {
        if let Some(x) = s.strip_suffix(ext) {
            s = x;
        }
    }
    s
}

/// Which entry a file of the backup folder belongs to (`rel` has `/` between folders), or None for a file that is not
/// part of any chat.
pub(crate) fn unit_of(rel: &str) -> Option<(String, String)> {
    let p: Vec<&str> = rel.split('/').collect();
    let one = |prov: &str, key: &str| Some((prov.to_string(), key.to_string()));
    match *p.first()? {
        "claude" => match *p.get(1)? {
            // claude/projects/<project>/<chat>.jsonl and claude/projects/<project>/<chat>/tool-results/...
            "projects" if p.len() == 4 => one("claude", p[3].strip_suffix(".jsonl")?),
            "projects" if p.len() >= 5 => one("claude", p[3]),
            "sessions-meta" => one("claude", "session-details"),
            _ => None,
        },
        "codex" => match *p.get(1)? {
            "sessions" | "archived_sessions" => {
                let name = stem(p.last()?);
                // rollout-<date>-<36 character id>
                let key = if name.len() >= 36 && name.is_char_boundary(name.len() - 36) { &name[name.len() - 36..] } else { name };
                one("codex", key)
            }
            "session_index.jsonl" => one("codex", "thread-names"),
            "generated_images" => one("codex", "generated-images"),
            "attachments" => one("codex", p.get(2).map(|k| stem(k))?),
            _ => None,
        },
        "cursor" => {
            if let Some(i) = p.iter().position(|x| *x == "agent-transcripts") {
                return one("cursor", stem(p.get(i + 1)?));
            }
            // cursor/chats/<store>/<chat>/store.db
            if p.get(1) == Some(&"chats") {
                return one("cursor", p.get(3).or(p.get(2))?);
            }
            None
        }
        "gemini" => {
            // gemini/tmp/<project>/chats/<chat>.json(l) and gemini/tmp/<project>/tool-outputs/...
            if p.get(1) != Some(&"tmp") {
                return None;
            }
            let project = p.get(2)?;
            if p.get(3) == Some(&"chats") {
                return one("gemini", stem(p.get(4)?));
            }
            one("gemini", &format!("{project}-files"))
        }
        "cline" => one("cline", stem(p.get(2)?)),
        "opencode" => one("opencode", "database"),
        _ => None,
    }
}

/// A readable name for the entries that are not one chat.
fn shared_label(provider: &str, key: &str) -> Option<&'static str> {
    Some(match (provider, key) {
        ("claude", "session-details") => "Claude chat names and details",
        ("codex", "thread-names") => "Codex chat names",
        ("codex", "generated-images") => "Codex generated pictures",
        ("opencode", "database") => "OpenCode database",
        _ => return None,
    })
}

#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub id: String,
    pub provider: String,
    pub key: String,
    pub title: String,
    pub project: Option<String>,
    pub last_ms: i64,
    pub local_files: usize,
    pub local_bytes: u64,
    pub drive_files: usize,
    pub drive_bytes: u64,
    /// Names of the PCs that uploaded this entry.
    pub devices: Vec<String>,
    /// Ids of the PCs that uploaded this entry, to list one backup's chats.
    pub device_ids: Vec<String>,
    /// synced | upload | download | differs | unchecked
    pub status: String,
    /// The chat still exists in its own tool: deleting the backup does not lose it.
    pub original: bool,
    /// Not one chat: shared files such as chat names or the OpenCode database.
    pub shared: bool,
}

type Sizes = HashMap<String, (i64, i64)>;

/// Compares what is in the backup folder with what is in Drive for one entry.
pub(crate) fn status_of(local: &Sizes, remote: &Sizes, drive_checked: bool) -> &'static str {
    if !drive_checked {
        return "unchecked";
    }
    if remote.is_empty() {
        return "upload";
    }
    if local.is_empty() {
        return "download";
    }
    let (mut only_local, mut only_remote, mut differ) = (false, false, false);
    for (rel, (size, mtime)) in local {
        match remote.get(rel) {
            None => only_local = true,
            Some((rs, rm)) => {
                if rs != size || (rm - mtime).abs() >= 2000 {
                    differ = true;
                }
            }
        }
    }
    for rel in remote.keys() {
        if !local.contains_key(rel) {
            only_remote = true;
        }
    }
    if differ || (only_local && only_remote) {
        "differs"
    } else if only_local {
        "upload"
    } else if only_remote {
        "download"
    } else {
        "synced"
    }
}

#[derive(Default)]
struct Group {
    local: Sizes,
    remote: Sizes,
    devices: HashSet<String>,
    device_ids: HashSet<String>,
    last: i64,
}

/// One chat as the index knows it.
struct Known {
    title: String,
    project: String,
    last_ms: i64,
    live: bool,
}

fn known_chats() -> HashMap<(String, String), Known> {
    let mut out: HashMap<(String, String), Known> = HashMap::new();
    let Ok(c) = crate::index::open_db() else { return out };
    let Ok(mut st) = c.prepare("SELECT provider,id,title,project,last_ms,backup,path FROM chats") else { return out };
    let rows = st.query_map([], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?, r.get::<_, i64>(4)?, r.get::<_, i64>(5)? != 0, r.get::<_, String>(6)?))
    });
    let Ok(rows) = rows else { return out };
    for (prov, id, title, project, last_ms, backup, path) in rows.flatten() {
        let mut keys = vec![id.clone()];
        // Gemini and Cline name their files differently from the chat id: match on the file's or folder's name.
        let p = Path::new(&path);
        if let Some(s) = p.file_name().map(|n| stem(&n.to_string_lossy()).to_string()) {
            keys.push(s);
        }
        if let Some(d) = p.parent().and_then(|d| d.file_name()).map(|n| n.to_string_lossy().to_string()) {
            keys.push(d);
        }
        for k in keys {
            let e = out.entry((prov.clone(), k)).or_insert(Known { title: title.clone(), project: project.clone(), last_ms, live: false });
            e.live |= !backup;
            if !backup {
                e.title = title.clone();
                e.project = project.clone();
            }
            e.last_ms = e.last_ms.max(last_ms);
        }
    }
    out
}

/// The whole list. `drive` also reads what is in Google Drive (needs a connection; slower).
pub fn entries(drive: bool) -> Result<Vec<Entry>, String> {
    let root = backup_root().ok_or("There is no backup folder.")?;
    let checked = drive && gdrive::connected();
    let mut groups: HashMap<(String, String), Group> = HashMap::new();
    for (rel, _path, size, mtime) in gdrive::local_files(&root) {
        let Some(unit) = unit_of(&rel) else { continue };
        let g = groups.entry(unit).or_default();
        g.last = g.last.max(mtime);
        g.local.insert(rel, (size, mtime));
    }
    if checked {
        let (remote, names) = gdrive::remote_snapshot()?;
        for r in remote {
            let Some(unit) = unit_of(&r.rel) else { continue };
            let g = groups.entry(unit).or_default();
            g.last = g.last.max(r.mtime);
            g.devices.insert(names.get(&r.device).cloned().unwrap_or_else(|| "A PC".into()));
            g.device_ids.insert(r.device.clone());
            // The best copy of each file across PCs.
            let better = g.remote.get(&r.rel).is_none_or(|(s, m)| (r.size, r.mtime) > (*s, *m));
            if better {
                g.remote.insert(r.rel, (r.size, r.mtime));
            }
        }
    }
    let known = known_chats();
    let mut out = Vec::with_capacity(groups.len());
    for ((provider, key), g) in groups {
        let info = known.get(&(provider.clone(), key.clone()));
        let label = shared_label(&provider, &key);
        let title = match (label, info) {
            (Some(l), _) => l.to_string(),
            (None, Some(k)) if !k.title.trim().is_empty() => k.title.clone(),
            _ if key.ends_with("-files") => format!("{} files", provider_name(&provider)),
            _ => format!("{} chat {}", provider_name(&provider), key.chars().take(8).collect::<String>()),
        };
        out.push(Entry {
            id: format!("{provider}:{key}"),
            status: status_of(&g.local, &g.remote, checked).to_string(),
            local_files: g.local.len(),
            local_bytes: g.local.values().map(|(s, _)| *s as u64).sum(),
            drive_files: g.remote.len(),
            drive_bytes: g.remote.values().map(|(s, _)| *s as u64).sum(),
            devices: { let mut d: Vec<String> = g.devices.into_iter().collect(); d.sort(); d },
            device_ids: g.device_ids.into_iter().collect(),
            last_ms: info.map(|k| k.last_ms).unwrap_or(0).max(g.last),
            original: info.is_some_and(|k| k.live),
            project: info.map(|k| k.project.clone()),
            shared: label.is_some() || key.ends_with("-files"),
            title,
            provider,
            key,
        });
    }
    out.sort_by(|a, b| b.last_ms.cmp(&a.last_ms).then(a.title.cmp(&b.title)));
    Ok(out)
}

fn provider_name(p: &str) -> &'static str {
    match p {
        "claude" => "Claude",
        "codex" => "Codex",
        "cursor" => "Cursor",
        "gemini" => "Gemini",
        "opencode" => "OpenCode",
        "cline" => "Cline",
        _ => "Chat",
    }
}

/// Sends files to the Recycle Bin in one go (one PowerShell start for all of them). Returns how many were moved.
fn recycle_many(paths: &[PathBuf]) -> Result<usize, String> {
    use std::os::windows::process::CommandExt;
    if paths.is_empty() {
        return Ok(0);
    }
    let list = std::env::temp_dir().join(format!("chatvault-recycle-{}.txt", std::process::id()));
    let text: Vec<String> = paths.iter().map(|p| p.to_string_lossy().trim_start_matches(r"\\?\").to_string()).collect();
    fs::write(&list, text.join("\n")).map_err(|_| "Could not prepare the delete.")?;
    let script = "Add-Type -AssemblyName Microsoft.VisualBasic; $n=0; foreach($p in [IO.File]::ReadAllLines($env:CHATVAULT_LIST)){ try{ if(Test-Path -LiteralPath $p -PathType Leaf){ [Microsoft.VisualBasic.FileIO.FileSystem]::DeleteFile($p,'OnlyErrorDialogs','SendToRecycleBin'); $n++ } }catch{} }; Write-Output $n";
    let out = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .env("CHATVAULT_LIST", &list)
        .creation_flags(0x0800_0000)
        .output();
    let _ = fs::remove_file(&list);
    let out = out.map_err(|_| "Could not move the files to the Recycle Bin.")?;
    Ok(String::from_utf8_lossy(&out.stdout).trim().parse::<usize>().unwrap_or(0))
}

/// Removes folders left empty by a delete, up to (not including) the backup folder.
fn tidy(root: &Path, files: &[PathBuf]) {
    let mut dirs: HashSet<PathBuf> = HashSet::new();
    for f in files {
        let mut d = f.parent();
        while let Some(x) = d {
            if x == root || !x.starts_with(root) {
                break;
            }
            dirs.insert(x.to_path_buf());
            d = x.parent();
        }
    }
    let mut dirs: Vec<PathBuf> = dirs.into_iter().collect();
    dirs.sort_by_key(|d| std::cmp::Reverse(d.components().count()));
    for d in dirs {
        let _ = fs::remove_dir(&d);
    }
}

/// Deletes the copies of the given entries: this PC's backup copy (to the Recycle Bin) and/or the Google Drive copy.
pub fn delete(units: &[UnitRef], local: bool, drive: bool) -> Result<gdrive::Removed, String> {
    let root = backup_root().ok_or("There is no backup folder.")?;
    let set: HashSet<(String, String)> = units.iter().map(|u| (u.provider.clone(), u.key.clone())).collect();
    if set.is_empty() {
        return Ok(gdrive::Removed::default());
    }
    if drive && !gdrive::connected() {
        return Err("Google Drive is not connected.".into());
    }
    let mut out = gdrive::Removed::default();
    let mut run = || -> Result<(), String> {
        if drive {
            out.drive_files = gdrive::delete_drive(&set)?;
        }
        if local {
            let files: Vec<(String, PathBuf)> = gdrive::local_files(&root).into_iter().filter(|(rel, ..)| unit_of(rel).is_some_and(|u| set.contains(&u))).map(|(rel, path, ..)| (rel, path)).collect();
            // Only files that are really inside the backup folder.
            let safe: Vec<&(String, PathBuf)> = files.iter().filter(|(_, p)| p.starts_with(&root)).collect();
            let paths: Vec<PathBuf> = safe.iter().map(|(_, p)| p.clone()).collect();
            out.local_files = recycle_many(&paths)?;
            out.failed = paths.len().saturating_sub(out.local_files);
            gdrive::forget_local(&safe.iter().map(|(r, _)| r.clone()).collect::<Vec<_>>());
            tidy(&root, &paths);
        }
        Ok(())
    };
    // A delete in Drive must not overlap a sync; a local-only delete does not touch Drive.
    if drive {
        gdrive::exclusive(|| run())?;
    } else {
        run()?;
    }
    Ok(out)
}

// ---------------------------------------------------------------- the list of backups

/// One backup: this PC's backup folder, or one PC's backup in Google Drive.
#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct BackupSet {
    /// "local", or "drive:<device id>".
    pub id: String,
    /// "local" | "drive"
    pub kind: String,
    /// The PC's name.
    pub name: String,
    /// The folder, for the backup on this PC.
    pub path: Option<String>,
    /// The Drive device id, for a Drive backup.
    pub device: Option<String>,
    pub this_pc: bool,
    pub files: usize,
    pub bytes: u64,
    /// Chats in it (shared files such as chat names are not counted).
    pub chats: usize,
    pub last_ms: Option<i64>,
    /// Chats that are nowhere else: not in their tool, not in any other backup. Deleting this backup loses them.
    pub only_copy: usize,
    /// For this PC's Drive backup: files here not uploaded yet. For another PC's: its files not downloaded here yet.
    pub pending: usize,
    /// For the backup on this PC: files in it that are not in the tools yet ("Restore missing" copies them).
    pub restorable: usize,
}

fn is_chat(unit: &(String, String)) -> bool {
    shared_label(&unit.0, &unit.1).is_none() && !unit.1.ends_with("-files")
}

/// This PC's backup folder first, then each PC's backup in Google Drive (this PC's first). `drive` reads Drive too.
pub fn sets(drive: bool) -> Result<Vec<BackupSet>, String> {
    let root = backup_root().ok_or("There is no backup folder.")?;
    let local = gdrive::local_files(&root);
    let status = crate::sync::status();
    let me = gdrive::this_device();
    let pc = gdrive::this_pc_name();
    let (skip_up, skip_down) = gdrive::skips();

    // Which backups hold each chat, to find the ones a delete would lose.
    let mut holders: HashMap<(String, String), HashSet<String>> = HashMap::new();
    let mut here = BackupSet {
        id: "local".into(),
        kind: "local".into(),
        name: pc.clone(),
        path: Some(root.to_string_lossy().to_string()),
        this_pc: true,
        last_ms: status.last_sync_ms,
        restorable: crate::sync::restore_preview(),
        ..Default::default()
    };
    let mut local_sizes: HashMap<&str, (i64, i64)> = HashMap::new();
    for (rel, _, size, mtime) in &local {
        here.files += 1;
        here.bytes += *size as u64;
        local_sizes.insert(rel.as_str(), (*size, *mtime));
        if let Some(u) = unit_of(rel) {
            holders.entry(u).or_default().insert("local".into());
        }
    }

    let mut out = Vec::new();
    if drive && gdrive::connected() {
        let all = gdrive::drive_listing()?;
        let devices = gdrive::device_files(&all);
        let mut by_dev: HashMap<String, BackupSet> = HashMap::new();
        let mut mine: HashMap<String, (i64, i64)> = HashMap::new();
        for r in all.iter().filter_map(gdrive::remote_of_pub) {
            let set = by_dev.entry(r.device.clone()).or_insert_with(|| BackupSet { id: format!("drive:{}", r.device), kind: "drive".into(), device: Some(r.device.clone()), this_pc: r.device == me, ..Default::default() });
            set.files += 1;
            // What it takes up in Drive (compressed), the same figure Google shows.
            set.bytes += (if r.bytes > 0 { r.bytes } else { r.size }).max(0) as u64;
            set.last_ms = Some(set.last_ms.unwrap_or(0).max(r.mtime));
            if let Some(u) = unit_of(&r.rel) {
                holders.entry(u).or_default().insert(set.id.clone());
            }
            if r.device == me {
                mine.insert(r.rel.clone(), (r.size, r.mtime));
            } else if !local_sizes.contains_key(r.rel.as_str()) && skip_down.get(&r.rel) != Some(&r.version()) {
                set.pending += 1;
            }
        }
        // A PC whose device file is there but whose files were all removed still shows, empty.
        for (id, (name, at, _)) in &devices {
            let set = by_dev.entry(id.clone()).or_insert_with(|| BackupSet { id: format!("drive:{id}"), kind: "drive".into(), device: Some(id.clone()), this_pc: *id == me, ..Default::default() });
            set.name = name.clone();
            if at.is_some() {
                set.last_ms = *at;
            }
        }
        for set in by_dev.values_mut() {
            if set.this_pc {
                set.name = pc.clone();
                // This PC's files that are not in its Drive backup yet (and were not left out on purpose).
                set.pending = local
                    .iter()
                    .filter(|(rel, _, size, mtime)| {
                        let up = mine.get(rel).is_some_and(|(s, m)| s == size && (m - mtime).abs() < 2000);
                        !up && skip_up.get(rel) != Some(&(*size, *mtime))
                    })
                    .count();
            } else if set.name.is_empty() {
                set.name = "A PC".into();
            }
        }
        let mut drive_sets: Vec<BackupSet> = by_dev.into_values().collect();
        drive_sets.sort_by(|a, b| b.this_pc.cmp(&a.this_pc).then(b.last_ms.cmp(&a.last_ms)).then(a.name.cmp(&b.name)));
        out.push(here);
        out.extend(drive_sets);
    } else {
        out.push(here);
    }

    // Chats per backup, and the ones only that backup has.
    let live: HashSet<(String, String)> = known_chats().into_iter().filter(|(_, k)| k.live).map(|(u, _)| u).collect();
    for set in out.iter_mut() {
        for (u, h) in &holders {
            if !is_chat(u) || !h.contains(&set.id) {
                continue;
            }
            set.chats += 1;
            if h.len() == 1 && !live.contains(u) {
                set.only_copy += 1;
            }
        }
    }
    Ok(out)
}

/// Moves this PC's whole backup to the Recycle Bin (every file ChatVault put in the backup folder). The chats in the
/// tools and the Google Drive backups are not touched.
pub fn delete_local_all() -> Result<gdrive::Removed, String> {
    if crate::sync::BUSY.load(std::sync::atomic::Ordering::SeqCst) || gdrive::busy() {
        return Err("A backup or sync is running. Try again when it finishes.".into());
    }
    let root = backup_root().ok_or("There is no backup folder.")?;
    let files: Vec<(String, PathBuf)> = gdrive::local_files(&root).into_iter().filter(|(_, p, ..)| p.starts_with(&root)).map(|(r, p, ..)| (r, p)).collect();
    let paths: Vec<PathBuf> = files.iter().map(|(_, p)| p.clone()).collect();
    let moved = recycle_many(&paths)?;
    gdrive::forget_local(&files.iter().map(|(r, _)| r.clone()).collect::<Vec<_>>());
    tidy(&root, &paths);
    // "Last backed up" no longer describes anything that is there.
    let _ = fs::remove_file(root.join(".chatvault-sync.json"));
    Ok(gdrive::Removed { local_files: moved, drive_files: 0, failed: paths.len().saturating_sub(moved) })
}

/// The chats of one backup, for "Download to this PC": every entry that PC has in Drive.
pub fn device_units(device: &str) -> Result<HashSet<(String, String)>, String> {
    let all = gdrive::drive_listing()?;
    Ok(all.iter().filter_map(gdrive::remote_of_pub).filter(|r| r.device == device).filter_map(|r| unit_of(&r.rel)).collect())
}

/// Where the remote file of an entry is, for the tests and the UI's "which PC" text.
#[allow(dead_code)]
fn remote_units(remote: &[Remote]) -> HashSet<(String, String)> {
    remote.iter().filter_map(|r| unit_of(&r.rel)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(rel: &str) -> Option<(String, String)> {
        unit_of(rel)
    }
    fn pair(a: &str, b: &str) -> Option<(String, String)> {
        Some((a.to_string(), b.to_string()))
    }

    #[test]
    fn files_are_grouped_into_the_chat_they_belong_to() {
        assert_eq!(u("claude/projects/C--proj/abc-123.jsonl"), pair("claude", "abc-123"));
        assert_eq!(u("claude/projects/C--proj/abc-123/tool-results/shot.png"), pair("claude", "abc-123"));
        assert_eq!(u("claude/projects/C--proj/abc-123/subagents/agent-1.jsonl"), pair("claude", "abc-123"));
        assert_eq!(u("claude/sessions-meta/acct/org/local_x.json"), pair("claude", "session-details"));
        assert_eq!(u("codex/sessions/2026/11/25/rollout-2026-11-25T10-00-00-01a1c18a-602d-70d2-89d2-f9a3bb4e2981.jsonl"), pair("codex", "01a1c18a-602d-70d2-89d2-f9a3bb4e2981"));
        assert_eq!(u("codex/archived_sessions/rollout-x-01a1c18a-602d-70d2-89d2-f9a3bb4e2981.jsonl.zst"), pair("codex", "01a1c18a-602d-70d2-89d2-f9a3bb4e2981"));
        assert_eq!(u("codex/generated_images/a/b.png"), pair("codex", "generated-images"));
        assert_eq!(u("codex/attachments/01a1c18a/file.txt"), pair("codex", "01a1c18a"));
        assert_eq!(u("cursor/projects/p/agent-transcripts/chat1/chat1.jsonl"), pair("cursor", "chat1"));
        assert_eq!(u("cursor/chats/store/chat2/store.db"), pair("cursor", "chat2"));
        assert_eq!(u("gemini/tmp/proj/chats/session-2026-abcd.json"), pair("gemini", "session-2026-abcd"));
        assert_eq!(u("gemini/tmp/proj/tool-outputs/x.txt"), pair("gemini", "proj-files"));
        assert_eq!(u("cline/sessions/task1/ui.json"), pair("cline", "task1"));
        assert_eq!(u("opencode/opencode.db"), pair("opencode", "database"));
        assert_eq!(u("unknown/x"), None);
        assert_eq!(u("claude/projects/p"), None);
    }

    fn sizes(items: &[(&str, i64, i64)]) -> Sizes {
        items.iter().map(|(r, s, m)| (r.to_string(), (*s, *m))).collect()
    }

    #[test]
    fn status_says_what_is_where() {
        let a = sizes(&[("f1", 10, 1000), ("f2", 5, 2000)]);
        assert_eq!(status_of(&a, &a, true), "synced");
        assert_eq!(status_of(&a, &sizes(&[("f1", 10, 1500), ("f2", 5, 2000)]), true), "synced", "a second of clock drift is not a difference");
        assert_eq!(status_of(&a, &Sizes::new(), true), "upload");
        assert_eq!(status_of(&Sizes::new(), &a, true), "download");
        assert_eq!(status_of(&a, &sizes(&[("f1", 10, 1000)]), true), "upload", "part of it is not in Drive yet");
        assert_eq!(status_of(&sizes(&[("f1", 10, 1000)]), &a, true), "download");
        assert_eq!(status_of(&a, &sizes(&[("f1", 99, 1000), ("f2", 5, 2000)]), true), "differs");
        assert_eq!(status_of(&sizes(&[("f1", 10, 1000), ("x", 1, 1)]), &sizes(&[("f1", 10, 1000), ("y", 1, 1)]), true), "differs");
        assert_eq!(status_of(&a, &Sizes::new(), false), "unchecked");
    }
}
