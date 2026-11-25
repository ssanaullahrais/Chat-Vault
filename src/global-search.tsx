import { useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Clock, FolderOpen, Loader2, Search, SearchX, X } from "lucide-react";
import { Input } from "@/components/ui/input";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { FadedScroll } from "@/faded-scroll";
import { ProviderLogo } from "@/claude-logo";
import { Highlight } from "@/highlight";
import { PROVIDERS, PROVIDER_NAME, PROVIDER_SHORT, fmtDate, type Chat, type Found, type Provider } from "@/chat-actions";
import { cn } from "@/lib/utils";

type Hit = { chat: Chat; snippet: string };
const none = (): Record<Provider, Hit[]> => ({ claude: [], codex: [], cursor: [], gemini: [], opencode: [], cline: [] });

const RECENT_KEY = "chatvault.recentSearches";
const loadRecent = (): string[] => {
  try {
    const v = JSON.parse(localStorage.getItem(RECENT_KEY) ?? "[]");
    return Array.isArray(v) ? v.filter((x) => typeof x === "string").slice(0, 8) : [];
  } catch {
    return [];
  }
};

/** Search the words inside every chat of every tool. Results from each tool appear as soon as that tool has been searched. */
export function GlobalSearch({
  onOpenChat,
  onClose,
  titleOf,
  renamed,
  initial = "",
}: {
  onOpenChat: (c: Chat, term: string) => void;
  onClose: () => void;
  titleOf: (c: Chat) => string;
  /** Names given to chats in ChatVault ("provider:id" → name), searched as titles. */
  renamed: Record<string, string>;
  initial?: string;
}) {
  const renamedRef = useRef(renamed);
  renamedRef.current = renamed;
  const [q, setQ] = useState(initial);
  const [hits, setHits] = useState<Record<Provider, Hit[]>>(none());
  const [busy, setBusy] = useState<Provider[]>([]);
  const [searched, setSearched] = useState("");
  const [only, setOnly] = useState<Provider | "all">("all");
  const [recent, setRecent] = useState<string[]>(loadRecent);
  const run = useRef(0);
  const input = useRef<HTMLInputElement | null>(null);

  useEffect(() => input.current?.focus(), []);

  const remember = (term: string) => {
    const next = [term, ...recent.filter((r) => r.toLowerCase() !== term.toLowerCase())].slice(0, 8);
    setRecent(next);
    try {
      localStorage.setItem(RECENT_KEY, JSON.stringify(next));
    } catch {
      /* storage can be blocked */
    }
  };

  useEffect(() => {
    const term = q.trim();
    if (term.length < 2) {
      setHits(none());
      setBusy([]);
      setSearched("");
      return;
    }
    const id = ++run.current;
    const t = window.setTimeout(async () => {
      setBusy([...PROVIDERS]);
      setSearched(term);
      try {
        // One ranked search over every tool; best matches first within each tool.
        const found = await invoke<Found[]>("search_chats", { query: term, renamed: renamedRef.current, includeArchived: true, limit: 500 });
        if (run.current !== id) return;
        const next = none();
        found.forEach((f) => next[f.chat.provider]?.push({ chat: f.chat, snippet: f.snippet }));
        setHits(next);
      } catch {
        if (run.current === id) setHits(none());
      } finally {
        if (run.current === id) setBusy([]);
      }
    }, 250);
    return () => window.clearTimeout(t);
  }, [q]);

  const counts = useMemo(() => PROVIDERS.map((p) => [p, hits[p].length] as const), [hits]);
  const total = counts.reduce((n, [, c]) => n + c, 0);
  const typed = q.trim().length >= 2;
  const searching = busy.length > 0;
  const visible = PROVIDERS.filter((p) => (only === "all" || only === p) && (hits[p].length > 0 || busy.includes(p)));
  // Tools that were searched and found nothing stay out of the filter row, so it only offers choices that lead somewhere.
  const chips = counts.filter(([p, c]) => c > 0 || (busy.includes(p) && only === p));

  const open = (c: Chat) => {
    if (searched) remember(searched);
    onOpenChat(c, searched);
  };

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div>
        <div className="mx-auto w-full max-w-4xl space-y-4 px-6 pb-4 pt-8">
          <div className="flex items-start justify-between gap-4">
            <div>
              <h2 className="text-lg font-semibold tracking-tight">Search all chats</h2>
              <p className="text-sm text-muted-foreground">Find any word inside every chat, from every tool. Use "quotes" for an exact phrase, -word to leave a word out, and OR for either word.</p>
            </div>
            <Button variant="ghost" size="icon" aria-label="Close search" className="-mr-2 -mt-1 text-muted-foreground" onClick={onClose}>
              <X className="size-4" />
            </Button>
          </div>

          <div className="relative">
            <Search className="pointer-events-none absolute left-3.5 top-1/2 size-4 -translate-y-1/2 text-muted-foreground" />
            <Input
              ref={input}
              value={q}
              onChange={(e) => setQ(e.target.value)}
              onKeyDown={(e) => e.key === "Escape" && (q ? setQ("") : onClose())}
              placeholder="Type a word or a phrase"
              className="h-11 rounded-xl pl-10 pr-10 text-base"
            />
            <div className="absolute right-3 top-1/2 -translate-y-1/2">
              {searching ? (
                <Loader2 className="size-4 animate-spin text-muted-foreground" />
              ) : q ? (
                <button aria-label="Clear" className="cursor-pointer rounded-sm text-muted-foreground transition-colors hover:text-foreground" onClick={() => setQ("")}>
                  <X className="size-4" />
                </button>
              ) : null}
            </div>
          </div>

          <div className="flex min-h-7 flex-wrap items-center gap-1.5">
            {typed && chips.length > 0 ? (
              <>
                <FilterChip active={only === "all"} onClick={() => setOnly("all")}>
                  All <span className="tabular-nums opacity-70">{total}</span>
                </FilterChip>
                {chips.map(([p, c]) => (
                  <FilterChip key={p} active={only === p} onClick={() => setOnly(p)}>
                    <ProviderLogo provider={p} className="size-3.5" />
                    {PROVIDER_SHORT[p]} <span className="tabular-nums opacity-70">{c}</span>
                  </FilterChip>
                ))}
              </>
            ) : (
              <p className="text-xs text-muted-foreground">
                {!typed ? "Type at least 2 letters." : searching ? `Searching ${busy.map((p) => PROVIDER_SHORT[p]).join(", ")}…` : "No matches."}
              </p>
            )}
            {typed && chips.length > 0 && (
              <span className="ml-auto text-xs text-muted-foreground">
                {searching ? `Searching ${busy.length} more…` : `${total.toLocaleString()} ${total === 1 ? "chat" : "chats"} found`}
              </span>
            )}
          </div>
        </div>
      </div>

      <FadedScroll className="flex-1">
        <div className="mx-auto w-full max-w-4xl space-y-6 px-6 pb-10 pt-5">
          {!typed && (
            <div className="space-y-6 pt-4">
              {recent.length > 0 && (
                <section>
                  <div className="mb-2 flex items-center justify-between">
                    <h3 className="flex items-center gap-1.5 text-xs font-medium uppercase tracking-wide text-muted-foreground">
                      <Clock className="size-3.5" /> Recent searches
                    </h3>
                    <button
                      className="cursor-pointer text-xs text-muted-foreground transition-colors hover:text-foreground"
                      onClick={() => {
                        setRecent([]);
                        try {
                          localStorage.removeItem(RECENT_KEY);
                        } catch {
                          /* storage can be blocked */
                        }
                      }}
                    >
                      Clear
                    </button>
                  </div>
                  <div className="flex flex-wrap gap-2">
                    {recent.map((r) => (
                      <button key={r} className="cursor-pointer rounded-full border bg-card px-3 py-1 text-sm transition-colors hover:bg-muted" onClick={() => setQ(r)}>
                        {r}
                      </button>
                    ))}
                  </div>
                </section>
              )}
              <div className="flex flex-col items-center gap-3 rounded-xl border border-dashed px-6 py-10 text-center">
                <div className="flex -space-x-1.5">
                  {PROVIDERS.map((p) => (
                    <span key={p} title={PROVIDER_NAME[p]} className="flex size-8 items-center justify-center rounded-full bg-muted p-1.5 ring-2 ring-background">
                      <ProviderLogo provider={p} className="size-full" />
                    </span>
                  ))}
                </div>
                <p className="text-sm font-medium">Search the words inside every chat</p>
                <p className="max-w-md text-xs text-muted-foreground">Claude Code, Codex, Cursor, Gemini CLI, OpenCode and Cline are searched together, best matches first.</p>
              </div>
            </div>
          )}

          {typed && searching && total === 0 && (
            <div className="space-y-2">
              {Array.from({ length: 5 }).map((_, i) => (
                <div key={i} className="flex gap-3 rounded-xl border p-3">
                  <Skeleton className="size-8 shrink-0 rounded-lg" />
                  <div className="min-w-0 flex-1 space-y-2">
                    <Skeleton className="h-4 w-1/2" />
                    <Skeleton className="h-3 w-full" />
                    <Skeleton className="h-3 w-3/4" />
                  </div>
                </div>
              ))}
            </div>
          )}

          {typed && visible.map((p) => (
            <section key={p}>
              <h3 className="mb-2 flex items-center gap-2 text-xs font-medium uppercase tracking-wide text-muted-foreground">
                <ProviderLogo provider={p} className="size-4" /> {PROVIDER_NAME[p]}
                {busy.includes(p) ? <Loader2 className="size-3 animate-spin" /> : <span className="rounded-full bg-muted px-1.5 py-0.5 text-[10px] tabular-nums normal-case tracking-normal">{hits[p].length}</span>}
              </h3>
              <div className="space-y-2">
                {hits[p].map((h) => (
                  <button
                    key={h.chat.id}
                    onClick={() => open(h.chat)}
                    className="group flex w-full cursor-pointer items-start gap-3 rounded-xl border bg-card p-3 text-left transition-colors hover:border-foreground/20 hover:bg-muted/40"
                  >
                    <span className="mt-0.5 flex size-8 shrink-0 items-center justify-center rounded-lg bg-muted">
                      <ProviderLogo provider={h.chat.provider} className="size-5" />
                    </span>
                    <span className="min-w-0 flex-1">
                      <span className="flex items-baseline gap-3">
                        <span className="min-w-0 flex-1 truncate text-[13px] font-medium">
                          <Highlight text={titleOf(h.chat)} q={searched} activeMark={false} />
                        </span>
                        <span className="shrink-0 text-xs text-muted-foreground">{fmtDate(h.chat.lastMs)}</span>
                      </span>
                      <span className="mt-0.5 flex items-center gap-1 text-xs text-muted-foreground">
                        <FolderOpen className="size-3 shrink-0" strokeWidth={1.5} />
                        <span className="truncate">{h.chat.project}</span>
                      </span>
                      {h.snippet ? (
                        <span className="mt-1.5 line-clamp-2 block text-[13px] leading-relaxed text-muted-foreground">
                          <Highlight text={h.snippet} q={searched} activeMark={false} />
                        </span>
                      ) : (
                        <span className="mt-1.5 block text-xs text-muted-foreground">Found in the title.</span>
                      )}
                    </span>
                  </button>
                ))}
              </div>
            </section>
          ))}

          {typed && !searching && total === 0 && (
            <div className="flex flex-col items-center gap-2 py-16 text-center">
              <span className="flex size-12 items-center justify-center rounded-full bg-muted">
                <SearchX className="size-5 text-muted-foreground" strokeWidth={1.5} />
              </span>
              <p className="text-sm font-medium">No chat contains “{searched}”</p>
              <p className="max-w-sm text-xs text-muted-foreground">Try a different word, a shorter phrase, or check the spelling. Chats that were added a moment ago may still be getting ready for search.</p>
            </div>
          )}
        </div>
      </FadedScroll>
    </div>
  );
}

function FilterChip({ active, onClick, children }: { active: boolean; onClick: () => void; children: React.ReactNode }) {
  return (
    <button
      onClick={onClick}
      className={cn(
        "flex cursor-pointer items-center gap-1.5 rounded-full border px-2.5 py-1 text-xs font-medium transition-colors",
        active ? "border-foreground/20 bg-foreground text-background" : "bg-card text-muted-foreground hover:bg-muted hover:text-foreground",
      )}
    >
      {children}
    </button>
  );
}
