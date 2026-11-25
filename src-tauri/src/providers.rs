//! Parsers that turn one transcript line into displayable messages, for Claude Code and Codex.

use serde::Serialize;
use serde_json::{json, Value};
use std::collections::HashMap;

#[derive(Serialize, Clone)]
pub struct Msg {
    pub role: String,
    pub ts: String,
    pub text: String,
    /// text | summary | image | tool | result | reasoning
    pub kind: String,
    /// Tool call id, so a result can be matched to its call.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Tool name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Structured tool input, kept only for the tools the viewer renders specially.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<bool>,
    /// Native database items keep their completed tool output on the same item.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Box<Msg>>,
}

/// Pictures larger than this are shown as a placeholder instead of being sent to the viewer.
const IMAGE_MAX: usize = 4_000_000;

impl Msg {
    pub fn new(role: &str, ts: &str, text: String, kind: &str) -> Msg {
        Msg { role: role.into(), ts: ts.into(), text, kind: kind.into(), id: None, name: None, data: None, error: None, result: None }
    }
}

pub fn clip(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(n).collect();
        out.push('…');
        out
    }
}

fn claude_noise(t: &str) -> bool {
    let t = t.trim_start();
    t.starts_with("<system-reminder>")
        || t.starts_with("<local-command")
        || t.starts_with("<command-name>")
        || t.starts_with("<command-message>")
        || t.starts_with("<ide_")
        || t.starts_with("[Request interrupted")
}

const CODEX_NOISE: [&str; 14] = [
    "<environment_context",
    "<codex_internal_context",
    "<in-app-browser-context",
    "<external_codex_apps",
    "<send_user_message",
    "<image",
    "</image>",
    "<user_instructions",
    "<turn_aborted",
    "<permissions",
    "<app-context",
    "# AGENTS.md instructions",
    "# Files mentioned",
    "<collaboration_mode",
];

fn codex_noise(t: &str) -> bool {
    let t = t.trim_start();
    CODEX_NOISE.iter().any(|p| t.starts_with(p))
}

fn first_detail(inp: &Value, keys: &[&str]) -> String {
    keys.iter()
        .find_map(|k| inp.get(*k).and_then(|x| x.as_str()))
        .unwrap_or("")
        .lines()
        .next()
        .unwrap_or("")
        .to_string()
}

/// Tools whose input the viewer draws as a terminal, plan, agent or source list.
fn keeps_data(name: &str) -> bool {
    matches!(
        name,
        "Bash" | "PowerShell" | "TodoWrite" | "Task" | "Agent" | "WebFetch" | "WebSearch" | "update_plan" | "exec_command" | "shell" | "local_shell" | "shell_command"
    ) || name.ends_with("__update_plan")
        // Cards the AI app draws for you: the end-of-turn summary with its question and buttons, and questions with options.
        || name.ends_with("__wrap_up")
        || matches!(name, "AskUserQuestion" | "request_user_input" | "request_user_input_async" | "ask_user" | "ask_followup_question" | "question")
}

fn line_count(s: &str) -> i64 {
    if s.is_empty() { 0 } else { s.lines().count() as i64 }
}

/// Lines removed and added between two texts: lines present in only one of them (a fast stand-in for a diff, good
/// enough for "+12 −3").
fn changed_lines(old: &str, new: &str) -> (i64, i64) {
    let mut left: HashMap<&str, i64> = HashMap::new();
    for l in old.lines() {
        *left.entry(l).or_default() += 1;
    }
    let mut added = 0;
    for l in new.lines() {
        match left.get_mut(l) {
            Some(n) if *n > 0 => *n -= 1,
            _ => added += 1,
        }
    }
    (left.values().sum(), added)
}

/// Counts +/- lines of a unified diff or a Codex `apply_patch` body, and the files it names.
fn patch_stats(patch: &str) -> Option<(Vec<String>, i64, i64)> {
    let mut files = Vec::new();
    let (mut added, mut removed) = (0, 0);
    for l in patch.lines() {
        if let Some(f) = l.strip_prefix("*** Update File: ").or_else(|| l.strip_prefix("*** Add File: ")).or_else(|| l.strip_prefix("*** Delete File: ")).or_else(|| l.strip_prefix("+++ b/")) {
            files.push(f.trim().to_string());
        } else if l.starts_with("+++") || l.starts_with("---") || l.starts_with("***") {
        } else if l.starts_with('+') {
            added += 1;
        } else if l.starts_with('-') {
            removed += 1;
        }
    }
    (added + removed > 0 || !files.is_empty()).then_some((files, added, removed))
}

