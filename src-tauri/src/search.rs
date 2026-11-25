//! One search for every place that looks for chats (sidebar, "Search all chats", Ctrl+K).
//!
//! A query is read the way people type it:
//! - words: every word must appear (in the title, the project, the first message or anywhere inside the chat),
//!   and a word also matches longer words that start with it ("deploy" finds "deployment");
//! - "a quoted phrase": those words next to each other, in that order;
//! - -word: leave out chats that contain it;
//! - a OR b: either one is enough.
//!
//! Results are ranked: a title that is exactly the search, then titles that contain it, then titles with all the words,
//! then chats whose text matches best, and newer chats first when the rest is equal.

use crate::index::{open_db, row_to_summary, ChatSummary, COLS};
use rusqlite::params;
use serde::Serialize;
use std::collections::HashMap;

/// One part of a query: a word, a word with dots or dashes inside ("file.rs" is two tokens next to each other), or a
/// quoted phrase.
#[derive(Debug, Clone, PartialEq)]
pub struct Unit {
    pub tokens: Vec<String>,
    /// Typed in quotes: matched exactly, without word-start matching on the last word.
    pub exact: bool,
}

#[derive(Debug, Default, PartialEq)]
pub struct Parsed {
    pub include: Vec<Unit>,
    pub exclude: Vec<Unit>,
    /// "OR" was typed between words: any one of them is enough.
    pub any: bool,
}

/// Lower-case words made of letters and digits, the same way the search index splits text.
pub fn tokens(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_alphanumeric()).filter(|t| !t.is_empty()).map(|t| t.to_lowercase()).collect()
}

pub fn parse(q: &str) -> Parsed {
    let mut out = Parsed::default();
    let chars: Vec<char> = q
        .chars()
        .map(|c| match c {
            '\u{201C}' | '\u{201D}' | '\u{201E}' => '"',
            _ => c,
        })
        .collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        let negate = c == '-' && i + 1 < chars.len() && !chars[i + 1].is_whitespace();
        let start = if negate { i + 1 } else { i };
        let (text, exact, next) = if chars.get(start) == Some(&'"') {
            let end = chars[start + 1..].iter().position(|&x| x == '"').map(|p| start + 1 + p).unwrap_or(chars.len());
            (chars[start + 1..end].iter().collect::<String>(), true, (end + 1).min(chars.len()))
        } else {
            let end = chars[start..].iter().position(|x| x.is_whitespace()).map(|p| start + p).unwrap_or(chars.len());
            (chars[start..end].iter().collect::<String>(), false, end)
        };
        i = next;
        if !negate && !exact && text == "OR" {
            out.any = true;
            continue;
        }
        let toks = tokens(&text);
        if toks.is_empty() {
            continue;
        }
        let unit = Unit { tokens: toks, exact };
        if negate {
            out.exclude.push(unit);
        } else {
            out.include.push(unit);
        }
    }
    out
}

fn fts_unit(u: &Unit) -> String {
    let phrase = format!("\"{}\"", u.tokens.join(" "));
    if u.exact { phrase } else { format!("{phrase}*") }
}

/// The full-text expression for a query, or None when there is nothing to look for.
pub fn fts_expr(p: &Parsed) -> Option<String> {
    if p.include.is_empty() {
        return None;
    }
    let join = if p.any { " OR " } else { " AND " };
    let pos = p.include.iter().map(fts_unit).collect::<Vec<_>>().join(join);
    if p.exclude.is_empty() {
        Some(pos)
    } else {
        let neg = p.exclude.iter().map(fts_unit).collect::<Vec<_>>().join(" OR ");
        Some(format!("({pos}) NOT ({neg})"))
    }
}

/// Text with every run of non-letters turned into one space, padded with spaces, for word-aware "contains" checks.
fn flat(s: &str) -> String {
    format!(" {} ", tokens(s).join(" "))
}

/// Does already-flattened text contain this part of the query? A plain word may be the start of a longer word or sit
/// inside a joined-up name ("quote" in "FuelQuote"); a quoted phrase must be whole words.
fn unit_in(flat_text: &str, u: &Unit) -> bool {
    let needle = u.tokens.join(" ");
    if u.exact { flat_text.contains(&format!(" {needle} ")) } else { flat_text.contains(&needle) }
}

fn excluded(flat_text: &str, p: &Parsed) -> bool {
    p.exclude.iter().any(|u| unit_in(flat_text, u))
}

