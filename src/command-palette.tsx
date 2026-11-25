import { useEffect, useRef, useState, type ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Command, CommandDialog, CommandEmpty, CommandGroup, CommandInput, CommandItem, CommandList, CommandSeparator, CommandShortcut } from "@/components/ui/command";
import { ProviderLogo } from "@/claude-logo";
import { Skeleton } from "@/components/ui/skeleton";
import { ScrollArea } from "@/components/ui/scroll-area";
import { PROVIDERS, PROVIDER_SHORT, type Chat, type Found, type Provider } from "@/chat-actions";

export type PaletteAction = { id: string; label: string; icon: ReactNode; shortcut?: string; run: () => void };

/** Ctrl+K: jump to any chat of either tool, or run an app action. */
export function CommandPalette({
  open,
  onOpenChange,
  provider,
  pinned,
  titleOf,
  onOpenChat,
  renamed,
  actions,
}: {
  open: boolean;
  onOpenChange: (o: boolean) => void;
  provider: Provider;
  pinned: Chat[];
  titleOf: (c: Chat) => string;
  onOpenChat: (c: Chat, term: string) => void;
  /** Names given to chats in ChatVault, searched as titles. */
  renamed: Record<string, string>;
  actions: PaletteAction[];
}) {
  const [value, setValue] = useState("");
  const [results, setResults] = useState<Chat[]>([]);
  const [loading, setLoading] = useState(false);
  const [snippets, setSnippets] = useState<Record<string, string>>({});
  const renamedRef = useRef(renamed);
  renamedRef.current = renamed;

  useEffect(() => {
    if (!open) {
      setValue("");
      return;
    }
    let stale = false;
    setLoading(true);
    const q = value.trim();
    const t = window.setTimeout(async () => {
      try {
        if (q.length >= 2) {
          // Same ranked search as the sidebar and the search page.
          const found = await invoke<Found[]>("search_chats", { query: q, renamed: renamedRef.current, includeArchived: false, limit: 12 });
          if (stale) return;
          const snips: Record<string, string> = {};
          found.forEach((f) => f.snippet && (snips[`${f.chat.provider}:${f.chat.id}`] = f.snippet));
          setResults(found.map((f) => f.chat));
          setSnippets(snips);
          return;
        }
        // Nothing (or one letter) typed: the newest chats, and titles for a single letter.
        const per = await Promise.all(
          PROVIDERS.map((p) =>
            invoke<{ items: Chat[] }>("query_chats", { provider: p, account: "all", q, offset: 0, limit: 8, sort: "recent", scope: "all" }).then((r) => r.items).catch(() => [] as Chat[]),
          ),
        );
        if (stale) return;
        setResults(per.flat().sort((x, y) => y.lastMs - x.lastMs).slice(0, 12));
        setSnippets({});
      } catch {
        if (!stale) setResults([]);
      } finally {
        if (!stale) setLoading(false);
      }
    }, 150);
    return () => {
      stale = true;
      window.clearTimeout(t);
    };
  }, [value, open]);

  const select = (fn: () => void) => {
    onOpenChange(false);
    fn();
  };
  const q = value.trim().toLowerCase();
  const shownActions = actions.filter((a) => !q || a.label.toLowerCase().includes(q));
  const shownPinned = pinned.filter((c) => !q || titleOf(c).toLowerCase().includes(q));

  const item = (c: Chat) => (
    <CommandItem key={`${c.provider}:${c.id}`} value={`${c.provider}:${c.id}`} onSelect={() => select(() => onOpenChat(c, value.trim().length >= 2 ? value.trim() : ""))}>
      <ProviderLogo provider={c.provider} className="size-4 shrink-0" />
      <span className="min-w-0 flex-1">
        <span className="block truncate">{titleOf(c)}</span>
        {q && snippets[`${c.provider}:${c.id}`] && <span className="block truncate text-xs text-muted-foreground">{snippets[`${c.provider}:${c.id}`]}</span>}
      </span>
    </CommandItem>
  );

  return (
    <CommandDialog open={open} onOpenChange={onOpenChange} title="Search ChatVault" description="Find a chat or run an action." className="top-1/2 -translate-y-1/2 sm:max-w-xl">
      <Command shouldFilter={false}>
        <CommandInput value={value} onValueChange={setValue} placeholder="Search chats and actions…" />
        <ScrollArea className="h-[min(420px,55dvh)]">
        <CommandList className="max-h-none overflow-visible">
          {loading && results.length === 0 && (
            <div className="px-1 py-1" aria-busy="true">
              <Skeleton className="mx-2.5 mb-1 mt-2 h-3 w-20" />
              {Array.from({ length: 5 }).map((_, i) => (
                <div key={i} className="flex items-center gap-2.5 px-2.5 py-2">
                  <Skeleton className="size-4 shrink-0 rounded" />
                  <Skeleton className={`h-3.5 ${["w-3/5", "w-2/3", "w-1/2", "w-3/4", "w-2/5"][i]}`} />
                </div>
              ))}
            </div>
          )}
          {!loading && results.length === 0 && shownActions.length === 0 && shownPinned.length === 0 && <CommandEmpty>No results found.</CommandEmpty>}
          {shownPinned.length > 0 && (
            <CommandGroup heading={`Pinned · ${PROVIDER_SHORT[provider]}`}>{shownPinned.map(item)}</CommandGroup>
          )}
          {results.length > 0 && (
            <>
              {shownPinned.length > 0 && <CommandSeparator />}
              <CommandGroup heading={q ? "Chats" : "Recent chats"}>{results.filter((c) => !shownPinned.some((p) => p.id === c.id && p.provider === c.provider)).slice(0, q ? 12 : 5).map(item)}</CommandGroup>
            </>
          )}
          {shownActions.length > 0 && (
            <>
              <CommandSeparator />
              <CommandGroup heading="Actions">
                {shownActions.map((a) => (
                  <CommandItem key={a.id} value={a.id} onSelect={() => select(a.run)}>
                    {a.icon}
                    <span className="flex-1">{a.label}</span>
                    {a.shortcut && <CommandShortcut>{a.shortcut}</CommandShortcut>}
                  </CommandItem>
                ))}
              </CommandGroup>
            </>
          )}
        </CommandList>
        </ScrollArea>
        <div className="flex items-center gap-3 border-t px-3 py-2 text-xs text-muted-foreground">
          <span><kbd className="rounded border bg-muted px-1">↑</kbd> <kbd className="rounded border bg-muted px-1">↓</kbd> to move</span>
          <span><kbd className="rounded border bg-muted px-1">Enter</kbd> to open</span>
          <span className="ml-auto"><kbd className="rounded border bg-muted px-1">Esc</kbd> to close</span>
        </div>
      </Command>
    </CommandDialog>
  );
}
