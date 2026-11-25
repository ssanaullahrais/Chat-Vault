//! Current Codex desktop/CLI history after the rollout-to-SQLite migration.
//! Source databases are opened read-only; pages query only the selected thread.
use crate::{
    paths::*,
    providers::{Msg, clip},
};
use rusqlite::{Connection, OpenFlags, params};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

fn open(path: &Path) -> Result<Connection, String> {
    Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(|e| e.to_string())
}
fn newest(root: &Path, prefix: &str) -> Option<PathBuf> {
    std::fs::read_dir(root)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter_map(|p| {
            let n = p
                .file_name()?
                .to_str()?
                .strip_prefix(prefix)?
                .strip_suffix(".sqlite")?
                .parse::<u32>()
                .ok()?;
            Some((n, p))
        })
        .max_by_key(|(n, _)| *n)
        .map(|(_, p)| p)
}
fn sources() -> Vec<(PathBuf, PathBuf)> {
    codex_roots()
        .into_iter()
        .filter_map(|r| Some((newest(&r, "state_")?, newest(&r, "thread_history_")?)))
        .collect()
}
/// The Codex databases that are read: the newest state and thread-history file in each Codex folder.
pub fn database_files() -> Vec<PathBuf> {
    sources().into_iter().flat_map(|(a, b)| [a, b]).collect()
}
pub fn fingerprint() -> i64 {
    sources()
        .into_iter()
        .flat_map(|(a, b)| {
            [
                a.clone(),
                PathBuf::from(format!("{}-wal", a.display())),
                b.clone(),
                PathBuf::from(format!("{}-wal", b.display())),
            ]
        })
        .filter_map(|p| {
            std::fs::metadata(p)
                .ok()?
                .modified()
                .ok()?
                .duration_since(std::time::UNIX_EPOCH)
                .ok()
                .map(|t| t.as_millis() as i64)
        })
        .max()
        .unwrap_or(0)
}
pub struct DbChat {
    pub id: String,
    pub title: String,
    pub cwd: Option<String>,
    pub created: i64,
    pub updated: i64,
    pub archived: bool,
    pub account: String,
    pub model: Option<String>,
    pub path: String,
}
fn metadata_query(c: &Connection) -> Result<String, String> {
    let mut st = c
        .prepare("PRAGMA table_info(threads)")
        .map_err(|e| e.to_string())?;
    let cols: std::collections::HashSet<String> = st
        .query_map([], |r| r.get(1))
        .map_err(|e| e.to_string())?
        .flatten()
        .collect();
    let col = |name: &str, fallback: &str| {
        if cols.contains(name) {
            name.to_string()
        } else {
            fallback.to_string()
        }
    };
    let title = format!(
        "COALESCE(NULLIF({},''),{},'Untitled')",
        col("name", "NULL"),
        col("title", "NULL")
    );
    let created = format!(
        "COALESCE({},{},0)",
        col("created_at_ms", "NULL"),
        if cols.contains("created_at") {
            "created_at*1000"
        } else {
            "NULL"
        }
    );
    let updated = format!(
        "COALESCE({},{},0)",
        col("updated_at_ms", "NULL"),
        if cols.contains("updated_at") {
            "updated_at*1000"
        } else {
            "NULL"
        }
    );
    Ok(format!(
        "SELECT id,{title},{},{created},{updated},{},COALESCE({},'unknown'),{} FROM threads{}",
        col("cwd", "NULL"),
        col("archived", "0"),
        col("creator_account_id", "NULL"),
        col("model", "NULL"),
        if cols.contains("history_mode") {
            " WHERE history_mode='paginated'"
        } else {
            ""
        }
    ))
}
pub fn scan() -> Vec<DbChat> {
    let mut chats = Vec::new();
    for (state, history) in sources() {
        let Ok(c) = open(&state) else { continue };
        let Ok(h) = open(&history) else { continue };
        let Ok(sql) = metadata_query(&c) else {
            continue;
        };
        let Ok(mut q) = c.prepare(&sql) else { continue };
        let Ok(rows) = q.query_map([], |r| {
            Ok(DbChat {
                id: r.get(0)?,
                title: r.get(1)?,
                cwd: r.get(2)?,
                created: r.get(3)?,
                updated: r.get(4)?,
                archived: r.get::<_, i64>(5)? != 0,
                account: r.get(6)?,
                model: r.get(7)?,
                path: String::new(),
            })
        }) else {
            continue;
        };
        for mut chat in rows.flatten() {
            let exists = h
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM thread_items WHERE thread_id=?1)",
                    [&chat.id],
                    |r| r.get::<_, bool>(0),
                )
                .unwrap_or(false);
            if !exists {
                continue;
            }
            chat.path = format!("codex-db:{}|{}", state.display(), chat.id);
            chats.push(chat);
        }
    }
    chats
}
pub fn real_file(path: &str) -> Result<PathBuf, String> {
    let (state, _) = path
        .strip_prefix("codex-db:")
        .and_then(|s| s.rsplit_once('|'))
        .ok_or("Invalid Codex database chat")?;
    let canon = Path::new(state).canonicalize().map_err(|e| e.to_string())?;
    if sources()
        .iter()
        .any(|(s, _)| s.canonicalize().ok().as_ref() == Some(&canon))
    {
        Ok(canon)
    } else {
        Err("This database is outside the Codex chat folders".into())
    }
}
fn source(path: &str) -> Result<(Connection, String), String> {
    let state = real_file(path)?;
    let id = path.rsplit_once('|').ok_or("Invalid chat")?.1.to_string();
    let history = newest(
        state.parent().ok_or("Missing Codex home")?,
        "thread_history_",
    )
    .ok_or("Codex history database was not found")?;
    Ok((open(&history)?, id))
}
fn texts(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(a) => a
            .iter()
            .map(|p| {
                p.as_str().map(String::from).unwrap_or_else(|| {
                    p.get("text")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string()
                })
            })
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}
fn item(v: &Value, ms: i64) -> Msg {
    let ty = v.get("type").and_then(Value::as_str).unwrap_or("unknown");
    let ts = chrono::DateTime::from_timestamp_millis(ms)
        .map(|t| t.to_rfc3339())
        .unwrap_or_default();
    let field = |k: &str| v.get(k).map(texts).unwrap_or_default();
    let mut m = match ty {
        "userMessage" => {
            let text = field("content");
            Msg::new(
                "user",
                &ts,
                if text.is_empty() {
                    "[Attachment or session context]".into()
                } else {
                    text
                },
                "text",
            )
        }
        "agentMessage" => Msg::new("assistant", &ts, field("text"), "text"),
        "reasoning" => Msg::new(
            "assistant",
            &ts,
            clip(&format!("{}\n{}", field("summary"), field("content")), 6000),
            "reasoning",
        ),
        "contextCompaction" => Msg::new(
            "assistant",
            &ts,
            "Conversation context compacted".into(),
            "summary",
        ),
        _ => {
            let name = match ty {
                "commandExecution" => "exec_command",
                "plan" => "update_plan",
                "mcpToolCall" => v.get("tool").and_then(Value::as_str).unwrap_or("MCP tool"),
                _ => ty,
            };
            let detail = if ty == "commandExecution" {
                field("command")
            } else {
                field("query")
            };
            let mut m = Msg::new(
                "assistant",
                &ts,
                format!("{name}: {}", clip(&detail, 160)),
                "tool",
            );
            m.name = Some(name.into());
            m.data = Some(crate::providers::trim_value(&if ty == "commandExecution" {
                json!({"command":field("command"),"cwd":v.get("cwd")})
            } else {
                v.get("arguments").cloned().unwrap_or_else(|| v.clone())
            }));
            // File changes carry their diffs: count the lines from the full item.
            crate::providers::annotate(&mut m, Some(v));
            m.error = Some(
                v.get("exitCode")
                    .and_then(Value::as_i64)
                    .is_some_and(|c| c != 0)
                    || v.get("status").and_then(Value::as_str) == Some("failed"),
            );
            let output = if ty == "commandExecution" {
                field("aggregatedOutput")
            } else {
                v.get("result")
                    .map(|r| {
                        let content = r.get("content").map(texts).unwrap_or_default();
                        if content.is_empty() {
                            r.to_string()
                        } else {
                            content
                        }
                    })
                    .unwrap_or_default()
            };
            if !output.is_empty() {
                let mut result = Msg::new("assistant", &ts, clip(&output, 8000), "result");
                result.error = m.error;
                m.result = Some(Box::new(result));
            }
            m
        }
    };
    m.id = v.get("id").and_then(Value::as_str).map(String::from);
    m
}
pub fn page(
    path: &str,
    before: Option<usize>,
    limit: usize,
) -> Result<(Vec<Msg>, usize, usize), String> {
    let (c, id) = source(path)?;
    page_from(&c, &id, before, limit)
}
fn page_from(
    c: &Connection,
    id: &str,
    before: Option<usize>,
    limit: usize,
) -> Result<(Vec<Msg>, usize, usize), String> {
    // Keep count and rows on the same SQLite snapshot while the source app writes.
    c.execute_batch("BEGIN").map_err(|e| e.to_string())?;
    let total = c
        .query_row(
            "SELECT COUNT(*) FROM thread_items WHERE thread_id=?1",
            [id],
            |r| r.get::<_, i64>(0),
        )
        .map_err(|e| e.to_string())? as usize;
    let end = before.unwrap_or(total).min(total);
    let start = end.saturating_sub(limit);
    let mut q=c.prepare("SELECT item_json,created_at_ms FROM thread_items WHERE thread_id=?1 ORDER BY rollout_ordinal,item_id LIMIT ?2 OFFSET ?3").map_err(|e|e.to_string())?;
    let rows = q
        .query_map(params![id, (end - start) as i64, start as i64], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
        })
        .map_err(|e| e.to_string())?;
    let mut messages = Vec::new();
    for row in rows {
        let (raw, ms) = row.map_err(|e| e.to_string())?;
        let v = serde_json::from_str(&raw)
            .map_err(|e| format!("Cannot read Codex history item: {e}"))?;
        messages.push(item(&v, ms));
    }
    Ok((messages, total, start))
}
pub fn read(path: &str) -> Result<Vec<Msg>, String> {
    page(path, None, usize::MAX).map(|p| p.0)
}