/// Cline's `replace_in_file`: SEARCH/REPLACE blocks.
fn search_replace_stats(diff: &str) -> Option<(i64, i64)> {
    if !diff.contains("<<<<<<< SEARCH") {
        return None;
    }
    let (mut removed, mut added) = (0, 0);
    for block in diff.split("<<<<<<< SEARCH").skip(1) {
        let (search, rest) = block.split_once("=======")?;
        let replace = rest.split(">>>>>>> REPLACE").next().unwrap_or("");
        let (r, a) = changed_lines(search.trim_matches('\n'), replace.trim_matches('\n'));
        removed += r;
        added += a;
    }
    Some((removed, added))
}

/// What a file-changing step did, from its input, the same way for every tool: Claude Code (Edit, MultiEdit, Write),
/// Codex (apply_patch, file changes), Gemini CLI (replace, write_file), OpenCode (edit, write), Cline
/// (replace_in_file, write_to_file) and Cursor. None for steps that change no file.
pub(crate) fn edit_info(input: &Value) -> Option<Value> {
    let s = |k: &str| input.get(k).and_then(|x| x.as_str());
    let path = ["file_path", "filePath", "path", "target_file", "absolute_path"].iter().find_map(|k| s(k)).map(String::from);
    let old = s("old_string").or(s("oldString")).or(s("old_str"));
    let new = s("new_string").or(s("newString")).or(s("new_str"));
    let (files, removed, added) = if let (Some(o), Some(n)) = (old, new) {
        let (r, a) = changed_lines(o, n);
        (path.into_iter().collect::<Vec<_>>(), r, a)
    } else if let Some(edits) = input.get("edits").and_then(|e| e.as_array()) {
        let (mut r, mut a) = (0, 0);
        for e in edits {
            let o = e.get("old_string").or(e.get("oldString")).and_then(|x| x.as_str()).unwrap_or("");
            let n = e.get("new_string").or(e.get("newString")).and_then(|x| x.as_str()).unwrap_or("");
            let (x, y) = changed_lines(o, n);
            r += x;
            a += y;
        }
        (path.into_iter().collect(), r, a)
    } else if let Some((r, a)) = s("diff").and_then(search_replace_stats) {
        (path.into_iter().collect(), r, a)
    } else if let Some(changes) = input.get("changes").and_then(|c| c.as_array()) {
        let (mut fs, mut r, mut a) = (Vec::new(), 0, 0);
        for c in changes {
            if let Some(p) = c.get("path").and_then(|x| x.as_str()) {
                fs.push(p.to_string());
            }
            if let Some((_, x, y)) = c.get("diff").and_then(|x| x.as_str()).and_then(patch_stats) {
                a += x;
                r += y;
            }
        }
        (fs, r, a)
    } else if let Some((fs, a, r)) = ["input", "patch", "cmd"].iter().find_map(|k| s(k)).filter(|p| p.contains("*** Begin Patch") || p.contains("\n@@")).and_then(patch_stats) {
        (fs, r, a)
    } else if let (Some(p), Some(content)) = (path.clone(), s("content").or(s("file_text")).or(s("contents"))) {
        (vec![p], 0, line_count(content))
    } else {
        return None;
    };
    if files.is_empty() && added + removed == 0 {
        return None;
    }
    Some(json!({ "files": files, "added": added, "removed": removed }))
}

/// Adds what the viewer shows for every step whatever the tool: the short description the AI gave it (`_desc`) and,
/// for a step that changed files, which ones and how many lines (`_edit`). The full input is not kept.
pub(crate) fn annotate(m: &mut Msg, input: Option<&Value>) {
    let Some(input) = input else { return };
    let desc = input.get("description").and_then(|x| x.as_str()).map(|d| clip(d.trim(), 160)).filter(|d| !d.is_empty());
    let edit = edit_info(input);
    if desc.is_none() && edit.is_none() {
        return;
    }
    let mut obj = match m.data.take() {
        Some(Value::Object(o)) => o,
        Some(other) => {
            m.data = Some(other);
            return;
        }
        None => serde_json::Map::new(),
    };
    if let Some(d) = desc {
        obj.insert("_desc".into(), Value::String(d));
    }
    if let Some(e) = edit {
        obj.insert("_edit".into(), e);
        // The file the step worked on, for the link in the step list.
        if !obj.contains_key("file_path") {
            if let Some(f) = obj.get("_edit").and_then(|e| e["files"].get(0)).cloned() {
                obj.insert("file_path".into(), f);
            }
        }
    }
    m.data = Some(Value::Object(obj));
}

