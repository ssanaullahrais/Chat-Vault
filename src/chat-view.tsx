import { FileViewContext, FileViewer, isFilePath } from "@/file-viewer";
import { StickyOpenContext, useStickyOpen } from "@/sticky-open";
import { ChatImage, GalleryContext, Lightbox } from "@/image-viewer";
import { useVirtualizer } from "@tanstack/react-virtual";
import { useCallback, useEffect, useLayoutEffect, useMemo, memo, useRef, useState, type ReactElement, type ReactNode } from "react";
import {
  ExternalLink,
  Save,
  Settings,
  LayoutDashboard,
  RefreshCw,
  ArrowRightLeft,
  ArrowUp,
  FolderOpen,
  Archive,
  ArchiveRestore,
  ArrowDown,
  Check,
  ChevronDown,
  ChevronRight,
  ChevronUp,
  Clock,
  Cpu,
  HardDrive,
  MessageSquare,
  Copy,
  FileText,
  ListTree,
  Loader2,
 
  Pencil,
  Pin,
  PinOff,
  Quote,
  Search,
  Trash2,
  User,
  X,
} from "lucide-react";
import { Avatar, AvatarFallback } from "@/components/ui/avatar";
import { Badge } from "@/components/ui/badge";
import { Bubble, BubbleContent } from "@/components/ui/bubble";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Message, MessageAvatar, MessageContent, MessageHeader } from "@/components/ui/message";
import { ScrollArea } from "@/components/ui/scroll-area";
import { openUrl } from "@tauri-apps/plugin-opener";
import { copyImage, downloadImage } from "@/image-viewer";
import { ContextArea, type Entry } from "@/context-area";
import { Toggle } from "@/components/ui/toggle";
import { ActionIcon } from "@/action-icon";
import { runAction } from "@/toast";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { Trace, TraceCollapsible, TraceCollapsibleContent, TraceCollapsibleTrigger, TraceDescription, TraceIcon, TraceLine, TraceTitle } from "@/components/ui/trace";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { cn } from "@/lib/utils";
import { ProviderLogo } from "@/claude-logo";
import { MessageBody } from "@/message-body";
import { Highlight, textHas } from "@/highlight";
import { PROVIDER_SHORT, chatActions, continueTargets, COPY_CONTENT_IDS, deleteKind, fmtDate, fmtSize, prettyTitle, type Chat, type ChatAction, type Msg, type MsgPage, type Provider } from "@/chat-actions";
import { HeaderSkeleton, MessagesSkeleton } from "@/skeletons";
import { PlanBlock, StepItem, SubagentBlock, isAgent, isPlan, mergeResults, stepName, type Item } from "@/steps";

type Props = {
  chat: Chat;
  page: MsgPage | null;
  loadingEarlier: boolean;
  live?: boolean;
  outline?: { ts: string; text: string }[] | null;
  onLoadUntil?: (ts: string) => Promise<void>;
  onLoadEarlier: () => void;
  onLoadAll: () => Promise<void>;
  showSteps: boolean;
  onToggleSteps: () => void;
  headerOpen: boolean;
  onToggleHeader: () => void;
  pinned: boolean;
  onTogglePin: () => void;
  onArchive?: () => void;
  onRename: () => void;
  onDelete: () => void;
  onRefresh?: () => void;
  onOverview?: () => void;
  /** Open the all-chats search with this word already typed. */
  onSearchAll?: (q: string) => void;
  /** Words searched for when this chat was opened from search results; they start the in-chat search. */
  initialSearch?: string;
  toast: (m: string) => void;
};

function Tip({ label, children }: { label: string; children: ReactElement }) {
  return (
    <Tooltip>
      <TooltipTrigger render={children} />
      <TooltipContent className="max-w-xs">{label}</TooltipContent>
    </Tooltip>
  );
}

type Block = { kind: "msg"; i: number; m: Item } | { kind: "plan"; i: number; m: Item } | { kind: "agent"; i: number; m: Item } | { kind: "trace"; i: number; items: { i: number; m: Item }[] };