pub fn stamp(path: &str) -> Result<(u64, u64), String> {
    let (c, id) = source(path)?;
    let modern = "SELECT COUNT(*),COALESCE(MAX(updated_at_ordinal),0),COALESCE(MAX(COALESCE(completed_at_ms,started_at_ms,created_at_ms)),0) FROM thread_items WHERE thread_id=?1";
    let legacy = "SELECT COUNT(*),COALESCE(MAX(rollout_ordinal),0),COALESCE(MAX(created_at_ms),0) FROM thread_items WHERE thread_id=?1";
    let mut q = c
        .prepare(modern)
        .or_else(|_| c.prepare(legacy))
        .map_err(|e| e.to_string())?;
    let (count, ordinal, ms) = q
        .query_row([id], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, i64>(2)?,
            ))
        })
        .map_err(|e| e.to_string())?;
    Ok((
        (count as u64).rotate_left(32) ^ ordinal as u64,
        ms.max(0) as u64,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn older_metadata_schema_does_not_require_new_columns() {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch("CREATE TABLE threads(id TEXT,title TEXT,created_at INTEGER,updated_at INTEGER); INSERT INTO threads VALUES('fixture','Older schema',2,3)").unwrap();
        let sql = metadata_query(&c).unwrap();
        let (title, created, account) = c
            .query_row(&sql, [], |r| {
                Ok((
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, String>(6)?,
                ))
            })
            .unwrap();
        assert_eq!(title, "Older schema");
        assert_eq!(created, 2000);
        assert_eq!(account, "unknown");
    }
    #[test]
    fn native_items_keep_user_assistant_reasoning_and_tools() {
        assert_eq!(
            item(
                &json!({"type":"userMessage","content":[{"type":"text","text":"question"}]}),
                0
            )
            .text,
            "question"
        );
        assert_eq!(
            item(&json!({"type":"agentMessage","text":"answer"}), 0).role,
            "assistant"
        );
        assert_eq!(
            item(&json!({"type":"reasoning","summary":["thought"]}), 0).kind,
            "reasoning"
        );
        let m = item(
            &json!({"type":"commandExecution","command":"echo hello","exitCode":1}),
            0,
        );
        assert_eq!(m.name.as_deref(), Some("exec_command"));
        assert_eq!(m.error, Some(true));
    }
    #[test]
    fn pages_are_ordered_isolated_and_count_native_items_once() {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch("CREATE TABLE thread_items(thread_id TEXT,item_id TEXT,rollout_ordinal INTEGER,created_at_ms INTEGER,item_json TEXT)").unwrap();
        for (id, n) in [("one", 3), ("two", 8), ("one", 1), ("one", 2)] {
            c.execute(
                "INSERT INTO thread_items VALUES(?1,?2,?3,0,?4)",
                params![
                    id,
                    n.to_string(),
                    n,
                    json!({"type":"agentMessage","text":n.to_string()}).to_string()
                ],
            )
            .unwrap();
        }
        let (messages, total, start) = page_from(&c, "one", None, 2).unwrap();
        assert_eq!((total, start), (3, 1));
        assert_eq!(
            messages.iter().map(|m| m.text.as_str()).collect::<Vec<_>>(),
            vec!["2", "3"]
        );
        c.execute_batch("ROLLBACK").unwrap();
        let (messages, total, start) = page_from(&c, "one", Some(1), 2).unwrap();
        assert_eq!((total, start, messages.len()), (3, 0, 1));
        assert_eq!(messages[0].text, "1");
    }
    #[test]
    fn bundled_tool_output_is_not_counted_twice() {
        let m = item(
            &json!({"type":"commandExecution","command":"echo hello","aggregatedOutput":"hello","exitCode":0}),
            0,
        );
        assert_eq!(m.result.unwrap().text, "hello");
        assert!(real_file("codex-db:C:\\outside\\state_5.sqlite|id").is_err());
    }
    #[test]
    #[ignore = "read-only diagnostic of native history on this PC"]
    fn real_native_history_reads_pages() {
        let chats = scan();
        assert!(!chats.is_empty());
        let chat = chats.iter().max_by_key(|c| c.updated).unwrap();
        let now = std::time::Instant::now();
        let (messages, total, start) = page(&chat.path, None, 40).unwrap();
        let (version, ms) = stamp(&chat.path).unwrap();
        assert!(version > 0);
        assert!(ms > 0);
        assert!(!messages.is_empty());
        assert_eq!(messages.len(), total - start);
        println!(
            "Native database chats: {}; latest chat page: {} of {} entries in {:?}",
            chats.len(),
            messages.len(),
            total,
            now.elapsed()
        );
    }
}