/// Long strings inside tool input are shortened so a chat page stays small.
pub(crate) fn trim_value(v: &Value) -> Value {
    match v {
        Value::String(s) => Value::String(clip(s, 3000)),
        Value::Array(a) => Value::Array(a.iter().take(60).map(trim_value).collect()),
        Value::Object(o) => Value::Object(o.iter().map(|(k, x)| (k.clone(), trim_value(x))).collect()),
        other => other.clone(),
    }
}

/// The first readable text in an entry the viewer has no special drawing for (`text`, `content`, `message`, ...).
pub(crate) fn text_of(v: &Value) -> String {
    for k in ["text", "content", "message", "summary", "description", "error", "reason", "title"] {
        match v.get(k) {
            Some(Value::String(s)) if !s.trim().is_empty() && s != "None" => return s.trim().to_string(),
            Some(Value::Array(a)) => {
                let t = a.iter().filter_map(|x| x.get("text").and_then(|t| t.as_str())).collect::<Vec<_>>().join("\n");
                if !t.trim().is_empty() {
                    return t.trim().to_string();
                }
            }
            Some(o @ Value::Object(_)) => {
                let t = text_of(o);
                if !t.is_empty() {
                    return t;
                }
            }
            _ => {}
        }
    }
    String::new()
}

/// A line the AI app shows as a small note in the conversation (a command's output, "Conversation compacted", an error,
/// a pull request link) rather than as a message. `name` says what kind of note it is; `error` marks a warning.
pub(crate) fn notice(ts: &str, name: &str, text: &str, error: bool) -> Msg {
    let mut m = Msg::new("system", ts, clip(text.trim(), 4000), "notice");
    m.name = Some(name.to_string());
    if error {
        m.error = Some(true);
    }
    m
}

/// The text between `<tag>` and `</tag>`, if the string has that tag.
fn between<'a>(s: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let a = s.find(&open)? + open.len();
    let b = s[a..].find(&close)? + a;
    Some(&s[a..b])
}

