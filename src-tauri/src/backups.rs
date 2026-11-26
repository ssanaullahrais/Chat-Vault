//! The Backups tab: every chat in the backup folder and in Google Drive as one list, what is where, and the actions to
//! manage it (upload, download, delete). Files are grouped into "entries": one chat with its pictures and files, or one
//! of the few shared files (chat names, generated pictures, the OpenCode database).

use crate::gdrive;
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
                if rs != size || rm != mtime {
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

#[derive(Clone, Default)]
struct Group {
    local: Sizes,
    remote: Sizes,
    devices: HashSet<String>,
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
pub fn entries(drive: bool, preview: Option<impl Fn(Vec<Entry>)>) -> Result<Vec<Entry>, String> {
    let root = backup_root().ok_or("There is no backup folder.")?;
    let checked = drive && gdrive::connected();
    let known = known_chats();
    let mut groups: HashMap<(String, String), Group> = HashMap::new();
    let mut files = 0;
    gdrive::visit_local_files(&root, |(rel, _, size, mtime)| {
        let Some(unit) = unit_of(&rel) else { return };
        let g = groups.entry(unit).or_default();
        g.last = g.last.max(mtime);
        g.local.insert(rel, (size, mtime));
        files += 1;
        if files == 32 || (files >= 512 && files % 512 == 0) {
            if let Some(emit) = &preview {
                emit(entries_from_groups(groups.clone(), &known, false));
            }
        }
    });
    if checked {
        if let Some(emit) = &preview {
            emit(entries_from_groups(groups.clone(), &known, false));
        }
    }
    if checked {
        let (remote, names) = gdrive::remote_snapshot()?;
        for r in remote {
            let Some(unit) = unit_of(&r.rel) else { continue };
            let g = groups.entry(unit).or_default();
            g.last = g.last.max(r.mtime);
            g.devices.insert(names.get(&r.device).cloned().unwrap_or_else(|| "A PC".into()));
            // The best copy of each file across PCs.
            let matches_local = g.local.get(&r.rel) == Some(&(r.size, r.mtime));
            let better = g.remote.get(&r.rel).is_none_or(|(s, m)| {
                matches_local || (g.local.get(&r.rel) != Some(&(*s, *m)) && (r.mtime, r.size) > (*m, *s))
            });
            if better {
                g.remote.insert(r.rel, (r.size, r.mtime));
            }
        }
    }
    Ok(entries_from_groups(groups, &known, checked))
}

fn entries_from_groups(
    groups: HashMap<(String, String), Group>,
    known: &HashMap<(String, String), Known>,
    checked: bool,
) -> Vec<Entry> {
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
    out
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
pub fn delete(
    units: &[UnitRef], local: bool, drive: bool,
    stop: &std::sync::atomic::AtomicBool, retry: Option<&HashSet<String>>, scope: Option<&str>,
    report: impl Fn(usize, usize) + Sync,
) -> Result<gdrive::Removed, String> {
    let root = backup_root().ok_or("There is no backup folder.")?;
    let set: HashSet<(String, String)> = units.iter().map(|u| (u.provider.clone(), u.key.clone())).collect();
    if set.is_empty() {
        return Ok(gdrive::Removed::default());
    }
    if drive && !gdrive::connected() {
        return Err("Google Drive is not connected.".into());
    }
    let mut out = gdrive::Removed::default();
    let run = || -> Result<(), String> {
        if drive {
            if let Some(scope) = scope { gdrive::validate_scope(scope)?; }
            gdrive::delete_drive(&set, &mut out, stop, retry, report)?;
            if out.error.is_some() || out.stopped {
                return Ok(());
            }
        }
        if local {
            let files: Vec<(String, PathBuf)> = gdrive::local_files(&root).into_iter().filter(|(rel, ..)| unit_of(rel).is_some_and(|u| set.contains(&u))).map(|(rel, path, ..)| (rel, path)).collect();
            // Only files that are really inside the backup folder.
            let safe: Vec<&(String, PathBuf)> = files.iter().filter(|(_, p)| p.starts_with(&root)).collect();
            let paths: Vec<PathBuf> = safe.iter().map(|(_, p)| p.clone()).collect();
            match recycle_many(&paths) {
                Ok(moved) => out.local_files = moved,
                Err(error) if drive => {
                    out.error = Some(error);
                    out.failed = paths.len();
                    return Ok(());
                }
                Err(error) => return Err(error),
            }
            out.failed = paths.len().saturating_sub(out.local_files);
            gdrive::forget_local(&safe.iter().map(|(r, _)| r.clone()).collect::<Vec<_>>());
            tidy(&root, &paths);
        }
        Ok(())
    };
    if drive && !local {
        gdrive::scoped(&set, run)?;
    } else {
        gdrive::exclusive(|| crate::sync::exclusive(run))?;
    }
    Ok(out)
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
    #[ignore = "uses the Windows Recycle Bin"]
    fn local_delete_recycles_only_the_selected_file() {
        let root = std::env::temp_dir().join(format!(
            "chatvault-recycle-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        let selected = root.join("disposable backup.jsonl");
        let retained = root.join("disposable original.jsonl");
        fs::write(&selected, b"test backup").unwrap();
        fs::write(&retained, b"test original").unwrap();

        assert_eq!(recycle_many(&[selected.clone()]).unwrap(), 1);
        assert!(!selected.exists());
        assert_eq!(fs::read(&retained).unwrap(), b"test original");

        fs::remove_file(&retained).unwrap();
        fs::remove_dir(root).unwrap();
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
        assert_eq!(status_of(&a, &sizes(&[("f1", 10, 1500), ("f2", 5, 2000)]), true), "differs", "modified timestamps distinguish file versions");
        assert_eq!(status_of(&a, &Sizes::new(), true), "upload");
        assert_eq!(status_of(&Sizes::new(), &a, true), "download");
        assert_eq!(status_of(&a, &sizes(&[("f1", 10, 1000)]), true), "upload", "part of it is not in Drive yet");
        assert_eq!(status_of(&sizes(&[("f1", 10, 1000)]), &a, true), "download");
        assert_eq!(status_of(&a, &sizes(&[("f1", 99, 1000), ("f2", 5, 2000)]), true), "differs");
        assert_eq!(status_of(&sizes(&[("f1", 10, 1000), ("x", 1, 1)]), &sizes(&[("f1", 10, 1000), ("y", 1, 1)]), true), "differs");
        assert_eq!(status_of(&a, &Sizes::new(), false), "unchecked");
    }
}