export function ChatView(p: Props) {
  const { chat, page } = p;
  const [searchOpen, setSearchOpen] = useState(false);
  const [q, setQ] = useState("");
  const [cur, setCur] = useState(0);
  const [showJump, setShowJump] = useState(false);
  const [atTop, setAtTop] = useState(true);
  const [atEnd, setAtEnd] = useState(false);
  const [activeTick, setActiveTick] = useState(0);
  const [loadingAll, setLoadingAll] = useState(false);
  const [viewer, setViewer] = useState<number | null>(null);
  const [fileOpen, setFileOpen] = useState<string | null>(null);
  // Full paths the chat itself mentions. A short path like src/app.tsx is looked up under the folders they point into,
  // so it still opens when the chat worked on a project other than its own folder.
  const fileHints = useMemo(() => {
    if (!fileOpen || !page) return [];
    const seen = new Set<string>();
    for (const m of page.messages) {
      const data = typeof m.data === "string" ? m.data : m.data ? JSON.stringify(m.data) : "";
      // JSON doubles every backslash in a path; undo that before looking for drive paths.
      const text = `${m.text}\n${data}`.split("\\\\").join("\\");
      for (const hit of text.match(/[A-Za-z]:\\[^\s"'`<>|*?]+/g) ?? []) {
        seen.add(hit.replace(/[.,;:)\]]+$/, ""));
        if (seen.size >= 300) return [...seen];
      }
    }
    return [...seen];
  }, [fileOpen, page]);
  const fileCtx = useMemo(() => ({ base: chat.cwd ?? null, open: (path: string) => setFileOpen(path) }), [chat.cwd]);
  useEffect(() => setFileOpen(null), [chat.id]);
  const wrapRef = useRef<HTMLDivElement | null>(null);
  const searchRef = useRef<HTMLInputElement | null>(null);
  const listRef = useRef<HTMLDivElement | null>(null);
  const [margin, setMargin] = useState(0);

  const merged = useMemo(() => (page ? mergeResults(page.messages) : []), [page]);
  const shown = useMemo(() => merged.filter((m) => p.showSteps || (m.kind !== "tool" && m.kind !== "reasoning")), [merged, p.showSteps]);
  const viewport = useCallback(() => wrapRef.current?.querySelector<HTMLElement>("[data-slot=scroll-area-viewport]") ?? null, []);

  const ready = page !== null;
  const userTick = useRef(0);
  const firstRequested = useRef(false);
  const pending = useRef<string | null>(null);

  // Consecutive tool steps collapse into one Trace.
  const blocks = useMemo(() => {
    const out: Block[] = [];
    shown.forEach((m, i) => {
      if (isPlan(m)) out.push({ kind: "plan", i, m });
      else if (isAgent(m)) out.push({ kind: "agent", i, m });
      else if (m.kind === "tool" || m.kind === "reasoning") {
        const last = out[out.length - 1];
        if (last && last.kind === "trace") last.items.push({ i, m });
        else out.push({ kind: "trace", i, items: [{ i, m }] });
      } else out.push({ kind: "msg", i, m });
    });
    return out;
  }, [shown]);

  // Stable keys keep measured heights when earlier messages are added above.
  const keys = useMemo(() => {
    const seen = new Map<string, number>();
    const list = blocks.map((b) => {
      const m = b.kind === "trace" ? b.items[0].m : b.m;
      const base = `${b.kind}|${m.ts ?? ""}|${m.role}|${m.text.slice(0, 48)}`;
      const n = (seen.get(base) ?? 0) + 1;
      seen.set(base, n);
      return `${base}|${n}`;
    });
    return list;
  }, [blocks]);
  const blockOf = useMemo(() => {
    const out: number[] = [];
    blocks.forEach((b, bi) => {
      if (b.kind === "trace") b.items.forEach((t) => (out[t.i] = bi));
      else out[b.i] = bi;
    });
    return out;
  }, [blocks]);

  // A first guess for the height of a row from its text, so rows measured later move the page as little as possible.
  const estimate = (i: number) => {
    const b = blocks[i];
    if (!b) return 90;
    if (b.kind === "trace") return 36;
    if (b.kind !== "msg") return 90;
    const t = b.m.text;
    let lines = 0;
    for (let at = 0; at < t.length; ) {
      const nl = t.indexOf("\n", at);
      const end = nl < 0 ? t.length : nl;
      lines += Math.max(1, Math.ceil((end - at) / 80));
      if (nl < 0) break;
      at = nl + 1;
      if (lines > 60) break;
    }
    return Math.min(1100, 52 + Math.min(lines, 60) * 24);
  };

  const openMemo = useMemo(() => new Map<string, boolean>(), [chat.id]);

  const virt = useVirtualizer({
    count: blocks.length,
    getScrollElement: viewport,
    estimateSize: estimate,
    overscan: 8,
    scrollMargin: margin,
    getItemKey: (i) => keys[i] ?? i,
    // Messages added above keep the row at the top of the screen where it is, in the same render, not a frame later.
    anchorTo: "end",
    followOnAppend: true,
    scrollEndThreshold: 80,
    scrollPaddingStart: 16,
  });
  // The browser owns wheel/touch movement; the virtualizer owns anchoring and navigation.
  // Cancel its pending navigation as soon as the reader takes over.
  useEffect(() => {
    const el = viewport();
    if (!el) return;
    const cancel = () => {
      userTick.current++;
      firstRequested.current = false;
      pending.current = null;
      virt.cancelScroll();
    };
    const events = ["wheel", "touchstart", "keydown", "pointerdown"] as const;
    events.forEach((event) => el.addEventListener(event, cancel, { passive: true }));
    return () => events.forEach((event) => el.removeEventListener(event, cancel));
  }, [viewport, virt]);

  useLayoutEffect(() => {
    if (ready) virt.scrollToEnd();
  }, [ready, chat.id, virt]);

  // Loading indicators above the list can change without page.start changing.
  useLayoutEffect(() => {
    const list = listRef.current;
    if (!list) return;
    const update = () => setMargin(list.offsetTop);
    update();
    const observer = new ResizeObserver(update);
    if (list.parentElement) observer.observe(list.parentElement);
    return () => observer.disconnect();
  }, [ready, page?.start, page?.partial]);

  useLayoutEffect(() => {
    if (firstRequested.current && page?.start === 0 && !page.partial) {
      firstRequested.current = false;
      virt.scrollToOffset(0);
    }
  }, [page, virt]);

  const goTo = useCallback(
    (shownIdx: number, align: "start" | "center", instant = false) => {
      const bi = blockOf[shownIdx];
      const el = viewport();
      if (bi === undefined || !el) return;
      const target = virt.getOffsetForIndex(bi, align)?.[0] ?? el.scrollTop;
      // Distant navigation should not animate through thousands of unmeasured messages.
      const distant = Math.abs(target - el.scrollTop) > el.clientHeight * 2.5;
      virt.scrollToIndex(bi, { align, behavior: instant || distant ? "auto" : "smooth" });
    },
    [blockOf, viewport, virt],
  );

  const images = useMemo(() => shown.filter((m) => m.kind === "image" && typeof m.data === "string").map((m) => m.data as string), [shown]);
  const gallery = useMemo(() => ({ images, open: (src: string) => setViewer(Math.max(0, images.indexOf(src))) }), [images]);
  useEffect(() => setViewer(null), [chat.id]);

  const query = searchOpen ? q.trim() : "";
  const matches = useMemo(() => {
    if (!query) return [] as number[];
    const out: number[] = [];
    shown.forEach((m, i) => {
      if (m.kind !== "image" && textHas(m.text, query)) out.push(i);
    });
    return out;
  }, [shown, query]);
  const matchIndex = matches.length ? Math.min(cur, matches.length - 1) : -1;
  const lastSearchJump = useRef<string | null>(null);
  useEffect(() => setCur(0), [query, chat.id]);
  useEffect(() => {
    if (matchIndex < 0) {
      lastSearchJump.current = null;
      return;
    }
    const target = `${query}|${matchIndex}|${keys[blockOf[matches[matchIndex]]]}`;
    if (lastSearchJump.current === target) return;
    lastSearchJump.current = target;
    goTo(matches[matchIndex], "center");
  }, [query, matchIndex, matches, keys, blockOf, goTo]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "f") {
        e.preventDefault();
        setSearchOpen(true);
        window.setTimeout(() => searchRef.current?.select(), 0);
      }
      if (e.key === "Escape") setSearchOpen(false);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  // Opened from search results: the in-chat search starts with those words, so every match is highlighted and the first one is shown.
  useEffect(() => {
    const seed = p.initialSearch?.trim() ?? "";
    setSearchOpen(!!seed);
    setQ(seed);
  }, [chat.id, viewport, p.initialSearch]);

  const live = useRef({ start: 0, loading: false, partial: false, loadEarlier: p.onLoadEarlier });
  live.current = { start: page?.start ?? 0, loading: p.loadingEarlier, partial: !!page?.partial, loadEarlier: p.onLoadEarlier };
  const ticks = useMemo(() => shown.map((m, i) => ({ m, i })).filter((t) => t.m.role === "user" && t.m.kind === "text"), [shown]);
  useEffect(() => {
    const el = viewport();
    if (!el) return;
    let raf = 0;
    const onScroll = () => {
      // Near the top of what is loaded, the next earlier messages are read; the view stays where it is.
      if (live.current.start > 0 && !live.current.loading && !live.current.partial && el.scrollTop - margin < 1800) live.current.loadEarlier();
      cancelAnimationFrame(raf);
      raf = requestAnimationFrame(() => {
        setShowJump(el.scrollHeight - el.scrollTop - el.clientHeight > 400);
        setAtTop(el.scrollTop < 8);
        setAtEnd(el.scrollHeight - el.scrollTop - el.clientHeight < 8);
        const line = el.scrollTop + 120;
        // Ticks are in page order, so a binary search over the row positions finds the current one.
        let lo = 0;
        let hi = ticks.length - 1;
        let active = 0;
        while (lo <= hi) {
          const mid = (lo + hi) >> 1;
          const row = virt.measurementsCache[blockOf[ticks[mid].i]];
          if (row && row.start <= line) {
            active = mid;
            lo = mid + 1;
          } else hi = mid - 1;
        }
        setActiveTick(active);
      });
    };
    onScroll();
    el.addEventListener("scroll", onScroll, { passive: true });
    return () => {
      el.removeEventListener("scroll", onScroll);
      cancelAnimationFrame(raf);
    };
  }, [viewport, ticks, page, virt, blockOf, margin]);


  const loadAll = async () => {
    setLoadingAll(true);
    try {
      await p.onLoadAll();
    } finally {
      setLoadingAll(false);
    }
  };

  const needsAll = searchOpen && !!query && !!page && page.start > 0;
  useEffect(() => {
    if (needsAll && !loadingAll) void loadAll();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [needsAll]);

  // The timeline covers the whole chat: ticks come from the light outline, and the ones already loaded jump straight there.
  const barTicks = useMemo(() => {
    const keyOf = (ts: string, text: string) => `${ts}|${text.trim().slice(0, 60)}`;
    const loaded = new Map<string, number>();
    ticks.forEach((t, k) => loaded.set(keyOf(t.m.ts ?? "", t.m.text), k));
    const out: { key: string; i: number; text: string; ts?: string; loaded: boolean; k: number }[] = [];
    const used = new Set<string>();
    (p.outline ?? []).forEach((o) => {
      const key = keyOf(o.ts, o.text);
      const k = loaded.get(key);
      used.add(key);
      out.push({ key, i: k !== undefined ? ticks[k].i : -1, text: o.text, ts: o.ts, loaded: k !== undefined, k: k ?? -1 });
    });
    ticks.forEach((t, k) => {
      const key = keyOf(t.m.ts ?? "", t.m.text);
      if (!used.has(key)) out.push({ key, i: t.i, text: t.m.text, ts: t.m.ts, loaded: true, k });
    });
    return out;
  }, [ticks, p.outline]);
  const step = Math.max(1, Math.ceil(barTicks.length / 60));
  const sampled = barTicks.filter((t, n) => n % step === 0 || (t.loaded && t.k === activeTick));
  /** The first message of the chat: reads every earlier message first when only the latest are loaded. */
  const goToFirst = async () => {
    firstRequested.current = true;
    const tick = userTick.current;
    if ((page?.start ?? 0) > 0) {
      try {
        if (p.onLoadUntil) await p.onLoadUntil("1970-01-01T00:00:00Z");
        else await p.onLoadAll();
      } catch {
        firstRequested.current = false;
      }
    } else if (userTick.current === tick) {
      firstRequested.current = false;
      virt.scrollToOffset(0);
    }
  };
  useEffect(() => {
    const key = pending.current;
    if (!key) return;
    const hit = barTicks.find((t) => t.key === key && t.loaded);
    if (hit) {
      pending.current = null;
      goTo(hit.i, "start", true);
    }
  }, [barTicks, goTo]);

  /**
   * Right-click menu of the chat window. The first block depends on what was clicked (selected words, a link, a picture, a file
   * name, a message); below it come the things people do with a chat: pick it up again, move it, copy it, save it.
   */
  const menuEntries = (target: HTMLElement): Entry[] => {
    // Windows opened above the chat (picture viewer, file viewer) have their own controls.
    if (target.closest('[role="dialog"], [data-lightbox], [data-slot="dialog-content"]')) return [];
    const out: Entry[] = [];
    const say = (text: string, what: string) => async () => {
      await navigator.clipboard.writeText(text);
      p.toast(`${what} copied`);
    };
    const selection = window.getSelection()?.toString().trim() ?? "";
    const link = target.closest("a")?.getAttribute("href");
    const img = target.closest("img");
    const at = target.closest<HTMLElement>("[data-index]");
    const block = at ? blocks[Number(at.dataset.index)] : undefined;
    const said = block && block.kind === "msg" ? (block as { m?: Msg }).m : undefined;
    const codeText = target.closest("code")?.textContent?.trim() ?? "";
    const word = selection && selection.length <= 80 && !selection.includes("\n") ? selection : "";

    // What was clicked
    if (img?.src) {
      out.push(
        { type: "item", label: "Copy image", icon: <Copy strokeWidth={1.5} />, onSelect: () => copyImage(img.src) },
        { type: "item", label: "Save image", icon: <Save strokeWidth={1.5} />, onSelect: () => downloadImage(img.src, 1) },
      );
    }
    if (link) {
      out.push(
        { type: "item", label: "Open link in the browser", icon: <ExternalLink strokeWidth={1.5} />, onSelect: () => void openUrl(link).catch(() => {}) },
        { type: "item", label: "Copy link address", icon: <Copy strokeWidth={1.5} />, onSelect: say(link, "Link") },
      );
    }
    const path = word || codeText;
    if (path && isFilePath(path)) out.push({ type: "item", label: "Open this file", icon: <FileText strokeWidth={1.5} />, onSelect: () => setFileOpen(path) });
    if (selection) out.push({ type: "item", label: "Copy selection", icon: <Copy strokeWidth={1.5} />, onSelect: say(selection, "Selection") });
    if (word && p.onSearchAll) out.push({ type: "item", label: `Search all chats for “${word.length > 24 ? `${word.slice(0, 24)}…` : word}”`, icon: <Search strokeWidth={1.5} />, onSelect: () => p.onSearchAll?.(word) });
    if (said?.text && !selection) out.push({ type: "item", label: said.role === "user" ? "Copy your message" : "Copy this reply", icon: <Copy strokeWidth={1.5} />, onSelect: say(said.text, "Message") });
    if (out.length) out.push({ type: "sep" });

    // The same order as the "…" menu on a sidebar card.
    const acts = chatActions(chat, p.toast, page?.messages ?? null);
    const asEntry = (a: ChatAction, label?: string): Entry => {
      return { type: "item", label: label ?? a.label, icon: <ActionIcon action={a} className="size-4" />, onSelect: () => runAction(a.label, a.run) };
    };
    const short = (a: ChatAction) => a.label.replace(/^Continue in /, "").replace(/^Copy /, "").replace(/ command$/, "");
    const find = (id: string) => acts.find((a) => a.id === id);
    const others = continueTargets(acts);

    // Around the conversation
    out.push(
      { type: "item", label: "Search in this chat", icon: <Search strokeWidth={1.5} />, onSelect: () => setSearchOpen(true) },
      { type: "item", label: p.showSteps ? "Hide the AI's steps" : "Show the AI's steps", icon: <ListTree strokeWidth={1.5} />, onSelect: p.onToggleSteps },
    );
    out.push({
      type: "sub",
      label: "Go to",
      icon: <ArrowDown strokeWidth={1.5} />,
      entries: [
        { type: "item", label: "First message", icon: <ArrowUp strokeWidth={1.5} />, onSelect: () => void goToFirst() },
        { type: "item", label: "Latest message", icon: <ArrowDown strokeWidth={1.5} />, onSelect: () => virt.scrollToEnd() },
        ...(p.onOverview ? ([{ type: "sep" }, { type: "item", label: "Back to Home", icon: <LayoutDashboard strokeWidth={1.5} />, onSelect: p.onOverview }] as Entry[]) : []),
      ],
    });
    out.push({ type: "sep" });

    // Continue it, copy it for an AI, save it
    if (others.length) out.push({ type: "sub", label: "Continue this chat in", icon: <ArrowRightLeft strokeWidth={1.5} />, entries: others.map((a) => asEntry(a, short(a))) });
    for (const id of ["copy-prompt", "copy-path"]) {
      const a = find(id);
      if (a) out.push(asEntry(a));
    }
    const moreCopies = ["copy-md", "copy-id", "fork"].map(find).filter((a): a is ChatAction => !!a);
    const commands = acts.filter((a) => a.group === "Copy" && a.id.startsWith("copy-") && !COPY_CONTENT_IDS.includes(a.id));
    const copyLabel: Record<string, string> = { "copy-md": "Markdown", "copy-id": "Chat ID" };
    out.push({
      type: "sub",
      label: "More copy options",
      icon: <Copy strokeWidth={1.5} />,
      entries: [
        ...moreCopies.map((a) => asEntry(a, copyLabel[a.id])),
        ...(commands.length ? ([{ type: "sep" }, { type: "label", label: "Copy the command to start" }, ...commands.map((a) => asEntry(a, short(a)))] as Entry[]) : []),
      ],
    });
    const saves = acts.filter((a) => a.group === "Save");
    if (saves.length) out.push({ type: "sub", label: "Save as", icon: <Save strokeWidth={1.5} />, entries: saves.map((a) => asEntry(a)) });
    out.push({ type: "sep" });

    // Keep it tidy
    out.push(
      { type: "item", label: "Rename", icon: <Pencil strokeWidth={1.5} />, onSelect: p.onRename },
      { type: "item", label: p.pinned ? "Unpin" : "Pin to top", icon: p.pinned ? <PinOff strokeWidth={1.5} /> : <Pin strokeWidth={1.5} />, onSelect: p.onTogglePin },
    );
    if (p.onArchive) out.push({ type: "item", label: chat.userArchived ? "Unarchive" : "Archive", icon: chat.userArchived ? <ArchiveRestore strokeWidth={1.5} /> : <Archive strokeWidth={1.5} />, onSelect: p.onArchive });
    out.push({ type: "sep" });

    // Elsewhere
    const open = acts.filter((a) => a.group === "Open");
    if (open.length) out.push({ type: "sub", label: "Open", icon: <FolderOpen strokeWidth={1.5} />, entries: open.map((a) => asEntry(a)) });
    if (p.onRefresh) out.push({ type: "item", label: "Refresh", icon: <RefreshCw strokeWidth={1.5} />, onSelect: p.onRefresh });
    out.push({ type: "sep" }, { type: "item", label: "Delete chat", icon: <Trash2 strokeWidth={1.5} />, danger: deleteKind(chat) !== "blocked", muted: deleteKind(chat) === "blocked", onSelect: p.onDelete });
    return out;
  };

  return (
    <GalleryContext.Provider value={gallery}>
    <FileViewContext.Provider value={fileCtx}>
    <StickyOpenContext.Provider value={openMemo}>
    <ContextArea className="flex min-h-0 flex-1 flex-col" entries={menuEntries} onError={(e) => p.toast(String(e))}>
    <div className="flex min-h-0 flex-1 flex-col">
      {viewer !== null && <Lightbox images={images} index={Math.min(viewer, images.length - 1)} onIndex={setViewer} onClose={() => setViewer(null)} />}
      {!page ? (
        <HeaderSkeleton />
      ) : (
        <header className="border-b py-3">
          <div className="w-full space-y-3 px-4">
          <div className="flex items-center gap-3">
            <div className="flex size-10 shrink-0 items-center justify-center rounded-xl bg-muted">
              <ProviderLogo provider={chat.provider} className="size-6" />
            </div>
            <div className="min-w-0 flex-1">
              <div className="group/title flex items-center gap-1">
                <h2 className="truncate text-[13px] font-medium leading-tight">{prettyTitle(chat.title)}</h2>
                {p.live && (
                  <Tip label={`${PROVIDER_SHORT[chat.provider]} is working in this chat right now`}>
                    <span className="relative ml-1 flex size-2 shrink-0">
                      <span className="absolute inline-flex size-full animate-ping rounded-full bg-emerald-400 opacity-75" />
                      <span className="relative inline-flex size-2 rounded-full bg-emerald-500" />
                    </span>
                  </Tip>
                )}
                <Tip label="Rename this chat in ChatVault.">
                  <Button variant="ghost" size="icon" className="size-6 shrink-0 text-muted-foreground opacity-0 group-hover/title:opacity-100 focus-visible:opacity-100" aria-label="Rename" onClick={p.onRename}>
                    <Pencil className="size-3.5" />
                  </Button>
                </Tip>
              </div>
              <div className="mt-1 flex flex-wrap items-center gap-1">
                {[
                  chat.model ? { icon: Cpu, label: chat.model } : null,
                  chat.backup ? { icon: Archive, label: "Backup copy" } : null,
                  { icon: Clock, label: fmtDate(chat.lastMs) },
                  { icon: HardDrive, label: chat.path.startsWith("codex-db:") ? "Database storage" : fmtSize(chat.size) },
                  { icon: MessageSquare, label: page.partial ? "Loading transcript…" : `${page.total.toLocaleString()} transcript entries` },
                ]
                  .filter((m): m is { icon: typeof Cpu; label: string } => m !== null)
                  .map((m) => (
                    <Badge key={m.label} variant="secondary" className="h-5 gap-1 rounded-md px-1.5 text-[11px] font-normal text-muted-foreground">
                      <m.icon className="size-3" strokeWidth={1.5} />
                      {m.label}
                    </Badge>
                  ))}
              </div>
            </div>
            <div className="flex shrink-0 items-center gap-1">
              <Tip label={p.pinned ? "Unpin" : "Pin"}>
                <Toggle aria-label="Pin" size="sm" pressed={p.pinned} onPressedChange={p.onTogglePin}>
                  {p.pinned ? <PinOff /> : <Pin />}
                </Toggle>
              </Tip>
              <Tip label="Search in this chat (Ctrl+F)">
                <Toggle
                  aria-label="Search in chat"
                  size="sm"
                  pressed={searchOpen}
                  onPressedChange={(v) => {
                    setSearchOpen(v);
                    if (v) window.setTimeout(() => searchRef.current?.focus(), 0);
                  }}
                >
                  <Search />
                </Toggle>
              </Tip>
              <Tip label={p.showSteps ? "Hide the steps the AI took" : "Show the steps the AI took"}>
                <Toggle aria-label="Show steps" size="sm" pressed={p.showSteps} onPressedChange={p.onToggleSteps}>
                  <ListTree />
                </Toggle>
              </Tip>
            </div>
            <div className="flex shrink-0 flex-wrap items-center justify-end gap-2">
              <ExportMenu kind="copy" chat={chat} msgs={page.messages} toast={p.toast} />
              <ExportMenu kind="save" chat={chat} msgs={page.messages} toast={p.toast} />
              <ActionsMenu chat={chat} msgs={page.messages} toast={p.toast} onRename={p.onRename} onDelete={p.onDelete} pinned={p.pinned} onTogglePin={p.onTogglePin} onArchive={p.onArchive} />
            </div>
          </div>

          </div>
        </header>
      )}

      <div ref={wrapRef} className="relative min-h-0 flex-1">
        {page && searchOpen && (
          <div className="absolute right-6 top-3 z-20 w-[min(26rem,calc(100%-3rem))] rounded-xl border bg-popover p-1.5 text-popover-foreground shadow-lg">
            <div className="flex items-center gap-1">
              <Search className="ml-2 size-4 shrink-0 text-muted-foreground" />
              <Input
                ref={searchRef}
                className="h-8 border-0 bg-transparent px-2 shadow-none focus-visible:ring-0"
                placeholder="Find in this chat"
                value={q}
                onChange={(e) => setQ(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter" && matches.length) setCur((c) => (e.shiftKey ? (c - 1 + matches.length) % matches.length : (c + 1) % matches.length));
                }}
              />
              <span className="shrink-0 px-1 text-xs tabular-nums text-muted-foreground">{query ? (loadingAll ? "…" : matches.length ? `${matchIndex + 1}/${matches.length}` : "0/0") : ""}</span>
              <Button variant="ghost" size="icon" className="size-7" aria-label="Previous match" disabled={!matches.length} onClick={() => setCur((c) => (c - 1 + matches.length) % matches.length)}>
                <ChevronUp className="size-4" />
              </Button>
              <Button variant="ghost" size="icon" className="size-7" aria-label="Next match" disabled={!matches.length} onClick={() => setCur((c) => (c + 1) % matches.length)}>
                <ChevronDown className="size-4" />
              </Button>
              <Button variant="ghost" size="icon" className="size-7" aria-label="Close search" onClick={() => setSearchOpen(false)}>
                <X className="size-4" />
              </Button>
            </div>
          </div>
        )}

        <ScrollArea className="h-full" viewportClassName="[overflow-anchor:none]">
          {!page ? (
            <MessagesSkeleton />
          ) : (
            <div className="mx-auto max-w-3xl px-4 pb-10 pt-4">
              {page.partial && (
                <div className="flex items-center justify-center gap-2 pb-7 text-xs text-muted-foreground">
                  <Loader2 className="size-3.5 animate-spin" /> Loading…
                </div>
              )}
              {!page.partial && page.start > 0 && (
                <div className="flex justify-center pb-7">
                  <Button size="sm" variant="outline" onClick={p.onLoadEarlier} disabled={p.loadingEarlier}>
                    {p.loadingEarlier ? <Loader2 className="size-4 animate-spin" /> : <ChevronUp className="size-4" />}
                    Load earlier messages ({page.start.toLocaleString()} more)
                  </Button>
                </div>
              )}
              <div ref={listRef} className="relative w-full" style={{ height: virt.getTotalSize() }}>
                {virt.getVirtualItems().map((vi) => {
                  const b = blocks[vi.index];
                  if (!b) return null;
                  return (
                    <div key={vi.key} data-index={vi.index} ref={virt.measureElement} className="absolute left-0 top-0 w-full pb-7" style={{ transform: `translateY(${vi.start - margin}px)` }}>
                      {b.kind === "plan" ? (
                        <PlanBlock item={b.m} memoKey={String(vi.key)} />
                      ) : b.kind === "agent" ? (
                        <SubagentBlock item={b.m} memoKey={String(vi.key)} />
                      ) : b.kind === "trace" ? (
                        <StepsTrace items={b.items} q={query} rowKey={String(vi.key)} />
                      ) : (
                        <MessageRow m={b.m} provider={chat.provider} q={query} isCurrentMatch={matchIndex >= 0 && matches[matchIndex] === b.i} toast={p.toast} rowKey={String(vi.key)} />
                      )}
                    </div>
                  );
                })}
              </div>
              {p.live && (
                <div className="flex items-center gap-3 pt-2" aria-live="polite">
                  <div className="flex size-8 shrink-0 items-center justify-center rounded-full bg-muted">
                    <ProviderLogo provider={chat.provider} className="size-5 animate-pulse" />
                  </div>
                  <div className="flex items-center gap-2 text-sm text-muted-foreground">
                    <span className="animate-pulse">{PROVIDER_SHORT[chat.provider]} is working</span>
                    <span className="flex gap-[3px]">
                      {[0, 200, 400].map((d) => (
                        <span key={d} className="size-[3px] animate-pulse rounded-full bg-muted-foreground/60" style={{ animationDelay: `${d}ms`, animationDuration: "1.4s" }} />
                      ))}
                    </span>
                  </div>
                </div>
              )}
            </div>
          )}
        </ScrollArea>

        {page && sampled.length >= 3 && (
          <ConversationBar
            ticks={sampled.map((t, n) => ({ i: n, text: t.text, ts: t.ts ?? "", active: t.loaded && t.k === activeTick }))}
            onNear={(n) => {
              const t = sampled[n];
              if (t && !t.loaded && p.onLoadUntil && t.ts) void p.onLoadUntil(t.ts);
            }}
            onPick={(n) => {
              const t = sampled[n];
              if (!t) return;
              if (t.loaded) goTo(t.i, "start", true);
              else if (p.onLoadUntil && t.ts) {
                pending.current = t.key;
                void p.onLoadUntil(t.ts);
              }
            }}
          />
        )}

        <div className={cn("pointer-events-none absolute inset-x-0 top-0 z-10 h-12 bg-gradient-to-b from-background via-background/70 to-transparent transition-opacity duration-200", atTop ? "opacity-0" : "opacity-100")} />
        <div className={cn("pointer-events-none absolute inset-x-0 bottom-0 z-10 h-20 bg-gradient-to-t from-background via-background/70 to-transparent transition-opacity duration-200", atEnd ? "opacity-0" : "opacity-100")} />

        {showJump && (
          <Button
            size="icon"
            aria-label="Scroll to the bottom"
            className="absolute bottom-4 left-1/2 z-20 size-9 -translate-x-1/2 rounded-full shadow-md"
            onClick={() => virt.scrollToEnd()}
          >
            <ArrowDown className="size-4" />
          </Button>
        )}
      </div>
    </div>
      <FileViewer path={fileOpen} base={chat.cwd ?? null} hints={fileHints} onClose={() => setFileOpen(null)} />
    </ContextArea>
    </StickyOpenContext.Provider>
    </FileViewContext.Provider>
    </GalleryContext.Provider>
  );
}


/** Copying and saving the conversation itself: as Markdown, with its ID, as a file. Kept apart from the AI hand-off and the chat actions. */
function ExportMenu({ kind, chat, msgs, toast }: { kind: "copy" | "save"; chat: Chat; msgs: Msg[]; toast: (m: string) => void }) {
  const all = chatActions(chat, toast, msgs);
  const isCommand = (a: ChatAction) => a.group === "Copy" && a.id.startsWith("copy-") && !COPY_CONTENT_IDS.includes(a.id);
  const sections = (
    kind === "copy"
      ? [
          { title: "Copy this chat", items: [...all.filter((a) => a.group === "Copy" && !isCommand(a)), ...all.filter((a) => a.id === "fork")] },
          { title: "Copy the command to start", items: all.filter(isCommand) },
        ]
      : [{ title: "Save this chat", items: all.filter((a) => a.group === "Save") }]
  ).filter((x) => x.items.length);
  if (!sections.length) return null;
  return (
    <DropdownMenu>
      <Tooltip>
        <TooltipTrigger render={<DropdownMenuTrigger render={<Button size="icon-sm" variant="outline" aria-label={kind === "copy" ? "Copy" : "Save"} />} />}>
          {kind === "copy" ? <Copy className="size-4" strokeWidth={1.5} /> : <Save className="size-4" strokeWidth={1.5} />}
        </TooltipTrigger>
        <TooltipContent>{kind === "copy" ? "Copy" : "Save"}</TooltipContent>
      </Tooltip>
      <DropdownMenuContent align="end" className="w-56">
        {sections.map((sec, i) => (
          <div key={sec.title}>
            {i > 0 && <DropdownMenuSeparator />}
            <DropdownMenuGroup>
              <DropdownMenuLabel>{sec.title}</DropdownMenuLabel>
              {sec.items.map((a) => {
                const Icon = a.icon;
                return (
                  <DropdownMenuItem key={a.id} onClick={() => void runAction(a.label, a.run)}>
                    <Icon className="size-4" strokeWidth={1.5} /> {sec.title.startsWith("Copy the command") ? a.label.replace(/^Copy /, "").replace(/ command$/, "") : a.label}
                  </DropdownMenuItem>
                );
              })}
            </DropdownMenuGroup>
          </div>
        ))}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

/** One primary button for everything you do with the chat itself: continue it in a tool, keep it tidy, find its files, delete it. */
function ActionsMenu({ chat, msgs, toast, onRename, onDelete, pinned, onTogglePin, onArchive }: { chat: Chat; msgs: Msg[]; toast: (m: string) => void; onRename: () => void; onDelete: () => void; pinned: boolean; onTogglePin: () => void; onArchive?: () => void }) {
  const all = chatActions(chat, toast, msgs);
  const tools = continueTargets(all);
  const open = all.filter((a) => a.group === "Open");
  const row = (a: ChatAction, label?: string) => {
    return (
      <DropdownMenuItem key={a.id} onClick={() => void runAction(a.label, a.run)}>
        <ActionIcon action={a} className="size-4" /> {label ?? a.label}
      </DropdownMenuItem>
    );
  };
  return (
    <DropdownMenu>
      <Tooltip>
        <TooltipTrigger render={<DropdownMenuTrigger render={<Button size="sm" aria-label="Chat actions" />} />}>
          <Settings className="size-4" strokeWidth={1.5} /> Actions <ChevronDown className="size-3.5 opacity-70" />
        </TooltipTrigger>
        <TooltipContent>Actions</TooltipContent>
      </Tooltip>
      <DropdownMenuContent align="end" className="w-60">
        {tools.length > 0 && (
          <>
            <DropdownMenuGroup>
              <DropdownMenuLabel>Continue this chat in</DropdownMenuLabel>
              {tools.map((a) => row(a, a.label.replace(/^Continue in /, "")))}
            </DropdownMenuGroup>
            <DropdownMenuSeparator />
          </>
        )}
        <DropdownMenuGroup>
          <DropdownMenuLabel>Chat</DropdownMenuLabel>
          <DropdownMenuItem onClick={onRename}>
            <Pencil className="size-4" strokeWidth={1.5} /> Rename
          </DropdownMenuItem>
          <DropdownMenuItem onClick={onTogglePin}>
            {pinned ? <PinOff className="size-4" strokeWidth={1.5} /> : <Pin className="size-4" strokeWidth={1.5} />} {pinned ? "Unpin" : "Pin to top"}
          </DropdownMenuItem>
          {onArchive && (
            <DropdownMenuItem onClick={onArchive}>
              {chat.userArchived ? <ArchiveRestore className="size-4" strokeWidth={1.5} /> : <Archive className="size-4" strokeWidth={1.5} />} {chat.userArchived ? "Unarchive" : "Archive"}
            </DropdownMenuItem>
          )}
        </DropdownMenuGroup>
        {open.length > 0 && (
          <>
            <DropdownMenuSeparator />
            <DropdownMenuGroup>
              <DropdownMenuLabel>Open</DropdownMenuLabel>
              {open.map((a) => row(a))}
            </DropdownMenuGroup>
          </>
        )}
        <DropdownMenuSeparator />
        <DropdownMenuItem variant={deleteKind(chat) === "blocked" ? "default" : "destructive"} className={deleteKind(chat) === "blocked" ? "opacity-50" : undefined} onClick={onDelete}>
          <Trash2 className="size-4" strokeWidth={1.5} /> Delete chat
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

/** A run of tool steps, collapsed to one line with the shadcn Trace. */
const StepsTrace = memo(function StepsTrace({ items, q, rowKey }: { items: { i: number; m: Item }[]; q: string; rowKey: string }) {
  const [open, setOpen] = useStickyOpen(rowKey);
  const hasMatch = !!q && items.some((t) => textHas(t.m.text, q));
  const names = Array.from(new Set(items.map((t) => stepName(t.m))));
  return (
    <Trace className="pl-11">
      <TraceCollapsible open={open || hasMatch} onOpenChange={setOpen}>
        <TraceCollapsibleTrigger>
          <TraceLine>
            <TraceIcon>
              <ListTree />
            </TraceIcon>
            <TraceTitle>
              {items.length} {items.length === 1 ? "step" : "steps"}
            </TraceTitle>
            <TraceDescription className="max-w-64 truncate">{names.slice(0, 3).join(", ")}</TraceDescription>
          </TraceLine>
        </TraceCollapsibleTrigger>
        <TraceCollapsibleContent>
          <div className="space-y-1.5">
            {items.map(({ i, m }) => (
              <StepItem key={i} item={m} q={q} memoKey={`${rowKey}|${m.id ?? m.ts ?? i}`} />
            ))}
          </div>
        </TraceCollapsibleContent>
      </TraceCollapsible>
    </Trace>
  );
});

const MessageRow = memo(function MessageRow({ m, provider, q, isCurrentMatch, toast, rowKey }: { m: Msg; provider: Provider; q: string; isCurrentMatch: boolean; toast: (m: string) => void; rowKey: string }) {
  const [open, setOpen] = useStickyOpen(rowKey);
  const [copied, setCopied] = useState<string | null>(null);
  const time = m.ts ? new Date(m.ts).toLocaleTimeString(undefined, { timeStyle: "short" }) : "";
  const isUser = m.role === "user";
  const who = isUser ? "You" : PROVIDER_SHORT[provider];

  if (m.kind === "summary") {
    return (
      <div className="rounded-lg border border-dashed p-3 text-sm">
        <button className="flex items-center gap-1.5 text-muted-foreground" onClick={() => setOpen((o) => !o)}>
          {open || q ? <ChevronDown className="size-4" /> : <ChevronRight className="size-4" />}
          Earlier part was summarized here
        </button>
        {(open || q) && (
          <pre className="mt-2 whitespace-pre-wrap font-sans text-xs text-muted-foreground">
            <Highlight text={m.text} q={q} activeMark={isCurrentMatch} />
          </pre>
        )}
      </div>
    );
  }

  const long = m.text.length > 1800;
  const expanded = open || !!q;
  const text = long && !expanded ? m.text.slice(0, 1800) + "…" : m.text;
  const copy = async (what: string, value: string) => {
    try {
      await navigator.clipboard.writeText(value);
      setCopied(what);
      window.setTimeout(() => setCopied(null), 1400);
    } catch {
      toast("Could not copy");
    }
  };

  return (
    <Message align={isUser ? "end" : "start"}>
      <MessageAvatar className="self-start bg-transparent">
        <Avatar>
          <AvatarFallback className={cn("overflow-hidden", isUser ? "bg-secondary text-secondary-foreground" : provider === "codex" ? "bg-transparent" : "bg-muted")}>
            {isUser ? <User className="size-4" /> : <ProviderLogo provider={provider} className={provider === "codex" ? "size-full" : "size-5"} />}
          </AvatarFallback>
        </Avatar>
      </MessageAvatar>
      <MessageContent>
        <MessageHeader className={cn("gap-2", isUser && "flex-row-reverse")}>
          <span className="text-foreground">{who}</span>
          <span className="font-normal">{time}</span>
          <div className="ml-auto flex items-center gap-0.5 opacity-0 transition-opacity group-hover/message:opacity-100 focus-within:opacity-100">
            <MiniAction label={copied === "text" ? "Copied" : "Copy message"} onClick={() => copy("text", m.text)}>
              {copied === "text" ? <Check className="size-3.5" /> : <Copy className="size-3.5" />}
            </MiniAction>
            <MiniAction label={copied === "md" ? "Copied" : "Copy as Markdown"} onClick={() => copy("md", `**${who}** · ${time}\n\n${m.text}`)}>
              {copied === "md" ? <Check className="size-3.5" /> : <FileText className="size-3.5" />}
            </MiniAction>
            <MiniAction label={copied === "quote" ? "Copied" : "Copy as quote"} onClick={() => copy("quote", m.text.split("\n").map((l) => `> ${l}`).join("\n"))}>
              {copied === "quote" ? <Check className="size-3.5" /> : <Quote className="size-3.5" />}
            </MiniAction>
          </div>
        </MessageHeader>
        {m.kind === "image" && typeof m.data === "string" ? (
          <div className={cn("max-w-full", isUser && "flex justify-end")}>
            <ChatImage src={m.data} />
          </div>
        ) : (
        <Bubble variant={isUser ? "secondary" : "outline"} className="max-w-full">
          <BubbleContent className="overflow-visible">
            {m.kind === "image" ? <span className="text-muted-foreground">[Image]</span> : <MessageBody text={text} q={q} active={isCurrentMatch} assistant={!isUser} />}
            {long && !q && (
              <button className="mt-1 text-xs text-muted-foreground underline" onClick={() => setOpen((o) => !o)}>
                {open ? "Show less" : "Show all"}
              </button>
            )}
          </BubbleContent>
        </Bubble>
        )}
      </MessageContent>
    </Message>
  );
  // The toast callback changes identity on every render; it is deliberately left out so rows are not redrawn while scrolling.
}, (a, b) => a.m === b.m && a.provider === b.provider && a.q === b.q && a.isCurrentMatch === b.isCurrentMatch && a.rowKey === b.rowKey);

function MiniAction({ label, onClick, children }: { label: string; onClick: () => void; children: ReactNode }) {
  return (
    <Tooltip>
      <TooltipTrigger render={<Button variant="ghost" size="icon" className="size-6" aria-label={label} onClick={onClick} />}>{children}</TooltipTrigger>
      <TooltipContent>{label}</TooltipContent>
    </Tooltip>
  );
}


type BarTick = { i: number; text: string; ts: string; active: boolean };

const PITCH = 8; // height of one tick row, in px
const PAD = 12; // top padding of the bar, in px

/**
 * The strip of ticks beside the chat. Ticks swell like a wave around the pointer and the closest one
 * turns dark and previews that question. The pointer position is turned into a tick index with plain
 * arithmetic and animated with transforms, so nothing is measured while you move.
 */
function ConversationBar({ ticks, onPick, onNear }: { ticks: BarTick[]; onPick: (i: number) => void; onNear?: (i: number) => void }) {
  const navRef = useRef<HTMLElement | null>(null);
  const topRef = useRef(0);
  const frame = useRef(0);
  // Fractional tick position under the pointer, or null when it is away.
  const [pos, setPos] = useState<number | null>(null);

  const onMove = (e: React.MouseEvent) => {
    const y = e.clientY;
    cancelAnimationFrame(frame.current);
    frame.current = requestAnimationFrame(() => setPos((y - topRef.current - PAD) / PITCH - 0.5));
  };
  const nearest = pos === null ? -1 : Math.min(ticks.length - 1, Math.max(0, Math.round(pos)));
  const near = ticks[nearest];
  const head = near?.text.trim().split("\n")[0] ?? "";

  useEffect(() => () => cancelAnimationFrame(frame.current), []);
  // Hovering a tick lets the chat quietly fetch what that tick needs, so the click lands without any waiting.
  useEffect(() => {
    if (nearest < 0 || !onNear) return;
    const t = window.setTimeout(() => onNear(nearest), 120);
    return () => window.clearTimeout(t);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [nearest]);

  return (
    <nav
      ref={navRef}
      aria-label="Conversation"
      className="absolute left-0 top-1/2 z-10 flex max-h-[80%] -translate-y-1/2 cursor-pointer flex-col pl-3 pr-5"
      style={{ paddingTop: PAD, paddingBottom: PAD }}
      onMouseEnter={(e) => {
        topRef.current = e.currentTarget.getBoundingClientRect().top;
      }}
      onMouseMove={onMove}
      onMouseLeave={() => {
        cancelAnimationFrame(frame.current);
        setPos(null);
      }}
      onClick={() => near && onPick(near.i)}
    >
      {ticks.map((t, idx) => {
        // A smooth bump centred on the pointer, about two ticks wide.
        const f = pos === null ? 0 : Math.exp(-Math.pow(idx - pos, 2) / 5);
        const hot = pos !== null ? idx === nearest : t.active;
        const scale = ((hot ? 22 : 10) + 22 * f) / 44;
        return (
          <div key={t.i} className="flex items-center" style={{ height: PITCH }}>
            <div
              className={cn("w-11 origin-left rounded-full transition-[transform,opacity,background-color] duration-100 ease-out will-change-transform", hot ? "bg-foreground" : "bg-muted-foreground")}
              style={{ height: hot ? 2.5 : f > 0.5 ? 2 : 1.5, transform: `scaleX(${scale})`, opacity: hot ? 1 : 0.5 + 0.35 * f }}
            />
          </div>
        );
      })}
      {near && pos !== null && (
        <div
          className="pointer-events-none absolute left-full z-20 ml-1 w-72 rounded-xl border bg-popover p-3 text-popover-foreground shadow-md"
          style={{ top: PAD + nearest * PITCH + PITCH / 2, transform: "translateY(-50%)" }}
        >
          <p className="line-clamp-2 text-xs font-medium">{head}</p>
          <p className="mt-1 line-clamp-3 text-xs text-muted-foreground">{near.text.trim().slice(head.length).trim() || (near.ts ? fmtDate(Date.parse(near.ts)) : "")}</p>
        </div>
      )}
    </nav>
  );
}