/// Removes terminal colour codes from a command's output.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\u{1b}' {
            if it.peek() == Some(&'[') {
                it.next();
                for d in it.by_ref() {
                    if d.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// User lines that Claude Code writes as tagged text: a slash command you ran, its output, a background task's
/// report, and pasted text. Returns None for ordinary text.
fn claude_tagged_user(ts: &str, t: &str) -> Option<Vec<Msg>> {
    let s = t.trim_start();
    if s.starts_with("<command-name>") || s.starts_with("<command-message>") {
        let name = between(s, "command-name").unwrap_or("").trim();
        let args = between(s, "command-args").unwrap_or("").trim();
        let cmd = if name.starts_with('/') { name.to_string() } else { format!("/{name}") };
        let text = if args.is_empty() { cmd } else { format!("{cmd} {args}") };
        return Some(if name.is_empty() { Vec::new() } else { vec![Msg::new("user", ts, text, "text")] });
    }
    if s.starts_with("<local-command-stdout>") || s.starts_with("<local-command-stderr>") {
        let err = s.starts_with("<local-command-stderr>");
        let body = between(s, if err { "local-command-stderr" } else { "local-command-stdout" }).unwrap_or("");
        let body = strip_ansi(body);
        return Some(if body.trim().is_empty() { Vec::new() } else { vec![notice(ts, "command-output", &body, err)] });
    }
    if s.starts_with("<task-notification>") {
        let summary = between(s, "summary").or_else(|| between(s, "status")).unwrap_or("A background task finished");
        let failed = between(s, "status").map(|st| st.contains("fail") || st.contains("error")).unwrap_or(false);
        return Some(vec![notice(ts, "task", summary, failed)]);
    }
    if s.starts_with("<local-command-caveat>") || s.starts_with("<system-reminder>") || s.starts_with("<ide_") {
        return Some(Vec::new());
    }
    if s.contains("<pasted_content") {
        // Text you pasted: shown as it was pasted, without the wrapper tags.
        let mut cleaned = String::new();
        for line in t.lines() {
            let l = line.trim();
            if (l.starts_with("<pasted_content") || l.starts_with("</pasted_content")) && l.ends_with('>') {
                continue;
            }
            cleaned.push_str(line);
            cleaned.push('\n');
        }
        return Some(vec![Msg::new("user", ts, cleaned.trim().to_string(), "text")]);
    }
    None
}

/// One Claude Code transcript line.
pub fn claude_messages(v: &Value) -> Vec<Msg> {
    let mut out = Vec::new();
    let ty = v.get("type").and_then(|x| x.as_str()).unwrap_or("");
    let ts0 = v.get("timestamp").and_then(|x| x.as_str()).unwrap_or("").to_string();
    // Notes Claude Code shows in the conversation. Its bookkeeping (hook summaries) stays hidden, as in the app.
    if ty == "system" {
        let sub = v.get("subtype").and_then(|x| x.as_str()).unwrap_or("");
        let level = v.get("level").and_then(|x| x.as_str()).unwrap_or("");
        let warn = level == "error" || level == "warning";
        match sub {
            "stop_hook_summary" | "turn_duration" => {}
            "compact_boundary" => out.push(notice(&ts0, "compact", "Conversation compacted", false)),
            "api_error" => {
                let detail = text_of(v);
                out.push(notice(&ts0, "api-error", if detail.is_empty() { "The AI service returned an error, and Claude Code tried again." } else { &detail }, true));
            }
            _ => {
                let raw = text_of(v);
                let body = between(&raw, "local-command-stdout").map(strip_ansi).unwrap_or(raw);
                if !body.trim().is_empty() {
                    out.push(notice(&ts0, if sub.is_empty() { "system" } else { sub }, &body, warn));
                }
            }
        }
        return out;
    }
    if ty == "pr-link" {
        let url = v.get("prUrl").and_then(|x| x.as_str()).unwrap_or("");
        let num = v.get("prNumber").map(|n| n.as_str().map(String::from).unwrap_or_else(|| n.to_string())).unwrap_or_default();
        let repo = v.get("prRepository").and_then(|x| x.as_str()).unwrap_or("");
        if !url.is_empty() {
            let mut m = notice(&ts0, "pull-request", &format!("Pull request #{num} {repo}").trim().to_string(), false);
            m.data = Some(json!({ "url": url }));
            out.push(m);
        }
        return out;
    }
    // A message you typed while Claude was working: Claude Code saves it as a "queued_command" attachment and reads
    // it out to Claude mid-turn. It is your message, so it is shown as one, where it happened.
    if ty == "attachment" {
        let a = v.get("attachment").unwrap_or(&Value::Null);
        let human = a.get("origin").and_then(|o| o.get("kind")).and_then(|k| k.as_str()).map(|k| k == "human").unwrap_or(false)
            || a.get("humanTurn").and_then(|h| h.as_bool()).unwrap_or(false);
        let prompt_mode = a.get("commandMode").and_then(|m| m.as_str()).map(|m| m == "prompt").unwrap_or(true);
        if a.get("type").and_then(|t| t.as_str()) == Some("queued_command") && human && prompt_mode {
            let text = match a.get("prompt") {
                Some(Value::String(s)) => s.clone(),
                Some(Value::Array(parts)) => parts.iter().filter_map(|p| p.get("text").and_then(|t| t.as_str())).collect::<Vec<_>>().join("\n"),
                _ => String::new(),
            };
            if !text.trim().is_empty() && !claude_noise(&text) {
                let ts = v.get("timestamp").and_then(|x| x.as_str()).unwrap_or("").to_string();
                let mut m = Msg::new("user", &ts, text, "text");
                // Marks a message typed during a turn: it does not start a new turn, so the turn's plan stays last.
                m.name = Some("queued".into());
                out.push(m);
            }
        }
        // A file you mentioned with @: the app shows it with your message.
        if a.get("type").and_then(|t| t.as_str()) == Some("file") {
            let shown = a.get("displayPath").or(a.get("filename")).and_then(|x| x.as_str()).unwrap_or("");
            if !shown.is_empty() {
                out.push(notice(&ts0, "attachment", &format!("Attached {shown}"), false));
            }
        }
        return out;
    }
    if ty != "user" && ty != "assistant" {
        return out;
    }
    if v.get("isMeta").and_then(|x| x.as_bool()).unwrap_or(false) {
        return out;
    }
    let ts = v.get("timestamp").and_then(|x| x.as_str()).unwrap_or("").to_string();
    let role = ty.to_string();
    let Some(content) = v.get("message").and_then(|m| m.get("content")) else { return out };
    let push_text = |out: &mut Vec<Msg>, t: &str| {
        if t.trim().is_empty() {
            return;
        }
        if role == "user" {
            if let Some(msgs) = claude_tagged_user(&ts, t) {
                out.extend(msgs);
                return;
            }
            if claude_noise(t) {
                return;
            }
        }
        let kind = if role == "user" && t.starts_with("This session is being continued") { "summary" } else { "text" };
        out.push(Msg::new(&role, &ts, t.to_string(), kind));
    };
    match content {
        Value::String(s) => push_text(&mut out, s),
        Value::Array(blocks) => {
            for b in blocks {
                match b.get("type").and_then(|x| x.as_str()).unwrap_or("") {
                    "text" => {
                        if let Some(t) = b.get("text").and_then(|x| x.as_str()) {
                            push_text(&mut out, t);
                        }
                    }
                    "thinking" => {
                        let t = b.get("thinking").and_then(|x| x.as_str()).unwrap_or("");
                        if !t.trim().is_empty() {
                            out.push(Msg::new(&role, &ts, clip(t, 6000), "reasoning"));
                        }
                    }
                    "image" => {
                        let mut m = Msg::new(&role, &ts, "[Image]".into(), "image");
                        let src = b.get("source");
                        let kind = src.and_then(|s| s.get("type")).and_then(|x| x.as_str()).unwrap_or("");
                        let url = if kind == "base64" {
                            let mt = src.and_then(|s| s.get("media_type")).and_then(|x| x.as_str()).unwrap_or("image/png");
                            src.and_then(|s| s.get("data")).and_then(|x| x.as_str()).map(|d| format!("data:{mt};base64,{d}"))
                        } else {
                            src.and_then(|s| s.get("url")).and_then(|x| x.as_str()).map(|u| u.to_string())
                        };
                        m.data = url.filter(|u| u.len() <= IMAGE_MAX).map(Value::String);
                        out.push(m);
                    }
                    "tool_use" => {
                        let name = b.get("name").and_then(|x| x.as_str()).unwrap_or("tool");
                        let input = b.get("input");
                        let detail = input
                            .map(|i| first_detail(i, &["description", "command", "file_path", "pattern", "url", "query", "prompt"]))
                            .unwrap_or_default();
                        let mut m = Msg::new(&role, &ts, format!("{}: {}", name, clip(&detail, 160)), "tool");
                        m.id = b.get("id").and_then(|x| x.as_str()).map(String::from);
                        m.name = Some(name.to_string());
                        if keeps_data(name) {
                            m.data = input.map(trim_value);
                        }
                        annotate(&mut m, input);
                        out.push(m);
                    }
                    "tool_result" => {
                        let body = match b.get("content") {
                            Some(Value::String(s)) => s.clone(),
                            Some(Value::Array(a)) => a
                                .iter()
                                .filter_map(|x| x.get("text").and_then(|t| t.as_str()))
                                .collect::<Vec<_>>()
                                .join("\n"),
                            _ => String::new(),
                        };
                        let mut m = Msg::new(&role, &ts, clip(&body, 3000), "result");
                        m.id = b.get("tool_use_id").and_then(|x| x.as_str()).map(String::from);
                        m.error = b.get("is_error").and_then(|x| x.as_bool());
                        out.push(m);
                    }
                    "document" => {
                        let title = b.get("title").and_then(|x| x.as_str()).unwrap_or("");
                        let kind = b.get("source").and_then(|s| s.get("media_type")).and_then(|x| x.as_str()).map(|t| if t.contains("pdf") { "PDF" } else { "document" }).unwrap_or("document");
                        out.push(notice(&ts, "attachment", &if title.is_empty() { format!("Attached a {kind}") } else { format!("Attached {title}") }, false));
                    }
                    // Anything newer that this reader does not know yet: shown as a note with its text, never dropped.
                    other => {
                        let t = text_of(b);
                        if !t.is_empty() && other != "redacted_thinking" {
                            out.push(notice(&ts, other, &t, false));
                        }
                    }
                }
            }
        }
        _ => {}
    }
    out
}

/// Codex wraps command output as JSON with an exit code; unwrap it when it does.
fn codex_output(raw: &str) -> (String, Option<bool>) {
    if let Ok(v) = serde_json::from_str::<Value>(raw) {
        if let Some(o) = v.get("output").and_then(|x| x.as_str()) {
            let failed = v
                .get("metadata")
                .and_then(|m| m.get("exit_code"))
                .and_then(|c| c.as_i64())
                .map(|c| c != 0);
            return (clip(o, 3000), failed);
        }
    }
    let failed = raw
        .lines()
        .find_map(|l| l.strip_prefix("Exit code: ").or_else(|| l.strip_prefix("Process exited with code ")))
        .and_then(|c| c.trim().parse::<i64>().ok())
        .map(|c| c != 0);
    (clip(raw, 3000), failed)
}

/// `"exit_code":N` anywhere in the output (newer Codex puts each tool's answer as JSON inside the text).
fn nested_exit_failed(raw: &str) -> Option<bool> {
    let at = raw.find("\"exit_code\":")?;
    let rest = raw[at + 12..].trim_start();
    let num: String = rest.chars().take_while(|c| c.is_ascii_digit() || *c == '-').collect();
    num.parse::<i64>().ok().map(|c| c != 0)
}

/// One Codex rollout line.
pub fn codex_messages(v: &Value) -> Vec<Msg> {
    let mut out = Vec::new();
    let top = v.get("type").and_then(|x| x.as_str()).unwrap_or("");
    let ts = v.get("timestamp").and_then(|x| x.as_str()).unwrap_or("").to_string();
    let Some(p) = v.get("payload") else { return out };
    let pt = p.get("type").and_then(|x| x.as_str()).unwrap_or("");
    // What Codex shows besides messages and steps. Its bookkeeping (token counts, settings, turn context) stays hidden.
    match top {
        "compacted" => {
            out.push(notice(&ts, "compact", "Conversation compacted", false));
            return out;
        }
        "event_msg" => {
            match pt {
                "turn_aborted" => {
                    let why = p.get("reason").and_then(|x| x.as_str()).unwrap_or("");
                    out.push(notice(&ts, "stopped", if why == "interrupted" { "You stopped this turn" } else { "The turn was stopped" }, false));
                }
                "error" | "stream_error" => {
                    let t = text_of(p);
                    out.push(notice(&ts, "error", if t.is_empty() { "Codex reported an error" } else { &t }, true));
                }
                _ => {}
            }
            return out;
        }
        "realtime_item" => {
            // Voice conversations: what you said and what Codex answered.
            if pt == "transcript_segment" {
                let role = if p.get("role").and_then(|x| x.as_str()) == Some("user") { "user" } else { "assistant" };
                let t = p.get("text").and_then(|x| x.as_str()).unwrap_or("").trim();
                if !t.is_empty() {
                    out.push(Msg::new(role, &ts, t.to_string(), "text"));
                }
            }
            return out;
        }
        "response_item" => {}
        _ => return out,
    }
    match pt {
        "message" => {
            let role = p.get("role").and_then(|x| x.as_str()).unwrap_or("");
            if role != "user" && role != "assistant" {
                return out;
            }
            let Some(blocks) = p.get("content").and_then(|c| c.as_array()) else { return out };
            for b in blocks {
                match b.get("type").and_then(|x| x.as_str()).unwrap_or("") {
                    "input_text" | "output_text" => {
                        let t = b.get("text").and_then(|x| x.as_str()).unwrap_or("");
                        if t.trim().is_empty() || (role == "user" && codex_noise(t)) {
                            continue;
                        }
                        out.push(Msg::new(role, &ts, t.to_string(), "text"));
                    }
                    "input_image" => {
                        let mut m = Msg::new(role, &ts, "[Image]".into(), "image");
                        m.data = b.get("image_url").and_then(|x| x.as_str()).filter(|u| u.len() <= IMAGE_MAX).map(|u| Value::String(u.to_string()));
                        out.push(m);
                    }
                    _ => {}
                }
            }
        }
        "reasoning" => {
            let text = p
                .get("summary")
                .and_then(|s| s.as_array())
                .map(|a| a.iter().filter_map(|x| x.get("text").and_then(|t| t.as_str())).collect::<Vec<_>>().join("\n\n"))
                .unwrap_or_default();
            if !text.trim().is_empty() {
                out.push(Msg::new("assistant", &ts, clip(&text, 6000), "reasoning"));
            }
        }
        "function_call" => {
            let name = p.get("name").and_then(|x| x.as_str()).unwrap_or("tool");
            let args = p.get("arguments").and_then(|x| x.as_str()).unwrap_or("");
            let parsed = serde_json::from_str::<Value>(args).ok();
            let detail = parsed
                .as_ref()
                .map(|a| first_detail(a, &["cmd", "command", "path", "query", "url", "explanation"]))
                .unwrap_or_else(|| args.lines().next().unwrap_or("").to_string());
            let mut m = Msg::new("assistant", &ts, format!("{}: {}", name, clip(&detail, 160)), "tool");
            m.id = p.get("call_id").and_then(|x| x.as_str()).map(String::from);
            m.name = Some(name.to_string());
            if keeps_data(name) {
                m.data = parsed.as_ref().map(trim_value);
            }
            annotate(&mut m, parsed.as_ref());
            out.push(m);
        }
        "custom_tool_call" => {
            let name = p.get("name").and_then(|x| x.as_str()).unwrap_or("tool");
            let input = p.get("input").and_then(|x| x.as_str()).unwrap_or("");
            let mut m = Msg::new("assistant", &ts, format!("{}: {}", name, clip(input.lines().next().unwrap_or(""), 160)), "tool");
            m.id = p.get("call_id").and_then(|x| x.as_str()).map(String::from);
            m.name = Some(name.to_string());
            m.data = Some(json!({ "cmd": clip(input, 3000) }));
            // apply_patch sends the patch itself as the input: count its lines from the full text.
            annotate(&mut m, Some(&json!({ "input": input })));
            out.push(m);
        }
        "function_call_output" | "custom_tool_call_output" => {
            // Older Codex stores the output as one string; newer Codex as a list of text parts (the script's own
            // summary, then each tool's JSON answer with its exit_code). Both are read.
            let joined;
            let raw = match p.get("output") {
                Some(Value::String(s)) => s.as_str(),
                Some(Value::Array(parts)) => {
                    joined = parts.iter().filter_map(|x| x.get("text").and_then(|t| t.as_str())).collect::<Vec<_>>().join("\n");
                    joined.as_str()
                }
                _ => "",
            };
            let (mut text, mut failed) = codex_output(raw);
            if failed.is_none() {
                failed = nested_exit_failed(raw);
            }
            // A part that is a tool's JSON answer: show its output text instead of the JSON.
            if let Some(o) = raw.lines().find_map(|l| serde_json::from_str::<Value>(l.trim()).ok().and_then(|v| v.get("output").and_then(|x| x.as_str()).map(String::from))) {
                text = clip(&o, 3000);
            }
            let mut m = Msg::new("assistant", &ts, text, "result");
            m.id = p.get("call_id").and_then(|x| x.as_str()).map(String::from);
            m.error = failed;
            out.push(m);
        }
        // Codex's own setup (tool lists, settings) is not part of the conversation.
        "additional_tools" | "configuration_update" => {}
        // A kind of step this reader does not know yet (for example a web search or a new tool type): shown as a step
        // under its own name with whatever it says, never dropped.
        other => {
            let t = text_of(p);
            let detail = if t.is_empty() { p.get("action").map(|a| text_of(a)).unwrap_or_default() } else { t };
            let mut m = Msg::new("assistant", &ts, format!("{}: {}", other, clip(detail.lines().next().unwrap_or(""), 160)), "tool");
            m.name = Some(other.to_string());
            m.id = p.get("call_id").or(p.get("id")).and_then(|x| x.as_str()).map(String::from);
            out.push(m);
        }
    }
    out
}

pub fn messages_for(provider: &str, v: &Value) -> Vec<Msg> {
    if provider == "codex" {
        codex_messages(v)
    } else {
        claude_messages(v)
    }
}

/// Kinds that carry readable conversation, as opposed to tool traffic.
pub fn is_readable(m: &Msg) -> bool {
    matches!(m.kind.as_str(), "text" | "summary" | "image")
}

#[cfg(test)]
mod step_tests {
    use super::*;

    #[test]
    fn edits_are_counted_for_every_tool_shape() {
        // Claude Edit / Gemini replace / OpenCode edit
        let e = edit_info(&json!({"file_path": "a.ts", "old_string": "x\ny", "new_string": "x\nz\nw"})).unwrap();
        assert_eq!((e["added"].as_i64(), e["removed"].as_i64()), (Some(2), Some(1)));
        assert!(edit_info(&json!({"filePath": "a.ts", "oldString": "a", "newString": "b"})).is_some());
        // Claude Write / Gemini write_file / Cline write_to_file
        let w = edit_info(&json!({"file_path": "n.md", "content": "1\n2\n3"})).unwrap();
        assert_eq!(w["added"].as_i64(), Some(3));
        // Codex apply_patch inside an exec script
        let p = edit_info(&json!({"input": "*** Begin Patch\n*** Update File: src/a.rs\n@@\n-old\n+new\n+more\n*** End Patch"})).unwrap();
        assert_eq!((p["added"].as_i64(), p["removed"].as_i64(), p["files"][0].as_str()), (Some(2), Some(1), Some("src/a.rs")));
        // Cline replace_in_file
        let c = edit_info(&json!({"path": "b.ts", "diff": "<<<<<<< SEARCH\nold\n=======\nnew\nnewer\n>>>>>>> REPLACE"})).unwrap();
        assert_eq!((c["added"].as_i64(), c["removed"].as_i64()), (Some(2), Some(1)));
        // Not an edit
        assert!(edit_info(&json!({"command": "ls"})).is_none());
        assert!(edit_info(&json!({"file_path": "a.ts"})).is_none());
    }

    #[test]
    fn messages_typed_mid_turn_are_shown_as_yours() {
        let v = json!({"type": "attachment", "timestamp": "2026-11-25T12:22:02Z", "attachment": {"type": "queued_command", "prompt": "check codex too", "commandMode": "prompt", "origin": {"kind": "human"}}});
        let m = claude_messages(&v);
        assert_eq!(m.len(), 1);
        assert_eq!((m[0].role.as_str(), m[0].kind.as_str(), m[0].text.as_str()), ("user", "text", "check codex too"));
        // Not typed by a person: left out.
        let sys = json!({"type": "attachment", "attachment": {"type": "queued_command", "prompt": "x", "commandMode": "task-notification", "origin": {"kind": "task"}}});
        assert!(claude_messages(&sys).is_empty());
    }

    #[test]
    fn newer_codex_output_keeps_text_and_exit_code() {
        let v = json!({"type": "response_item", "timestamp": "t", "payload": {"type": "custom_tool_call_output", "call_id": "c1", "output": [
            {"type": "input_text", "text": "Script completed"},
            {"type": "input_text", "text": "{\"exit_code\":2,\"output\":\"boom\"}"}
        ]}});
        let m = codex_messages(&v);
        assert_eq!(m[0].text, "boom");
        assert_eq!(m[0].error, Some(true));
    }
}

#[cfg(test)]
mod notice_tests {
    use super::*;

    fn one(v: Value) -> Msg {
        let mut m = claude_messages(&v);
        assert_eq!(m.len(), 1, "{v}");
        m.remove(0)
    }

    #[test]
    fn claude_commands_outputs_and_system_notes_are_shown() {
        let c = one(json!({"type":"user","timestamp":"t","message":{"role":"user","content":"<command-name>/model</command-name>\n<command-message>model</command-message>\n<command-args>claude-opus-5-5</command-args>"}}));
        assert_eq!((c.role.as_str(), c.text.as_str()), ("user", "/model claude-opus-5-5"));
        let o = one(json!({"type":"user","timestamp":"t","message":{"role":"user","content":"<local-command-stdout>Set model to \u{1b}[1mOpus\u{1b}[22m</local-command-stdout>"}}));
        assert_eq!((o.kind.as_str(), o.text.as_str()), ("notice", "Set model to Opus"));
        assert!(claude_messages(&json!({"type":"user","message":{"role":"user","content":"<local-command-caveat>x</local-command-caveat>"}})).is_empty());
        let k = one(json!({"type":"system","subtype":"compact_boundary","content":"Conversation compacted","timestamp":"t"}));
        assert_eq!(k.text, "Conversation compacted");
        assert!(claude_messages(&json!({"type":"system","subtype":"stop_hook_summary"})).is_empty());
        let w = one(json!({"type":"system","subtype":"informational","level":"warning","content":"A hook blocked the turn"}));
        assert_eq!((w.kind.as_str(), w.error), ("notice", Some(true)));
        let pr = one(json!({"type":"pr-link","prNumber":"860","prUrl":"https://github.com/a/b/pull/860","prRepository":"a/b"}));
        assert_eq!(pr.text, "Pull request #860 a/b");
        let p = one(json!({"type":"user","message":{"role":"user","content":"<pasted_content id=\"1\">\nhello\n</pasted_content id=\"1\">"}}));
        assert_eq!(p.text, "hello");
    }

    #[test]
    fn unknown_blocks_and_codex_entries_are_never_dropped() {
        let u = one(json!({"type":"assistant","message":{"role":"assistant","content":[{"type":"brand_new_block","text":"something new"}]}}));
        assert_eq!((u.kind.as_str(), u.name.as_deref(), u.text.as_str()), ("notice", Some("brand_new_block"), "something new"));
        let a = codex_messages(&json!({"timestamp":"t","type":"event_msg","payload":{"type":"turn_aborted","reason":"interrupted"}}));
        assert_eq!(a[0].text, "You stopped this turn");
        let r = codex_messages(&json!({"timestamp":"t","type":"realtime_item","payload":{"type":"transcript_segment","role":"user","text":" Hello"}}));
        assert_eq!((r[0].role.as_str(), r[0].text.as_str()), ("user", "Hello"));
        let n = codex_messages(&json!({"timestamp":"t","type":"response_item","payload":{"type":"web_search_call","action":{"query":"tauri docs"}}}));
        assert_eq!((n[0].kind.as_str(), n[0].name.as_deref()), ("tool", Some("web_search_call")));
        assert!(codex_messages(&json!({"timestamp":"t","type":"event_msg","payload":{"type":"token_count"}})).is_empty());
    }
}