/// Every part (or, for OR, any part) of the query is in the text.
fn matches(flat_text: &str, p: &Parsed) -> bool {
    if p.include.is_empty() {
        return false;
    }
    let hit = |u: &Unit| unit_in(flat_text, u);
    let ok = if p.any { p.include.iter().any(hit) } else { p.include.iter().all(hit) };
    ok && !excluded(flat_text, p)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Found {
    pub chat: ChatSummary,
    /// A short piece of the chat around the match, or empty when only the title matched.
    pub snippet: String,
    /// The match is in the title (or the name you gave the chat).
    pub in_title: bool,
}

struct Cand {
    body_rank: Option<usize>,
    meta_rank: Option<usize>,
    snippet: String,
}

fn cand_of(m: &mut HashMap<String, Cand>, k: String) -> &mut Cand {
    m.entry(k).or_insert(Cand { body_rank: None, meta_rank: None, snippet: String::new() })
}

fn like_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")
}

/// Searches every chat. `renamed` holds the names given to chats inside ChatVault ("provider:id" → name), which count
/// as titles. `provider` limits the search to one tool. Archived chats are left out unless `include_archived`.
pub fn search(q: &str, provider: Option<&str>, renamed: &HashMap<String, String>, include_archived: bool, limit: usize) -> Result<Vec<Found>, String> {
    let p = parse(q);
    let Some(expr) = fts_expr(&p) else { return Ok(Vec::new()) };
    let c = open_db()?;
    let prov = provider.filter(|x| *x != "all").unwrap_or("");
    let in_prov = |key: &str| prov.is_empty() || key.starts_with(&format!("{prov}:"));
    let mut cands: HashMap<String, Cand> = HashMap::new();

    // Inside the chats: the full-text index, best matches first. Snippets are made later, only for the chats shown.
    {
        let mut st = c
            .prepare("SELECT key FROM chat_bodies WHERE chat_bodies MATCH ?1 ORDER BY rank LIMIT 400")
            .map_err(|e| e.to_string())?;
        let rows = st.query_map(params![expr], |r| r.get::<_, String>(0)).map_err(|e| e.to_string())?;
        for (n, key) in rows.flatten().filter(|k| in_prov(k)).enumerate() {
            cand_of(&mut cands, key).body_rank = Some(n);
        }
    }
    // Titles, projects and the first message.
    {
        let mut st = c
            .prepare("SELECT key FROM chats_fts WHERE chats_fts MATCH ?1 ORDER BY bm25(chats_fts, 0.0, 10.0, 3.0, 1.0) LIMIT 600")
            .map_err(|e| e.to_string())?;
        let rows = st.query_map(params![expr], |r| r.get::<_, String>(0)).map_err(|e| e.to_string())?;
        for (n, key) in rows.flatten().filter(|k| in_prov(k)).enumerate() {
            let cand = cand_of(&mut cands, key);
            cand.meta_rank = Some(n);
        }
    }
    // Part of a word, which the full-text index cannot see ("Quote" inside "FuelQuote"): titles and projects always,
    // and the text of the chats when the index found only a few.
    let plain = p.include.iter().map(|u| u.tokens.join(" ")).collect::<Vec<_>>();
    let longest = plain.iter().max_by_key(|s| s.chars().count()).cloned().unwrap_or_default();
    if longest.chars().count() >= 3 {
        let like = format!("%{}%", like_escape(&longest));
        let mut st = c
            .prepare("SELECT provider||':'||id FROM chats WHERE (title LIKE ?1 ESCAPE '\\' OR project LIKE ?1 ESCAPE '\\') LIMIT 600")
            .map_err(|e| e.to_string())?;
        let rows = st.query_map(params![like], |r| r.get::<_, String>(0)).map_err(|e| e.to_string())?;
        for key in rows.flatten().filter(|k| in_prov(k)) {
            cand_of(&mut cands, key);
        }
        let body_hits = cands.values().filter(|c| c.body_rank.is_some()).count();
        if body_hits < 15 && !p.any {
            let needle = longest.to_lowercase();
            let mut st = c
                .prepare(
                    "SELECT key, substr(body, max(1, instr(lower(body), ?2) - 36), 240) FROM chat_bodies
                     WHERE body LIKE ?1 ESCAPE '\\' LIMIT 60",
                )
                .map_err(|e| e.to_string())?;
            let rows = st.query_map(params![like, needle], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))).map_err(|e| e.to_string())?;
            for (key, snip) in rows.flatten().filter(|(k, _)| in_prov(k)) {
                let cand = cand_of(&mut cands, key);
                if cand.body_rank.is_none() {
                    cand.body_rank = Some(400);
                    cand.snippet = format!("…{}…", snip.replace(['\n', '\r'], " ").trim());
                }
            }
        }
    }
    // Names you gave chats in ChatVault.
    for (key, name) in renamed {
        if in_prov(key) && matches(&flat(name), &p) {
            cand_of(&mut cands, key.clone());
        }
    }
    if cands.is_empty() {
        return Ok(Vec::new());
    }

    // The chats themselves, in one query.
    let keys: Vec<&String> = cands.keys().collect();
    let json = serde_json::to_string(&keys).map_err(|e| e.to_string())?;
    let mut st = c
        .prepare(&format!("SELECT {COLS} FROM chats WHERE (provider||':'||id) IN (SELECT value FROM json_each(?1))"))
        .map_err(|e| e.to_string())?;
    let chats: Vec<ChatSummary> = st.query_map(params![json], row_to_summary).map_err(|e| e.to_string())?.flatten().collect();

    let full = format!(" {} ", plain.join(" "));
    // Chats whose text has all the words side by side, in the order typed, rank above ones that only have them apart.
    let phrase_keys: std::collections::HashSet<String> = if plain.len() > 1 && !p.any {
        let phrase = format!("\"{}\"", plain.join(" "));
        let mut st = c.prepare("SELECT key FROM chat_bodies WHERE chat_bodies MATCH ?1 LIMIT 1000").map_err(|e| e.to_string())?;
        let keys = st.query_map(params![phrase], |r| r.get::<_, String>(0)).map_err(|e| e.to_string())?.flatten().collect();
        keys
    } else {
        Default::default()
    };
    let now = chrono_now_ms();
    let body_n = cands.values().filter_map(|c| c.body_rank).filter(|r| *r < 400).count().max(1) as f64;
    let meta_n = cands.values().filter_map(|c| c.meta_rank).count().max(1) as f64;
    let mut scored: Vec<(f64, Found)> = Vec::new();
    for chat in chats {
        if chat.user_archived && !include_archived {
            continue;
        }
        let key = format!("{}:{}", chat.provider, chat.id);
        let Some(cand) = cands.get(&key) else { continue };
        let title = renamed.get(&key).cloned().unwrap_or_else(|| chat.title.clone());
        let ft = flat(&title);
        let fp = flat(&chat.project);
        // A chat whose title or project holds a left-out word is dropped even when its text did not show it.
        if excluded(&ft, &p) {
            continue;
        }
        let in_title = matches(&ft, &p);
        let mut score = 0.0;
        if in_title {
            score += if ft.trim() == full.trim() {
                3000.0
            } else if plain.len() > 1 && ft.contains(&full) {
                2200.0
            } else {
                1400.0
            };
        } else {
            let some = p.include.iter().filter(|u| unit_in(&ft, u)).count();
            score += 120.0 * some as f64;
        }
        if matches(&fp, &p) {
            score += 250.0;
        }
        if let Some(r) = cand.meta_rank {
            score += 300.0 * (1.0 - r as f64 / meta_n);
        }
        if let Some(r) = cand.body_rank {
            score += if r >= 400 { 250.0 } else { 300.0 + 600.0 * (1.0 - r as f64 / body_n) };
            if phrase_keys.contains(&key) {
                score += 300.0;
            }
        }
        // Nothing matched at all (a candidate from a part-of-word title search that does not have every word).
        if !in_title && cand.body_rank.is_none() && cand.meta_rank.is_none() && !matches(&fp, &p) {
            continue;
        }
        // Newer chats first among equals.
        let age_days = ((now - chat.last_ms).max(0) as f64) / 86_400_000.0;
        score += 120.0 / (1.0 + age_days / 14.0);
        scored.push((score, Found { snippet: cand.snippet.clone(), in_title, chat }));
    }
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal).then(b.1.chat.last_ms.cmp(&a.1.chat.last_ms)));
    let mut out: Vec<Found> = scored.into_iter().take(limit).map(|(_, f)| f).collect();

    // A piece of text around the match, for the chats being returned only (making one is the slow part of a search).
    let need: Vec<String> = out
        .iter()
        .filter(|f| f.snippet.is_empty())
        .map(|f| format!("{}:{}", f.chat.provider, f.chat.id))
        .filter(|k| cands.get(k).and_then(|c| c.body_rank).is_some_and(|r| r < 400))
        .collect();
    if !need.is_empty() {
        let json = serde_json::to_string(&need).map_err(|e| e.to_string())?;
        // The window starts a little before the whole search as typed, or else before its longest word. FTS5's own
        // snippet() re-reads each whole chat and took about half a second for a page of results; this takes a few ms.
        let phrase = plain.join(" ");
        let mut st = c
            .prepare(
                "SELECT key, substr(body, max(1, COALESCE(NULLIF(instr(lower(body), ?3), 0), instr(lower(body), ?4)) - 36), 240)
                 FROM chat_bodies WHERE chat_bodies MATCH ?1 AND key IN (SELECT value FROM json_each(?2))",
            )
            .map_err(|e| e.to_string())?;
        let snips: HashMap<String, String> = st
            .query_map(params![expr, json, phrase, longest.to_lowercase()], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .map_err(|e| e.to_string())?
            .flatten()
            .collect();
        for f in out.iter_mut().filter(|f| f.snippet.is_empty()) {
            if let Some(s) = snips.get(&format!("{}:{}", f.chat.provider, f.chat.id)) {
                f.snippet = tidy_window(s);
            }
        }
    }
    Ok(out)
}

