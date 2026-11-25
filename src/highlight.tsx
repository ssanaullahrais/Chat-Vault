import { cn } from "@/lib/utils";

/** Lower-cases and flattens quotes, dashes and spacing, so pasted text still matches. */
export function norm(s: string) {
  return s
    .replace(/[‘’‛]/g, "'")
    .replace(/[“”]/g, '"')
    .replace(/[–—]/g, "-")
    .replace(/\s+/g, " ")
    .trim()
    .toLowerCase();
}

/** One part of a search: a word, a word with dots or dashes inside, or a "quoted phrase". */
type Unit = { tokens: string[]; exact: boolean };
type Parsed = { include: Unit[]; exclude: Unit[]; any: boolean };

const wordsOf = (s: string) => s.toLowerCase().split(/[^\p{L}\p{N}]+/u).filter(Boolean);

/**
 * Reads a search the same way the app's search engine does (search.rs): words must all appear, "quoted phrases" must
 * appear as written, -word leaves chats out, and OR between words means any of them.
 */
export function parseQuery(q: string): Parsed {
  const out: Parsed = { include: [], exclude: [], any: false };
  const re = /(-?)(?:"([^"]*)"?|(\S+))/g;
  const text = q.replace(/[“”„]/g, '"');
  for (let m = re.exec(text); m; m = re.exec(text)) {
    const negate = m[1] === "-" && (m[2] !== undefined || !!m[3]);
    const exact = m[2] !== undefined;
    const raw = exact ? m[2] : m[3] ?? "";
    if (!negate && !exact && raw === "OR") {
      out.any = true;
      continue;
    }
    const tokens = wordsOf(raw);
    if (!tokens.length) continue;
    (negate ? out.exclude : out.include).push({ tokens, exact });
  }
  return out;
}

const flat = (s: string) => ` ${wordsOf(s).join(" ")} `;
const unitIn = (flatText: string, u: Unit) => {
  const needle = u.tokens.join(" ");
  return u.exact ? flatText.includes(` ${needle} `) : flatText.includes(needle);
};

/** Does this text match the search (every word, or any word for OR, and none of the left-out ones)? */
export function textHas(text: string, q: string) {
  if (!q) return false;
  const p = parseQuery(q);
  if (!p.include.length) return false;
  // The whole search as typed still counts, for searches made only of symbols such as "=>" or "C++".
  if (norm(text).includes(norm(q)) && !q.includes('"') && !/(^|\s)-\S/.test(q)) return true;
  const f = flat(text);
  const ok = p.any ? p.include.some((u) => unitIn(f, u)) : p.include.every((u) => unitIn(f, u));
  return ok && !p.exclude.some((u) => unitIn(f, u));
}

function escapeRe(s: string) {
  return s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

/** A pattern that finds one part of the search in the original text, whatever punctuation or spacing sits between its words. */
function unitPattern(u: Unit) {
  return u.tokens.map(escapeRe).join("[^\\p{L}\\p{N}]+");
}

/** A pattern that finds the whole search as typed, whatever quote style or spacing it uses. */
function phrasePattern(q: string) {
  return norm(q)
    .split("")
    .map((c) => {
      if (c === "'") return "['‘’‛]";
      if (c === '"') return '["“”]';
      if (c === "-") return "[-–—]";
      if (c === " ") return "\\s+";
      return escapeRe(c);
    })
    .join("");
}

/** Every piece of text to mark for a search: the whole search first, then each of its words and phrases. */
export function highlightPattern(q: string): RegExp | null {
  const p = parseQuery(q);
  const parts = p.include.map(unitPattern).sort((a, b) => b.length - a.length);
  const simple = !q.includes('"') && !/(^|\s)-\S/.test(q) && !/\sOR\s/.test(q);
  if (simple && norm(q)) parts.unshift(phrasePattern(q));
  if (!parts.length) return null;
  try {
    return new RegExp(`(${parts.join("|")})`, "giu");
  } catch {
    return null;
  }
}

/** Wraps every match of a search in a highlight: the whole search where it appears, and each of its words elsewhere. */
export function Highlight({ text, q, activeMark }: { text: string; q: string; activeMark: boolean }) {
  const re = q ? highlightPattern(q) : null;
  if (!re) return <>{text}</>;
  const parts = text.split(re);
  return (
    <>
      {parts.map((p, i) =>
        i % 2 === 0 ? (
          <span key={i}>{p}</span>
        ) : (
          <mark
            key={i}
            className={cn(
              // No side padding: a match is often the start of a longer word ("Invoice" in "Invoices"), and padding would split the word.
              "rounded-[3px] text-foreground [box-decoration-break:clone]",
              activeMark && i === 1 ? "bg-amber-300 dark:bg-amber-400/50" : "bg-yellow-200/70 dark:bg-yellow-400/25",
            )}
          >
            {p}
          </mark>
        ),
      )}
    </>
  );
}