/// A piece of chat text cut at word boundaries, on one line, with "…" where it was cut.
fn tidy_window(s: &str) -> String {
    let one_line: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut words: Vec<&str> = one_line.split(' ').collect();
    if words.len() > 2 {
        words.remove(0);
        words.pop();
    }
    format!("…{}…", words.join(" "))
}

fn chrono_now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(t: &[&str], exact: bool) -> Unit {
        Unit { tokens: t.iter().map(|s| s.to_string()).collect(), exact }
    }

    #[test]
    fn parses_words_phrases_exclusions_and_or() {
        let p = parse(r#"Fuel "price level" -draft file.rs"#);
        assert_eq!(p.include, vec![u(&["fuel"], false), u(&["price", "level"], true), u(&["file", "rs"], false)]);
        assert_eq!(p.exclude, vec![u(&["draft"], false)]);
        assert!(!p.any);
        assert!(parse("invoice OR bill").any);
        assert_eq!(parse("  -  ").include, vec![]);
    }

    #[test]
    fn builds_fts_expressions() {
        assert_eq!(fts_expr(&parse("fuel quote")).unwrap(), r#""fuel"* AND "quote"*"#);
        assert_eq!(fts_expr(&parse(r#""price level""#)).unwrap(), r#""price level""#);
        assert_eq!(fts_expr(&parse("a.b -c")).unwrap(), r#"("a b"*) NOT ("c"*)"#);
        assert_eq!(fts_expr(&parse("x OR y")).unwrap(), r#""x"* OR "y"*"#);
        assert!(fts_expr(&parse("-only")).is_none());
        // Quotes and other symbols never reach the expression unescaped.
        assert_eq!(fts_expr(&parse(r#"he said "hi"#)).unwrap(), r#""he"* AND "said"* AND "hi""#);
    }

    /// Times a few searches against the real chat index on this PC.
    #[test]
    #[ignore = "reads the real chat index on this PC"]
    fn real_index_search_speed() {
        crate::index::refresh(None).unwrap();
        crate::index::index_bodies(None).unwrap();
        for q in ["fuel quote", "\"price level\"", "invoice -draft", "routing OR webhook", "Quote", "single instance tray", "dialog"] {
            let t = std::time::Instant::now();
            let found = search(q, None, &HashMap::new(), true, 200).unwrap();
            println!("{q:>24}: {:>3} results in {:?}; top: {:?}", found.len(), t.elapsed(), found.iter().take(3).map(|f| f.chat.title.clone()).collect::<Vec<_>>());
        }
    }

    #[test]
    fn matches_titles_word_aware() {
        let t = flat("Rebuild FuelQuote dialog — price-level tab");
        assert!(matches(&t, &parse("fuel dialog")));
        assert!(matches(&t, &parse("quote")), "part of a joined-up word");
        assert!(matches(&t, &parse(r#""price level""#)));
        assert!(!matches(&t, &parse(r#""price lev""#)), "a quoted phrase is whole words");
        assert!(!matches(&t, &parse("fuel -dialog")));
        assert!(matches(&t, &parse("invoice OR dialog")));
        assert!(!matches(&t, &parse("invoice dialog")));
    }
}
