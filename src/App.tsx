import { lazy, Suspense, memo, useCallback, useEffect, useMemo, useRef, useState, type ReactElement, type ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  Archive,
  Tag,
  ArchiveRestore,
  ArrowRightLeft,
  Copy,
  ChevronRight,
  Layers,
  DatabaseBackup,
  Eye,
  FolderOpen,
  Settings as SettingsIcon,
  Trash2,
  ArrowUpDown,
  ChevronsDownUp,
  ChevronsUpDown,
  ListFilter,
  PanelLeft,
  RotateCcw,
  LayoutDashboard,
  Loader2,
  Moon,
  MoreHorizontal,
  Pencil,
  Pin,
  PinOff,
  RefreshCw,
  Save,
  Search,
  Sun,
  Users,
} from "lucide-react";
import { AlertDialog, AlertDialogAction, AlertDialogCancel, AlertDialogContent, AlertDialogDescription, AlertDialogFooter, AlertDialogHeader, AlertDialogTitle } from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Separator } from "@/components/ui/separator";
import { AppToaster, notify, notifyError, runAction } from "@/toast";
import { ConfirmHost } from "@/components/confirm";
import { initBulkExport } from "@/bulk-export";
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";
import type { LucideIcon } from "lucide-react";
import { ContextArea, type Entry } from "@/context-area";
import { AppLogo, ProviderLogo, ProviderStack } from "@/claude-logo";
import { Badge } from "@/components/ui/badge";
import { ActionIcon } from "@/action-icon";
import { ChatCard } from "@/chat-card";
import { CORE_PROVIDERS, PROVIDERS, setOffTools, setCustomAccountNames, originalAccountLabel, type Tool, PROVIDER_NAME, isHandoffOnly, deleteKind, deleteBlockedMessage, accountLabel, chatActions, continueTargets, COPY_CONTENT_IDS, prettyProject, setAccountNames, type AccountInfo, type Chat, type ChatAction, type Found, type Msg, type MsgPage, type Provider } from "@/chat-actions";
import { FadedScroll } from "@/faded-scroll";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { SidebarSkeleton, MessagesSkeleton } from "@/skeletons";
import { Splash } from "@/splash";
import type { DriveStatus } from "@/gdrive-panel";
import { resumeDriveJobs, startDriveJob } from "@/drive-jobs";
import { TitleBar } from "@/titlebar";
import { AUTO_SYNC_KEY } from "@/store";
import { BACKUP_AUTO_ENABLED, invalidateBackups, readBackupEntries } from "@/backup-refresh";
import { chatKey, useAccountNames, useChatMeta, usePersisted } from "@/store";
import { RenameAccountDialog } from "@/rename-account-dialog";
import { startAutoUpdate } from "@/updater";
import { groupChatsByDay } from "@/chat-groups";

// Keep the shell independent of chat rendering, charts and closed dialogs.
const ChatView = lazy(() => import("@/chat-view").then((m) => ({ default: m.ChatView })));
const OverviewPage = lazy(() => import("@/overview").then((m) => ({ default: m.OverviewPage })));
const CommandPalette = lazy(() => import("@/command-palette").then((m) => ({ default: m.CommandPalette })));
const GlobalSearch = lazy(() => import("@/global-search").then((m) => ({ default: m.GlobalSearch })));
const TagsDialog = lazy(() => import("@/tags-dialog").then((m) => ({ default: m.TagsDialog })));
const SyncDialog = lazy(() => import("@/sync-dialog").then((m) => ({ default: m.SyncDialog })));
const SettingsDialog = lazy(() => import("@/settings-dialog").then((m) => ({ default: m.SettingsDialog })));
const MoveBackupDialog = lazy(() => import("@/move-backup-dialog").then((m) => ({ default: m.MoveBackupDialog })));

const PAGE_SIZE = 20;
const MSG_PAGE = 80;

type Overview = { total: number; accounts: { id: string; count: number }[]; latestMs: number | null };
type IndexProg = { phase: string; done: number; total: number };

const isTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

export default function App() {
  const [provider, setProvider] = usePersisted<Tool>("chatvault.provider", "claude");
  const [accounts, setAccounts] = usePersisted<Record<string, string>>("chatvault.accounts", {});
  const [showSteps, setShowSteps] = usePersisted("chatvault.steps", true);
  const [sort, setSort] = usePersisted("chatvault.sort", "recent");
  const [scope, setScope] = usePersisted("chatvault.scope", "all");
  const [paletteOpen, setPaletteOpen] = useState(false);
  const [sidebarOpen, setSidebarOpen] = usePersisted("chatvault.sidebar", true);
  const [headerOpen, setHeaderOpen] = usePersisted("chatvault.headerOpen", true);
  const [pinnedOpen, setPinnedOpen] = usePersisted("chatvault.sec.pinned", true);
  const [todayOpen, setTodayOpen] = usePersisted("chatvault.sec.today", true);
  const [yesterdayOpen, setYesterdayOpen] = usePersisted("chatvault.sec.yesterday", true);
  const [calendarDay, setCalendarDay] = useState(() => new Date().toDateString());
  useEffect(() => {
    const timer = window.setInterval(() => setCalendarDay(new Date().toDateString()), 60_000);
    return () => window.clearInterval(timer);
  }, []);
  const [recentOpen, setRecentOpen] = usePersisted("chatvault.sec.recent", true);
  const [dark, setDark] = useState(() => localStorage.getItem("theme") !== "light");
  const { pins, names, togglePin, rename } = useChatMeta();
  // Read by searches without making them run again on every rename.
  const namesRef = useRef(names);
  namesRef.current = names;
  const { accountNames, renameAccount } = useAccountNames();
  setCustomAccountNames(accountNames);
  const [renamingAccount, setRenamingAccount] = useState<string | null>(null);
  const [accountOpen, setAccountOpen] = useState(false);

  const [overview, setOverview] = useState<Record<Provider, Overview | null>>({ claude: null, codex: null, cursor: null, gemini: null, opencode: null, cline: null });
  const [accountInfo, setAccountInfo] = useState<AccountInfo[]>([]);
  const account = provider === "all" ? "all" : accounts[provider] ?? "all";
  const [query, setQuery] = useState("");
  const [q, setQ] = useState("");
  const [items, setItems] = useState<Chat[]>([]);
  const [pinned, setPinned] = useState<Chat[]>([]);
  const [total, setTotal] = useState(0);
  const [loadingList, setLoadingList] = useState(true);
  const [searchAll, setSearchAll] = useState(false);
  const [searchSeed, setSearchSeed] = useState("");
  /** Opens the search page, optionally with a word already typed. */
  const openSearch = useCallback((q = "") => {
    setSearchSeed(q);
    setSearchAll(true);
  }, []);
  const [tagging, setTagging] = useState<Chat | null>(null);
  const [tagList, setTagList] = useState<[string, number][]>([]);
  // The HTML boot screen covers module loading; hand over to the shell on its first frame.
  const [splash, setSplash] = useState<"on" | "leaving" | "off">("on");
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    if (isTauri) initBulkExport();
  }, []);
  // A failed scan or query used to disappear silently; it now shows as an error toast.
  useEffect(() => {
    if (error) notifyError(error);
  }, [error]);
  const [hits, setHits] = useState<Record<string, string> | null>(null);
  const [searching, setSearching] = useState(false);
  const [reloadKey, setReloadKey] = useState(0);
  // Bumped after a background refresh: pinned chats, counts and search results follow, while the list keeps its place.
  const [liveTick, setLiveTick] = useState(0);
  const [indexing, setIndexing] = useState<IndexProg | null>(null);
  const [selected, setSelected] = useState<Chat | null>(null);
  const [page, setPageState] = useState<MsgPage | null>(null);
  const pageRef = useRef<MsgPage | null>(null);
  // Async readers and live updates see the same page before React's next render.
  const setPage = useCallback((next: MsgPage | null | ((current: MsgPage | null) => MsgPage | null)) => {
    const value = typeof next === "function" ? next(pageRef.current) : next;
    pageRef.current = value;
    setPageState(value);
  }, []);
  const [loadingEarlier, setLoadingEarlier] = useState(false);
  const [syncOpen, setSyncOpen] = useState(false);
  const [syncTab, setSyncTab] = useState<"backups" | "backup" | "drive">("backups");
  const [settingsOpen, setSettingsOpen] = useState(false);
  // Tools switched off in Settings are left out of every list. This only makes the screen redraw when that changes.
  const [, setOffTick] = useState(0);
  const applyOff = useCallback(async () => {
    const list = await invoke<string[]>("disabled_integrations").catch(() => [] as string[]);
    setOffTools(list);
    setOffTick((n) => n + 1);
    return list;
  }, []);
  useEffect(() => {
    void applyOff();
  }, [applyOff]);
  const [movePending, setMovePending] = useState<{ path: string; filesToMove: number; bytesToMove: number; isExistingBackup: boolean } | null>(null);
  const [renaming, setRenaming] = useState<Chat | null>(null);
  const [deleting, setDeleting] = useState<Chat | null>(null);
  const [organize, setOrganize] = usePersisted("chatvault.organize", "list");
  // Expand/collapse-all sets a mode; single clicks are exceptions to it, so projects that load later follow the mode too.
  const [projectsMode, setProjectsMode] = usePersisted<"open" | "closed">("chatvault.projectsMode", "open");
  const [projectExceptions, setProjectExceptions] = usePersisted<string[]>("chatvault.projectExceptions", []);
  // Collapsed projects render no chats; hovering one warms it up so it opens instantly.
  const [warmProjects, setWarmProjects] = useState<Set<string>>(new Set());
  const projectOpen = useCallback((n: string) => (projectsMode === "open") !== projectExceptions.includes(n), [projectsMode, projectExceptions]);

  const say = useCallback((m: string) => void notify(m), []);
  // The browser's own right-click menu (Back, Reload, Inspect) makes no sense in an app. Text fields keep their copy and paste menu.
  useEffect(() => {
    if (import.meta.env.DEV) return;
    const block = (e: MouseEvent) => {
      if (!(e.target as HTMLElement).closest("input, textarea, [contenteditable=true]")) e.preventDefault();
    };
    document.addEventListener("contextmenu", block);
    return () => document.removeEventListener("contextmenu", block);
  }, []);
  // A failed list load is reported as a toast, not as text inside the sidebar.
  useEffect(() => {
    if (error) notifyError(error);
  }, [error]);
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "k") {
        e.preventDefault();
        setPaletteOpen((o) => !o);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);
  const named = useCallback((c: Chat): Chat => (names[chatKey(c)] ? { ...c, title: names[chatKey(c)] } : c), [names]);
  const pinSet = useMemo(() => new Set(pins), [pins]);

  useEffect(() => {
    document.documentElement.classList.toggle("dark", dark);
    localStorage.setItem("theme", dark ? "dark" : "light");
  }, [dark]);
  useEffect(() => {
    const t = window.setTimeout(() => setQ(query.trim()), 250);
    return () => window.clearTimeout(t);
  }, [query]);

  // Scan the disk (only new and changed chats are read), then reload what is on screen.
  const rescan = useCallback(async () => {
    if (!isTauri) return;
    setIndexing({ phase: "Scanning", done: 0, total: 0 });
    try {
      const a = await invoke<AccountInfo[]>("account_info");
      setAccountNames(a);
      setAccountInfo(a);
      await invoke("refresh_index");
    } catch (e) {
      setError(String(e));
    } finally {
      setIndexing(null);
      setReloadKey((k) => k + 1);
      // The words inside chats are indexed quietly in the background so searching is instant.
      void invoke("build_search_index").catch(() => {});
    }
  }, []);

  useEffect(() => {
    if (!isTauri) return;
    const un = listen<IndexProg>("index-progress", (e) => setIndexing(e.payload.phase === "Done" ? null : e.payload));
    let cancelled = false;
    // Drive sync already makes the local backup, so the scheduler runs only one of the two jobs.
    let backingUp = false;
    const backup = async () => {
      if (cancelled || backingUp) return;
      backingUp = true;
      try {
        const drive = await invoke<DriveStatus>("gdrive_status").catch(() => null);
        if (cancelled) return;
        let changed = false;
        let refreshBackups = false;
        if (drive?.connected && drive.autoSync) {
          await startDriveJob({ kind: "sync" }, rescan, true);
          return;
        } else if (localStorage.getItem(AUTO_SYNC_KEY) !== "0") {
          const result = await invoke<{ copied: number }>("sync_chats").catch(() => null);
          changed = !!result?.copied;
          refreshBackups = changed;
        }
        if (cancelled) return;
        if (changed) await rescan();
        if (refreshBackups) invalidateBackups();
      } finally {
        backingUp = false;
      }
    };
    const onAutoEnabled = () => void backup();
    window.addEventListener(BACKUP_AUTO_ENABLED, onAutoEnabled);
    const startup = window.setTimeout(() => {
      void readBackupEntries().catch(() => {});
      const resume = resumeDriveJobs(rescan).catch(notifyError);
      void Promise.all([rescan(), resume]).then(backup);
    }, 750);
    const every = window.setInterval(onAutoEnabled, 10 * 60 * 1000);
    const stopUpdates = startAutoUpdate();
    const offSync = listen("tray-sync", () => setSyncOpen(true));
    const offScan = listen("tray-rescan", () => void rescan());
    const offSearch = listen("tray-search", () => setPaletteOpen(true));
    const offSettings = listen("tray-settings", () => setSettingsOpen(true));
    return () => {
      cancelled = true;
      window.removeEventListener(BACKUP_AUTO_ENABLED, onAutoEnabled);
      window.clearTimeout(startup);
      void un.then((f) => f());
      void offSync.then((f) => f());
      void offScan.then((f) => f());
      void offSearch.then((f) => f());
      void offSettings.then((f) => f());
      window.clearInterval(every);
      stopUpdates();
    };
  }, [rescan]);

  useEffect(() => {
    if (!isTauri) return;
    PROVIDERS.forEach((p) =>
      invoke<Overview>("provider_overview", { provider: p })
        .then((o) => setOverview((s) => ({ ...s, [p]: o })))
        .catch(() => {}),
    );
  }, [reloadKey, liveTick]);

  // Pinned chats of the current tool.
  useEffect(() => {
    if (!isTauri) return;
    const byTool = new Map<string, string[]>();
    for (const k of pins) {
      const at = k.indexOf(":");
      const p = k.slice(0, at);
      if (provider === "all" || p === provider) byTool.set(p, [...(byTool.get(p) ?? []), k.slice(at + 1)]);
    }
    if (!byTool.size) {
      setPinned([]);
      return;
    }
    Promise.all([...byTool].map(([p, ids]) => invoke<Chat[]>("chats_by_ids", { provider: p, ids }).catch(() => [] as Chat[])))
      .then((lists) => setPinned(lists.flat().sort((a, b) => b.lastMs - a.lastMs)))
      .catch(() => {});
  }, [pins, provider, reloadKey, liveTick]);

  // First page of the list whenever the tool, account, search or index changes.
  const searchingAll = q.length >= 2;
  useEffect(() => {
    if (!isTauri || searchingAll) return;
    let stale = false;
    setLoadingList(true);
    setHits(null);
    invoke<{ items: Chat[]; total: number }>("query_chats", { provider, account, q, offset: 0, limit: PAGE_SIZE, sort, scope })
      .then((r) => {
        if (stale) return;
        setItems(r.items);
        setTotal(r.total);
        setError(null);
      })
      .catch((e) => !stale && setError(String(e)))
      .finally(() => !stale && setLoadingList(false));
    return () => {
      stale = true;
    };
  }, [provider, account, q, sort, scope, reloadKey, searchingAll]);

  // Searching looks in every tool at once: matching titles and projects, plus the words inside the chats (instant, from the search index).
  useEffect(() => {
    if (!isTauri || !searchingAll) return;
    let stale = false;
    setSearching(true);
    void (async () => {
      try {
        // One ranked search over every tool: titles (including names you gave chats here), projects and the words inside.
        const found = await invoke<Found[]>("search_chats", { query: q, renamed: namesRef.current, includeArchived: false, limit: 150 });
        if (stale) return;
        const snippets: Record<string, string> = {};
        found.forEach((f) => f.snippet && (snippets[f.chat.id] = f.snippet));
        setItems(found.map((f) => f.chat));
        setTotal(found.length);
        setHits(snippets);
        setError(null);
        setLoadingList(false);
      } catch (e) {
        if (!stale) setError(String(e));
      } finally {
        if (!stale) setSearching(false);
      }
    })();
    return () => {
      stale = true;
    };
  }, [q, searchingAll, reloadKey, liveTick]);

  // "Getting your chats ready…" stays up from the very start until the first list of chats is on screen,
  // so the app never shows an empty or half-loaded sidebar. If loading takes unusually long it steps aside after 8 seconds.
  const listReady = !loadingList || !isTauri;
  const [splashTimedOut, setSplashTimedOut] = useState(false);
  useEffect(() => {
    const t = window.setTimeout(() => setSplashTimedOut(true), 8000);
    return () => window.clearTimeout(t);
  }, []);
  useEffect(() => {
    if (splash !== "on" || !(listReady || splashTimedOut)) return;
    const frame = requestAnimationFrame(() => setSplash("leaving"));
    return () => cancelAnimationFrame(frame);
  }, [splash, listReady, splashTimedOut]);
  useEffect(() => {
    if (splash !== "leaving") return;
    const timer = window.setTimeout(() => setSplash("off"), 320);
    return () => window.clearTimeout(timer);
  }, [splash]);

  const loadingMore = useRef(false);
  const loadMore = useCallback(async () => {
    if (loadingMore.current || hits || searchingAll) return;
    loadingMore.current = true;
    try {
      const r = await invoke<{ items: Chat[]; total: number }>("query_chats", { provider, account, q, offset: items.length, limit: PAGE_SIZE, sort, scope });
      setItems((cur) => [...cur, ...r.items]);
      setTotal(r.total);
    } finally {
      loadingMore.current = false;
    }
  }, [provider, account, q, sort, scope, items.length, hits, searchingAll]);

  const sentinelRef = useRef<HTMLDivElement | null>(null);
  const hasMore = !hits && items.length < total;
  useEffect(() => {
    const el = sentinelRef.current;
    if (!el || !hasMore) return;
    let timer: number | undefined;
    const io = new IntersectionObserver((entries) => {
      if (entries[0].isIntersecting && timer === undefined) {
        timer = window.setTimeout(() => {
          timer = undefined;
          void loadMore();
        }, 250);
      }
    });
    io.observe(el);
    return () => {
      io.disconnect();
      window.clearTimeout(timer);
    };
  }, [hasMore, loadMore, items.length, recentOpen, todayOpen, yesterdayOpen]);

  const openId = useRef(0);
  const [outline, setOutline] = useState<{ ts: string; text: string }[] | null>(null);
  // What was searched for when a chat was opened from search results: the chat opens with those words highlighted.
  const [openQuery, setOpenQuery] = useState("");
  const open = useCallback(
    (chat: Chat, query = "") => {
      const id = ++openId.current;
      setOpenQuery(query);
      setSearchAll(false);
      setSelected(chat);
      setPage(null);
      setOutline(null);
      // A light outline of the whole chat for the timeline, built in the background; only a few short lines come back.
      invoke<{ ts: string; text: string }[]>("chat_outline", { provider: chat.provider, path: chat.path })
        .then((o) => {
          if (openId.current === id) setOutline(o);
        })
        .catch(() => {});
      // The newest messages come first, read from the end of the file, so the chat is readable at once.
      invoke<Msg[]>("read_chat_tail", { provider: chat.provider, path: chat.path, limit: MSG_PAGE })
        .then((messages) => {
          if (openId.current === id) setPage((cur) => cur ?? { messages, total: messages.length, start: 0, partial: true });
        })
        .catch(() => {});
      // The real total follows; earlier messages are only read as you scroll up toward them.
      invoke<MsgPage>("read_chat_page", { provider: chat.provider, path: chat.path, before: null, limit: MSG_PAGE })
        .then((r) => {
          if (openId.current === id) setPage(r);
        })
        .catch((e) => {
          if (openId.current !== id) return;
          setPage((cur) => (cur ? { ...cur, partial: false } : { messages: [], total: 0, start: 0 }));
          say(String(e));
        });
    },
    [say],
  );

  // Keep the open chat live: when its file grows, only the new messages are read and added at the bottom.
  // A fork is a new file next to the original: index it, name it, and open it, all without a terminal.
  useEffect(() => {
    const onFork = async (e: Event) => {
      const d = (e as CustomEvent<{ from: Chat; id: string; path: string }>).detail;
      try {
        await invoke("refresh_index");
        setReloadKey((k) => k + 1);
        const r = await invoke<{ items: Chat[] }>("query_chats", { provider: d.from.provider, account: "all", q: "", offset: 0, limit: 40, sort: "recent", scope: "all" });
        const made = r.items.find((c) => c.id === d.id);
        if (made) {
          rename(chatKey(made), `${names[chatKey(d.from)] || d.from.title} (fork)`);
          setProvider(made.provider);
          open(made);
        }
      } catch (err) {
        say(String(err));
      }
    };
    window.addEventListener("chatvault:forked", onFork);
    return () => window.removeEventListener("chatvault:forked", onFork);
  }, [rename, names, open, say]);

  // Chats an agent is writing to right now (their file changed in the last 20 seconds) get a green dot in the list.
  const [liveKeys, setLiveKeys] = useState<Set<string>>(new Set());
  const listed = useRef<Chat[]>([]);
  listed.current = [...pinned, ...items];
  useEffect(() => {
    let busy = false;
    const tick = async () => {
      if (busy || document.hidden) return;
      busy = true;
      try {
        const seen = new Map<string, Chat>();
        listed.current.forEach((c) => !isHandoffOnly(c.provider) && seen.set(chatKey(c), c));
        const chats = [...seen.values()];
        const ms = await invoke<number[]>("chat_stamps", { items: chats.map((c) => [c.provider, c.path]) });
        const now = Date.now();
        const next = new Set<string>();
        chats.forEach((c, i) => ms[i] && now - ms[i] < 20000 && next.add(chatKey(c)));
        setLiveKeys((cur) => (cur.size === next.size && [...next].every((k) => cur.has(k)) ? cur : next));
      } catch {
        /* try again next time */
      } finally {
        busy = false;
      }
    };
    void tick();
    const id = window.setInterval(tick, 3000);
    return () => window.clearInterval(id);
  }, []);

  // Every tool: new conversations and new messages show up on their own. A new file is picked up at once;
  // changes inside existing chats are folded in at most every 15 seconds.
  // Refreshes the list in place: keeps as many rows as are already shown, so the sidebar does not jump back to the top.
  const listArgs = useRef({ provider, account, q, sort, scope, count: 0 });
  listArgs.current = { provider, account, q, sort, scope, count: items.length };
  const quietReload = useCallback(async () => {
    const l = listArgs.current;
    // While a search is shown, the search itself is run again (it follows liveTick) instead of replacing its results.
    if (l.q.length >= 2) return;
    const r = await invoke<{ items: Chat[]; total: number }>("query_chats", { provider: l.provider, account: l.account, q: l.q, offset: 0, limit: Math.min(300, Math.max(PAGE_SIZE, l.count)), sort: l.sort, scope: l.scope });
    setItems(r.items);
    setTotal(r.total);
  }, []);
  const cursorFp = useRef(0);
  const cursorChanged = useRef(0);
  const filesFp = useRef<[number, number] | null>(null);
  const lastRefresh = useRef(0);
  // A change seen but not yet applied. It is never dropped: it waits for the next allowed refresh.
  const changePending = useRef(false);
  useEffect(() => {
    if (!isTauri) return;
    let busy = false;
    const id = window.setInterval(async () => {
      if (busy || document.hidden) return;
      busy = true;
      try {
        const [fp, ff] = await Promise.all([invoke<number>("cursor_fingerprint"), invoke<[number, number]>("files_fingerprint")]);
        const cursorMoved = cursorFp.current !== 0 && fp !== cursorFp.current;
        const prev = filesFp.current;
        const fresh = !!prev && ff[0] !== prev[0];
        if (prev && (ff[0] !== prev[0] || ff[1] !== prev[1])) changePending.current = true;
        cursorFp.current = fp;
        filesFp.current = ff;
        if (cursorMoved) cursorChanged.current = Date.now();
        // New chats and Cursor changes are applied at once; changes inside existing chats and renames at most every 4 seconds.
        const due = changePending.current && Date.now() - lastRefresh.current > 4000;
        if (cursorMoved || fresh || due) {
          lastRefresh.current = Date.now();
          changePending.current = false;
          await invoke("refresh_index");
          void invoke("build_search_index").catch(() => {});
          await quietReload();
          setLiveTick((n) => n + 1);
          // The open chat's header follows a rename made in its own tool.
          const shownChat = selRef.current;
          if (shownChat) {
            const [latest] = await invoke<Chat[]>("chats_by_ids", { provider: shownChat.provider, ids: [shownChat.id] }).catch(() => [] as Chat[]);
            if (latest && selRef.current?.id === shownChat.id && selRef.current.provider === shownChat.provider && latest.title !== selRef.current.title) {
              setSelected((s) => (s && s.id === latest.id && s.provider === latest.provider ? { ...s, title: latest.title } : s));
            }
          }
          const sel = selRef.current;
          const cur = pageRef.current;
          if (cursorMoved && sel?.provider === "cursor" && cur && !cur.partial) {
            const r = await invoke<MsgPage>("read_chat_page", { provider: "cursor", path: sel.path, before: null, limit: 2000 });
            if (selRef.current?.id === sel.id && r.total !== cur.total) setPage(r);
          }
        }
        if (selRef.current?.provider === "cursor") setLive(Date.now() - cursorChanged.current < 20000);
      } catch {
        /* try again next time */
      } finally {
        busy = false;
      }
    }, 2000);
    return () => window.clearInterval(id);
  }, [quietReload]);

  const stamp = useRef("");
  const [live, setLive] = useState(false);
  const selRef = useRef(selected);
  selRef.current = selected;
  useEffect(() => {
    stamp.current = "";
    setLive(false);
    const sel = selected;
    if (!sel || isHandoffOnly(sel.provider)) return;
    let busy = false;
    const id = window.setInterval(async () => {
      const cur = pageRef.current;
      if (busy || !cur || cur.partial || selRef.current?.id !== sel.id) return;
      busy = true;
      try {
        const [size, ms] = await invoke<[number, number]>("chat_stamp", { provider: sel.provider, path: sel.path });
        // Working = the file changed a moment ago, or your message / a tool call is still waiting for the agent to continue.
        const last = cur.messages[cur.messages.length - 1];
        const waiting = !!last && (last.role === "user" || last.kind === "tool");
        const age = Date.now() - ms;
        setLive(age < 20000 || (waiting && age < 120000));
        const st = `${size}:${ms}`;
        if (!stamp.current) {
          stamp.current = st;
          return;
        }
        if (st === stamp.current) return;
        stamp.current = st;
        // Native history updates tool items in place as well as appending items.
        if (sel.path.startsWith("codex-db:")) {
          const r = await invoke<MsgPage>("read_chat_page", { provider: sel.provider, path: sel.path, before: null, limit: 2000 });
          if (selRef.current?.path !== sel.path || selRef.current?.provider !== sel.provider) return;
          if (r.total < cur.total || r.start > cur.total) { setPage(r); return; }
          setPage(p => {
            if (!p || p.total !== cur.total) return p;
            // Native pagination counts stored items, even when an item renders several chat parts.
            const prefix = p.messages.filter(m => m.sourceIndex !== undefined && m.sourceIndex < r.start);
            return { ...r, messages: [...prefix, ...r.messages], start: Math.min(p.start, r.start) };
          });
          return;
        }
        // Fast path: only the end of the file is read, and what is new is whatever follows the last message already shown.
        const sig = (m: Msg) => `${m.ts}|${m.role}|${m.kind}|${m.text.slice(0, 80)}`;
        const lastShown = cur.messages[cur.messages.length - 1];
        if (lastShown && !isHandoffOnly(sel.provider)) {
          try {
            const tail = await invoke<Msg[]>("read_chat_tail", { provider: sel.provider, path: sel.path, limit: 60 });
            const at = tail.map(sig).lastIndexOf(sig(lastShown));
            if (at >= 0) {
              const add = tail.slice(at + 1);
              if (selRef.current?.id !== sel.id) return;
              if (add.length) setPage((p) => (p && p.total === cur.total ? { ...p, messages: [...p.messages, ...add], total: p.total + add.length } : p));
              return;
            }
          } catch {
            /* fall back to the full read below */
          }
        }
        let r = await invoke<MsgPage>("read_chat_page", { provider: sel.provider, path: sel.path, before: null, limit: 50 });
        const diff = r.total - cur.total;
        if (diff > 50) r = await invoke<MsgPage>("read_chat_page", { provider: sel.provider, path: sel.path, before: null, limit: Math.min(diff, 2000) });
        if (selRef.current?.id !== sel.id) return;
        if (diff <= 0) {
          if (diff < 0) setPage(r);
          return;
        }
        const add = r.messages.slice(-diff);
        setPage((p) => (p && p.total === cur.total ? { ...p, messages: [...p.messages, ...add], total: r.total } : p));
      } catch {
        /* the file may be mid-write; try again next time */
      } finally {
        busy = false;
      }
    }, 600);
    return () => window.clearInterval(id);
  }, [selected?.id, selected?.provider, selected?.path]);

  // Serialize history reads so scroll, timeline and search cannot replace one another's pages.
  const historyQueue = useRef<Promise<void>>(Promise.resolve());
  const earlierBusy = useRef(false);
  const queueHistory = (work: (chat: Chat, valid: () => boolean) => Promise<void>) => {
    const chat = selRef.current;
    const generation = openId.current;
    const valid = () => !!chat && openId.current === generation && selRef.current?.id === chat.id && selRef.current?.provider === chat.provider && selRef.current?.path === chat.path;
    const job = historyQueue.current.then(async () => {
      if (chat && valid()) await work(chat, valid);
    });
    historyQueue.current = job.catch(() => {});
    return job;
  };
  const prependEarlier = (r: MsgPage, from: number) => {
    setPage((cur) => {
      if (!cur || cur.start !== from) return cur;
      return { ...cur, messages: [...r.messages, ...cur.messages], total: Math.max(r.total, cur.total), start: r.start };
    });
  };
  const loadEarlier = async () => {
    if (earlierBusy.current) return;
    earlierBusy.current = true;
    setLoadingEarlier(true);
    try {
      await queueHistory(async (chat, valid) => {
        const cur = pageRef.current;
        if (!cur || cur.partial || cur.start === 0) return;
        const r = await invoke<MsgPage>("read_chat_page", { provider: chat.provider, path: chat.path, before: cur.start, limit: 120 });
        if (valid()) prependEarlier(r, cur.start);
      });
    } catch (e) {
      say(String(e));
    } finally {
      earlierBusy.current = false;
      setLoadingEarlier(false);
    }
  };
  const loadHistoryTo = (target: number, limit: number) => queueHistory(async (chat, valid) => {
    while (valid()) {
      const cur = pageRef.current;
      if (!cur || cur.partial || cur.start === 0) return;
      const oldest = cur.messages.find((m) => m.ts)?.ts;
      if (oldest && Date.parse(oldest) <= target) return;
      const r = await invoke<MsgPage>("read_chat_page", { provider: chat.provider, path: chat.path, before: cur.start, limit });
      if (!valid()) return;
      if (r.start >= cur.start) throw new Error("Earlier messages could not be loaded. Please refresh this chat.");
      prependEarlier(r, cur.start);
    }
  });
  const loadUntil = async (ts: string) => {
    try {
      await loadHistoryTo(Date.parse(ts), 400);
    } catch (e) {
      say(String(e));
    }
  };
  const loadAll = async () => {
    try {
      await loadHistoryTo(Number.NEGATIVE_INFINITY, 2000);
    } catch (e) {
      say(String(e));
    }
  };

  const switchProvider = (p: Tool) => {
    if (p === provider) return;
    setProvider(p);
    setQuery("");
    setSelected(null);
  };

  const tag = (id: string) => {
    const a = accountInfo.find((x) => x.id === id);
    const parts = [a?.desktopCurrent && "Desktop", a?.cliCurrent && "CLI"].filter(Boolean);
    return parts.length ? ` · ${parts.join(" + ")}` : "";
  };
  // Every tool with chats, plus the three main ones. The picker grows with the tools ChatVault supports.
  const toolList: Tool[] = ["all", ...PROVIDERS.filter((p) => CORE_PROVIDERS.includes(p) || (overview[p]?.total ?? 0) > 0 || p === provider)];
  const stackTools = toolList.filter((x): x is Provider => x !== "all");
  const toolName = (p: Tool) => (p === "all" ? "All chats" : PROVIDER_NAME[p]);
  const toolTotal = (p: Tool) => (p === "all" ? PROVIDERS.reduce((n, x) => n + (overview[x]?.total ?? 0), 0) : overview[p]?.total ?? 0);
  const toolItems = Object.fromEntries(toolList.map((p) => [p, toolName(p)]));
  const accountList = provider === "all" ? [] : overview[provider]?.accounts ?? [];
  const accountItems = useMemo(
    () => ({
      all: `All accounts · ${toolTotal(provider).toLocaleString()}`,
      ...Object.fromEntries(accountList.map((a) => [a.id, `${accountLabel(a.id)} · ${a.count.toLocaleString()}${provider === "claude" ? tag(a.id) : ""}`])),
    }),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [accountList, overview, provider, accountInfo, accountNames],
  );

  const lq = q.toLowerCase();
  const pinnedShown = pinned.filter((c) => !lq || named(c).title.toLowerCase().includes(lq) || c.project.toLowerCase().includes(lq));
  const recent = useMemo(() => items.filter((c) => !pinSet.has(chatKey(c))), [items, pinSet]);
  // While searching, results stay in one list in the order the search ranked them (best match first), not split by day.
  const dayGroups = useMemo(
    () => (searchingAll ? { today: [] as Chat[], yesterday: [] as Chat[], recent } : groupChatsByDay(recent)),
    [recent, calendarDay, searchingAll],
  );
  const projectGroups = useMemo(() => groupByProject(recent), [recent]);
  const projectNames = useMemo(() => projectGroups.map(([name]) => name), [projectGroups]);
  const allProjectsClosed = projectNames.length > 0 && projectNames.every((n) => !projectOpen(n));
  const msgs: Msg[] | null = page?.messages ?? null;

  const extras = (c: Chat): ChatAction[] => orderChat(([
    { id: "view", label: "Open in ChatVault", hint: "Reads this chat here.", icon: Eye, group: "Chat", run: async () => open(c) },
    {
      id: "pin",
      label: pinSet.has(chatKey(c)) ? "Unpin" : "Pin to top",
      hint: "Pinned chats stay at the top of the list.",
      icon: pinSet.has(chatKey(c)) ? PinOff : Pin,
      group: "Chat",
      run: async () => togglePin(chatKey(c)),
    },
    {
      id: "delete",
      label: "Delete chat",
      hint: ({ trash: "Moves the chat to the Recycle Bin.", opencode: "Deletes the session from OpenCode. It cannot be restored.", blocked: "Not available: this chat is stored inside another app." } as const)[deleteKind(c)],
      icon: Trash2,
      group: "Chat",
      danger: true,
      muted: deleteKind(c) === "blocked",
      run: async () => requestDelete(c),
    },
    {
      id: "archive",
      label: c.userArchived ? "Unarchive" : "Archive",
      hint: c.userArchived ? "Moves this chat back to your list." : "Moves this chat out of your list into Archived. Nothing is deleted.",
      icon: c.userArchived ? ArchiveRestore : Archive,
      group: "Chat",
      run: async () => archive(c),
    },
    { id: "tags", label: "Tags", hint: "Add your own labels to this chat. Only shown in ChatVault.", icon: Tag, group: "Chat", run: async () => setTagging(c) },
    { id: "rename", label: "Rename", hint: "Give this chat your own name. The original is not changed.", icon: Pencil, group: "Chat", run: async () => setRenaming(c) },
  ] as ChatAction[]));

  const loadTags = useCallback(() => {
    if (!isTauri) return;
    Promise.all((provider === "all" ? PROVIDERS : [provider]).map((p) => invoke<[string, number][]>("all_tags", { provider: p }).catch(() => [] as [string, number][])))
      .then((lists) => {
        const sum = new Map<string, number>();
        lists.flat().forEach(([t, n]) => sum.set(t, (sum.get(t) ?? 0) + n));
        setTagList([...sum].sort((a, b) => a[0].localeCompare(b[0])));
      })
      .catch(() => {});
  }, [provider]);
  useEffect(loadTags, [loadTags, reloadKey]);

  /** Opens the confirm box, or says why this chat cannot be deleted from here. */
  const requestDelete = (c: Chat) => {
    if (deleteKind(c) === "blocked") say(deleteBlockedMessage(PROVIDER_NAME[c.provider]));
    else setDeleting(c);
  };

  const archive = async (c: Chat) => {
    const to = !c.userArchived;
    try {
      await invoke("set_chat_archived", { provider: c.provider, id: c.id, archived: to });
      say(to ? "Moved to Archived" : "Moved back to your list");
      if (selected?.id === c.id && selected.provider === c.provider) setSelected(to && scope !== "archived" ? { ...c, userArchived: to } : { ...c, userArchived: to });
      await quietReload();
    } catch (e) {
      say(String(e));
    }
  };

  /** Right-click on the empty part of the sidebar: how the list is organised, sorted and filtered, plus the usual tools. */
  const listEntries = (): Entry[] => [
    { type: "item", label: "Refresh", icon: <RefreshCw strokeWidth={1.5} />, onSelect: () => void rescan() },
    { type: "sep" },
    { type: "label", label: "Organize" },
    { type: "check", label: "By project", icon: <FolderOpen strokeWidth={1.5} />, checked: organize === "project", onSelect: () => setOrganize("project") },
    { type: "check", label: "In one list", icon: <ListFilter strokeWidth={1.5} />, checked: organize === "list", onSelect: () => setOrganize("list") },
    ...(organize === "project" && projectNames.length > 0
      ? ([{ type: "item", label: allProjectsClosed ? "Expand all projects" : "Collapse all projects", icon: <ChevronsUpDown strokeWidth={1.5} />, onSelect: () => { setProjectsMode(allProjectsClosed ? "open" : "closed"); setProjectExceptions([]); } }] as Entry[])
      : []),
    { type: "sep" },
    { type: "sub", label: "Sort chats by", icon: <ArrowUpDown strokeWidth={1.5} />, entries: Object.entries(SORTS).map(([k, label]) => ({ type: "check", label, checked: sort === k, onSelect: () => setSort(k) }) as Entry) },
    {
      type: "sub",
      label: "Show",
      icon: <Eye strokeWidth={1.5} />,
      entries: [
        ...Object.entries(SCOPES).map(([k, label]) => ({ type: "check", label, checked: scope === k, onSelect: () => setScope(k) }) as Entry),
        ...(tagList.length ? ([{ type: "sep" }] as Entry[]) : []),
        ...tagList.map(([t, n]) => ({ type: "check", label: `${t} (${n})`, icon: <Tag strokeWidth={1.5} />, checked: scope === `tag:${t}`, onSelect: () => setScope(`tag:${t}`) }) as Entry),
      ],
    },
    { type: "item", label: "Reset", icon: <RotateCcw strokeWidth={1.5} />, disabled: sort === "recent" && scope === "all" && organize === "list", onSelect: () => { setSort("recent"); setScope("all"); setOrganize("list"); } },
    { type: "sep" },
    { type: "item", label: "Search all chats", icon: <Search strokeWidth={1.5} />, onSelect: () => openSearch() },
    { type: "item", label: "Sync & backup", icon: <DatabaseBackup strokeWidth={1.5} />, onSelect: () => setSyncOpen(true) },
    { type: "item", label: "Settings", icon: <SettingsIcon strokeWidth={1.5} />, onSelect: () => setSettingsOpen(true) },
    { type: "item", label: "Hide the sidebar", icon: <PanelLeft strokeWidth={1.5} />, onSelect: () => setSidebarOpen(false) },
  ];

  // Stable wrappers around the latest closures, so a card only re-renders when its own data changes.
  const latest = useRef({ extras, open, found: "" });
  latest.current = { extras, open, found: searchingAll ? q : "" };
  const getExtras = useCallback((c: Chat) => latest.current.extras(c), []);
  const openChat = useCallback((c: Chat) => latest.current.open(c, latest.current.found), []);

  const row = (c: Chat, inProject = false) => (
    <ChatRow
      key={chatKey(c)}
      chat={named(c)}
      selected={selected?.id === c.id && selected.provider === c.provider}
      pinned={pinSet.has(chatKey(c))}
      live={liveKeys.has(chatKey(c))}
      hit={hits?.[c.id]}
      hitQuery={searchingAll ? q : undefined}
      inProject={inProject}
      msgs={selected?.id === c.id ? msgs : null}
      getExtras={getExtras}
      onOpen={openChat}
      toast={say}
    />
  );


  // Search is on every page; Sync & backup and Settings belong to the main page only.
  const searchButton = (
    <Tip label="Search chats and actions (Ctrl+K)">
      <Button variant="outline" size="sm" className="text-muted-foreground" onClick={() => setPaletteOpen(true)}>
        <Search className="size-4" /> Search
        <kbd className="ml-1 rounded border bg-muted px-1 text-[10px]">Ctrl K</kbd>
      </Button>
    </Tip>
  );
  const controls = (
    <>
            {searchButton}
            <Tip label="Back up or restore your chats.">
              <Button size="sm" onClick={() => setSyncOpen(true)}>
                <DatabaseBackup className="size-4" /> Sync &amp; backup
              </Button>
            </Tip>
            <Tip label="Settings: where each tool's chats are read from.">
              <Button size="icon" className="size-8" aria-label="Settings" onClick={() => setSettingsOpen(true)}>
                <SettingsIcon className="size-4" />
              </Button>
            </Tip>
    </>
  );

  const renderChatList = (list: Chat[]) => (
    organize === "project"
                  ? groupByProject(list).map(([project, list]) => {
                      const isOpen = projectOpen(project);
                      return (
                        <Collapsible
                          key={project}
                          open={isOpen}
                          onOpenChange={(o) => setProjectExceptions((c) => (o === (projectsMode === "open") ? c.filter((x) => x !== project) : c.includes(project) ? c : [...c, project]))}
                          className="pt-1"
                          onPointerEnter={() => setWarmProjects((w) => (w.has(project) ? w : new Set(w).add(project)))}
                        >
                          <CollapsibleTrigger className="flex w-full items-center gap-1.5 rounded-md px-2 py-1 text-xs text-muted-foreground hover:bg-muted/60 hover:text-foreground">
                            <ChevronRight className={cn("size-3.5 shrink-0 transition-transform", isOpen && "rotate-90")} />
                            <FolderOpen className="size-3.5 shrink-0" strokeWidth={1.5} />
                            <span className="min-w-0 flex-1 truncate text-left">{prettyProject(project)}</span>
                            <Badge variant="secondary" className="h-4 px-1.5 text-[11px] tabular-nums text-muted-foreground">{list.length}</Badge>
                          </CollapsibleTrigger>
                          <CollapsibleContent className="space-y-0.5" keepMounted={warmProjects.has(project)}>
                            {(isOpen || warmProjects.has(project)) && list.map((c) => row(c, true))}
                          </CollapsibleContent>
                        </Collapsible>
                      );
                    })
                  : list.map((c) => row(c))
  );

  return (
    <TooltipProvider delay={200}>
      <div className="flex h-screen overflow-hidden bg-background text-foreground">
      {splash !== "off" && <Splash leaving={splash === "leaving"} />}
        {/* Sidebar */}
        <aside className={cn("flex shrink-0 flex-col overflow-hidden border-r transition-[width] duration-300 ease-in-out", sidebarOpen ? "w-[320px]" : "w-0 border-r-0")}>
          {sidebarOpen ? (
            <div data-tauri-drag-region className="flex h-14 shrink-0 items-center gap-1 pl-3 pr-14">
              <button
                onClick={() => setSelected(null)}
                className="flex min-w-0 flex-1 cursor-pointer items-center gap-2.5 p-1 text-left outline-none"
              >
                <AppLogo className="size-7 shrink-0 rounded-lg ring-1 ring-border" />
                <div className="flex min-w-0 items-center gap-2">
                  <h1 className="truncate text-base font-medium leading-tight">ChatVault</h1>
                </div>
              </button>
            </div>
          ) : null}
          <div className={cn("flex min-h-0 w-[320px] flex-1 flex-col transition-opacity duration-200", sidebarOpen ? "opacity-100" : "pointer-events-none opacity-0")}>

          <div className="space-y-2 px-3 pb-3">
            <div className="relative">
              <Search className="absolute left-2.5 top-2.5 size-4 text-muted-foreground" />
              <Input
                className="h-9 pl-8"
                placeholder="Search chats"
                value={query}
                onChange={(e) => setQuery(e.target.value)}
              />
              {searching && <Loader2 className="absolute right-2.5 top-2.5 size-4 animate-spin text-muted-foreground" />}
            </div>
            <div className={cn("grid gap-2", provider === "all" ? "grid-cols-1" : "grid-cols-2")}>
              <Select value={provider} onValueChange={(v) => v && switchProvider(v as Tool)} items={toolItems}>
                <SelectTrigger className="h-9 w-full min-w-0">
                  {provider === "all" ? <Layers className="size-4 shrink-0 text-muted-foreground" strokeWidth={1.5} /> : <ProviderLogo provider={provider} className="size-4 shrink-0" />}
                  <SelectValue />
                  {provider === "all" && <ProviderStack providers={stackTools} className="ml-auto mr-1" />}
                </SelectTrigger>
                <SelectContent alignItemWithTrigger={false} align="start" sideOffset={6} className={cn("max-w-none", provider === "all" ? "w-(--anchor-width)" : "w-[calc(var(--anchor-width)*2+0.5rem)]")}>
                  {toolList.map((p) => (
                    <SelectItem key={p} value={p} className="pr-1" trailing={<span className="min-w-7 text-right text-xs tabular-nums text-muted-foreground">{toolTotal(p).toLocaleString()}</span>}>
                      {p === "all" ? <Layers className="size-4 shrink-0 text-muted-foreground" strokeWidth={1.5} /> : <ProviderLogo provider={p} className="size-4 shrink-0" />}
                      <span className="flex-1">{toolName(p)}</span>
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
              {provider !== "all" && (
              <Select open={accountOpen} onOpenChange={setAccountOpen} value={account} onValueChange={(v) => v && setAccounts((a) => ({ ...a, [provider]: v }))} items={accountItems}>
                <SelectTrigger className="h-9 w-full">
                  <Users className="size-4 shrink-0 text-muted-foreground" strokeWidth={1.5} />
                  <SelectValue />
                </SelectTrigger>
                <SelectContent alignItemWithTrigger={false} align="end" sideOffset={6} className="w-72">
                  {Object.entries(accountItems).map(([id, label]) => (
                    <SelectItem
                      key={id}
                      value={id}
                      trailing={
                        id !== "all" && id !== "unknown" ? (
                          <button
                            type="button"
                            aria-label={`Rename ${accountLabel(id)}`}
                            title="Rename account"
                            className="ml-1 flex size-6 shrink-0 cursor-pointer items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
                            onPointerDown={(e) => e.stopPropagation()}
                            onPointerUp={(e) => e.stopPropagation()}
                            onMouseDown={(e) => e.stopPropagation()}
                            onClick={(e) => {
                              e.preventDefault();
                              e.stopPropagation();
                              setAccountOpen(false);
                              setRenamingAccount(id);
                            }}
                          >
                            <Pencil className="size-3.5" strokeWidth={1.5} />
                          </button>
                        ) : undefined
                      }
                    >
                      {/* A long name is cut with "…" and shown in full on hover, so the tick and the pencil always stay in view. */}
                      <span className="block max-w-[13.5rem] truncate" title={label}>
                        {label}
                      </span>
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
              )}
            </div>
          </div>
          <Separator className="mx-3 w-auto opacity-70" />

          <ContextArea className="flex min-h-0 flex-1 flex-col" entries={() => listEntries()} onError={(e) => say(String(e))}>
          <FadedScroll className="flex-1" fadeTop={false}>
            <div className="p-1.5 pr-3.5">
              {pinnedShown.length > 0 && (
                <Section title="Pinned" count={pinnedShown.length} open={pinnedOpen} onOpenChange={setPinnedOpen}>
                  {pinnedShown.map((c) => row(c))}
                </Section>
              )}
              {/* A day with no chats is left out, so the list never shows an empty heading. */}
              {dayGroups.today.length > 0 && (
                <Section title="Today" count={dayGroups.today.length} open={todayOpen} onOpenChange={setTodayOpen}>
                  {renderChatList(dayGroups.today)}
                </Section>
              )}
              {dayGroups.yesterday.length > 0 && (
                <Section title="Yesterday" count={dayGroups.yesterday.length} open={yesterdayOpen} onOpenChange={setYesterdayOpen}>
                  {renderChatList(dayGroups.yesterday)}
                </Section>
              )}
              <Section
                title={searchingAll ? "Best matches" : "Recent"}
                count={dayGroups.recent.length}
                open={recentOpen}
                onOpenChange={setRecentOpen}
                action={
                  <div className="flex items-center">
                    {organize === "project" && (
                      <Tip label={allProjectsClosed ? "Expand all projects" : "Collapse all projects"}>
                        <Button
                          variant="ghost"
                          size="icon"
                          aria-label={allProjectsClosed ? "Expand all projects" : "Collapse all projects"}
                          className="size-7 shrink-0 text-muted-foreground"
                          onClick={() => {
                            setProjectsMode(allProjectsClosed ? "open" : "closed");
                            setProjectExceptions([]);
                          }}
                        >
                          {allProjectsClosed ? <ChevronsUpDown className="size-4" /> : <ChevronsDownUp className="size-4" />}
                        </Button>
                      </Tip>
                    )}
                  <FiltersMenu
                    sort={sort}
                    scope={scope}
                    tags={tagList}
                    organize={organize}
                    onSort={setSort}
                    onScope={setScope}
                    onOrganize={setOrganize}
                    onReset={() => {
                      setSort("recent");
                      setScope("all");
                      setOrganize("list");
                    }}
                  />
                  </div>
                }
              >
                {(loadingList || !!indexing || (searching && items.length === 0)) && items.length === 0 && <SidebarSkeleton rows={searching ? 5 : 8} />}
                {!loadingList && !indexing && !searching && !error && recent.length === 0 && (
                  <div className="flex flex-col items-center gap-2 p-8 text-center">
                    <Search className="size-8 text-muted-foreground" strokeWidth={1.25} />
                    <p className="text-sm font-medium">No Chats Found</p>
                    <p className="text-xs text-muted-foreground">Try a different word or account.</p>
                  </div>
                )}
                {renderChatList(dayGroups.recent)}
              </Section>
              {hasMore && (todayOpen || yesterdayOpen || recentOpen) && (
                <div ref={sentinelRef}>
                  <SidebarSkeleton rows={3} />
                </div>
              )}
            </div>
          </FadedScroll>
          </ContextArea>
          </div>
        </aside>

        {/* Content */}
        <div className="flex min-w-0 flex-1 flex-col">
        <TitleBar title={selected ? `${PROVIDER_NAME[selected.provider]} / ${prettyProject(selected.project)}` : undefined} actions={selected ? searchButton : undefined} sidebarOpen={sidebarOpen} busy={!!indexing} dark={dark} onRefresh={rescan} onTheme={() => setDark((d) => !d)} onDriveSettings={() => { setSyncTab("drive"); setSyncOpen(true); }} />
        <main className="flex min-h-0 min-w-0 flex-1 flex-col">
          <Suspense fallback={<MessagesSkeleton />}>
          {searchAll ? (
            <GlobalSearch
              key={searchSeed}
              initial={searchSeed}
              renamed={names}
              titleOf={(c) => named(c).title}
              onClose={() => setSearchAll(false)}
              onOpenChat={(c, term) => {
                setSearchAll(false);
                if (c.provider !== provider) setProvider(c.provider);
                open(c, term);
              }}
            />
          ) : !selected ? (
            <OverviewPage
              actions={controls}
              indexing={indexing}
              userName={accountInfo.find((a) => a.cliCurrent)?.name ?? undefined}
              overview={overview}
              accountCount={new Set([...(overview.claude?.accounts ?? []), ...(overview.codex?.accounts ?? [])].map((a) => a.id)).size}
              reloadKey={reloadKey}
              titleOf={(c) => named(c).title}
              onOpenChat={(c) => {
                if (c.provider !== provider) setProvider(c.provider);
                open(c);
              }}
              onSync={() => setSyncOpen(true)}
              onSearch={() => openSearch()}
              dark={dark}
              onTheme={() => setDark((d) => !d)}
              onRefresh={() => void rescan()}
              onShowTool={(p) => switchProvider(p)}
              onOpenBackup={() => void invoke("open_backup_folder").catch((e) => say(String(e)))}
              rowEntries={(c) => chatEntries(named(c), say, null, extras(c))}
            />
          ) : (
            <ChatView
              key={chatKey(selected)}
              chat={named(selected)}
              initialSearch={openQuery}
              page={page}
              loadingEarlier={loadingEarlier}
              live={live}
              outline={outline}
              onLoadUntil={loadUntil}
              onLoadEarlier={loadEarlier}
              onLoadAll={loadAll}
              showSteps={showSteps}
              onToggleSteps={() => setShowSteps((s) => !s)}
              headerOpen={headerOpen}
              onToggleHeader={() => setHeaderOpen((o) => !o)}
              pinned={pinSet.has(chatKey(selected))}
              onTogglePin={() => togglePin(chatKey(selected))}
              onArchive={() => void archive(selected)}
              onRename={() => setRenaming(selected)}
              onDelete={() => requestDelete(selected)}
              onSearchAll={(q) => openSearch(q)}
              onRefresh={() => { void rescan(); open(selected); }}
              onOverview={() => setSelected(null)}
              toast={say}
            />
          )}
          </Suspense>
        </main>
        </div>

        <Suspense fallback={null}>
        {paletteOpen && <CommandPalette
          open={paletteOpen}
          onOpenChange={setPaletteOpen}
          provider={provider === "all" ? "claude" : provider}
          pinned={pinned}
          titleOf={(c) => named(c).title}
          renamed={names}
          onOpenChat={(c, term) => {
            if (c.provider !== provider) setProvider(c.provider);
            open(c, term);
          }}
          actions={[
            { id: "overview", label: "Back to Home", icon: <LayoutDashboard className="size-4" />, run: () => setSelected(null) },
            { id: "claude", label: "Show Claude chats", icon: <ProviderLogo provider="claude" className="size-4" />, run: () => switchProvider("claude") },
            { id: "codex", label: "Show Codex chats", icon: <ProviderLogo provider="codex" className="size-4" />, run: () => switchProvider("codex") },
            { id: "sync", label: "Sync & backup", icon: <DatabaseBackup className="size-4" />, run: () => setSyncOpen(true) },
            { id: "settings", label: "Settings", icon: <SettingsIcon className="size-4" />, run: () => setSettingsOpen(true) },
            { id: "refresh", label: "Scan for new chats", icon: <RefreshCw className="size-4" />, run: () => void rescan() },
            { id: "theme", label: dark ? "Light theme" : "Dark theme", icon: dark ? <Sun className="size-4" /> : <Moon className="size-4" />, run: () => setDark((d) => !d) },
          ]}
        />}
        </Suspense>
        <div className={cn("fixed top-3 z-30 transition-[left] duration-300 ease-in-out", sidebarOpen ? "left-[272px]" : "left-3", splash !== "off" && "hidden")}>
          <Tip label={sidebarOpen ? "Hide the sidebar" : "Show the sidebar"}>
            <Button variant="ghost" size="icon" className="text-muted-foreground" aria-label={sidebarOpen ? "Hide the sidebar" : "Show the sidebar"} onClick={() => setSidebarOpen((o) => !o)}>
              <PanelLeft className="size-4" />
            </Button>
          </Tip>
        </div>
        <Suspense fallback={null}>
          {syncOpen && <SyncDialog open={syncOpen} onOpenChange={(o) => { setSyncOpen(o); if (!o) setSyncTab("backups"); }} onChanged={rescan} onMove={setMovePending} toast={say} initialTab={syncTab} />}
          {movePending && (
            <MoveBackupDialog
              check={movePending}
              toast={say}
              onClose={() => {
                setMovePending(null);
                setSyncOpen(true);
              }}
              onDone={() => {
                invalidateBackups({ clear: true });
                setMovePending(null);
                setSyncOpen(true);
                void rescan();
              }}
            />
          )}
          <RenameAccountDialog
            account={renamingAccount ? { id: renamingAccount, original: originalAccountLabel(renamingAccount), custom: accountNames[renamingAccount] ?? "" } : null}
            onSave={renameAccount}
            onClose={() => setRenamingAccount(null)}
          />
          {settingsOpen && (
            <SettingsDialog
              open={settingsOpen}
              onOpenChange={setSettingsOpen}
              onOpenBackup={(tab) => {
                setSyncTab(tab ?? "backups");
                setSyncOpen(true);
              }}
              onToolsChanged={async () => {
                const off = await applyOff();
                if (provider !== "all" && off.includes(provider)) setProvider("all");
                void rescan();
              }}
            />
          )}
        </Suspense>
        <DeleteDialog
          chat={deleting}
          onClose={() => setDeleting(null)}
          onConfirm={async (c) => {
            const kind = deleteKind(c);
            await invoke("delete_chat", { path: c.path });
            if (selected && selected.id === c.id && selected.provider === c.provider) setSelected(null);
            setDeleting(null);
            say(kind === "opencode" ? "Deleted from OpenCode" : "Moved to the Recycle Bin");
            setReloadKey((n) => n + 1);
          }}
        />
        <Suspense fallback={null}>
        {tagging && <TagsDialog
          chat={tagging}
          suggestions={tagList.map(([t]) => t)}
          onClose={() => setTagging(null)}
          onSaved={() => {
            loadTags();
            void quietReload();
          }}
        />}
        </Suspense>
        <RenameDialog
          chat={renaming}
          current={renaming ? (names[chatKey(renaming)] ?? "") : ""}
          onClose={() => setRenaming(null)}
          onSave={(name) => {
            if (renaming) rename(chatKey(renaming), name);
            setRenaming(null);
          }}
        />
        <AppToaster />
        <ConfirmHost />
      </div>
    </TooltipProvider>
  );
}

const groupByProject = (list: Chat[]) => {
  const map = new Map<string, Chat[]>();
  list.forEach((c) => {
    const g = map.get(c.project);
    if (g) g.push(c);
    else map.set(c.project, [c]);
  });
  return [...map.entries()];
};

const orderChat = (list: ChatAction[]) => [...list.filter((a) => !a.danger), ...list.filter((a) => a.danger)];

function Section({ title, count, open, onOpenChange, action, children }: { title: string; count: number; open: boolean; onOpenChange: (o: boolean) => void; action?: ReactNode; children: ReactNode }) {
  return (
    <Collapsible open={open} onOpenChange={onOpenChange} className="mb-1">
      <div className="flex items-center gap-1">
        <CollapsibleTrigger className="flex flex-1 items-center gap-1.5 rounded-md px-2 py-1.5 text-[13px] font-medium text-muted-foreground hover:bg-muted/60 hover:text-foreground">
          <ChevronRight className={cn("size-3.5 transition-transform", open && "rotate-90")} />
          {title}
          <span className="rounded-full bg-muted px-1.5 text-[10px] font-normal text-muted-foreground">{count.toLocaleString()}</span>
        </CollapsibleTrigger>
        {action}
      </div>
      <CollapsibleContent className="space-y-0.5">{children}</CollapsibleContent>
    </Collapsible>
  );
}

const SORTS: Record<string, string> = { recent: "Most recent", oldest: "Oldest first", title: "Title A to Z", size: "Largest first" };
const SCOPES: Record<string, string> = { all: "All", live: "On this PC", backup: "Backup only", archived: "Archived" };

function FiltersMenu({ sort, scope, organize, tags, onSort, onScope, onOrganize, onReset }: { tags: [string, number][]; sort: string; scope: string; organize: string; onSort: (s: string) => void; onScope: (s: string) => void; onOrganize: (s: string) => void; onReset: () => void }) {
  const applied = (sort !== "recent" ? 1 : 0) + (scope !== "all" ? 1 : 0);
  return (
    <DropdownMenu>
      <Tooltip>
        <TooltipTrigger
          render={
            <DropdownMenuTrigger render={<Button variant="ghost" size="icon" aria-label="Filters" className="relative size-7 shrink-0 text-muted-foreground" />}>
              <ListFilter className="size-4" />
              {applied > 0 && (
                <span className="absolute -right-0.5 -top-0.5 flex size-3.5 items-center justify-center rounded-full bg-primary text-[9px] text-primary-foreground">{applied}</span>
              )}
            </DropdownMenuTrigger>
          }
        />
        <TooltipContent>Filters</TooltipContent>
      </Tooltip>
      <DropdownMenuContent align="end" className="w-52">
        <DropdownMenuSub>
          <DropdownMenuSubTrigger>
            <LayoutDashboard className="size-4" strokeWidth={1.5} /> Organize sidebar
          </DropdownMenuSubTrigger>
          <DropdownMenuSubContent className="w-44">
            <DropdownMenuRadioGroup value={organize} onValueChange={onOrganize}>
              <DropdownMenuRadioItem value="project">
                <FolderOpen className="size-4" strokeWidth={1.5} /> By project
              </DropdownMenuRadioItem>
              <DropdownMenuRadioItem value="list">
                <ListFilter className="size-4" strokeWidth={1.5} /> In one list
              </DropdownMenuRadioItem>
            </DropdownMenuRadioGroup>
          </DropdownMenuSubContent>
        </DropdownMenuSub>
        <DropdownMenuSub>
          <DropdownMenuSubTrigger>
            <ArrowUpDown className="size-4" strokeWidth={1.5} /> Sort chats by
          </DropdownMenuSubTrigger>
          <DropdownMenuSubContent className="w-44">
            <DropdownMenuRadioGroup value={sort} onValueChange={onSort}>
              {Object.entries(SORTS).map(([k, label]) => (
                <DropdownMenuRadioItem key={k} value={k}>
                  {label}
                </DropdownMenuRadioItem>
              ))}
            </DropdownMenuRadioGroup>
          </DropdownMenuSubContent>
        </DropdownMenuSub>
        <DropdownMenuSub>
          <DropdownMenuSubTrigger>
            <Eye className="size-4" strokeWidth={1.5} /> Show
          </DropdownMenuSubTrigger>
          <DropdownMenuSubContent className="w-44">
            <DropdownMenuRadioGroup value={scope} onValueChange={onScope}>
              {Object.entries(SCOPES).map(([k, label]) => (
                <DropdownMenuRadioItem key={k} value={k}>
                  {label}
                </DropdownMenuRadioItem>
              ))}
              {tags.length > 0 && <DropdownMenuSeparator />}
              {tags.map(([t, n]) => (
                <DropdownMenuRadioItem key={t} value={`tag:${t}`}>
                  <Tag className="size-3.5" strokeWidth={1.5} /> {t} <span className="ml-auto text-xs text-muted-foreground">{n}</span>
                </DropdownMenuRadioItem>
              ))}
            </DropdownMenuRadioGroup>
          </DropdownMenuSubContent>
        </DropdownMenuSub>
        <DropdownMenuSeparator />
        <DropdownMenuItem disabled={applied === 0 && organize === "list"} onClick={onReset}>
          <RotateCcw className="size-4" strokeWidth={1.5} /> Reset
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

const ChatRow = memo(function ChatRow({
  chat,
  selected,
  pinned,
  live,
  hit,
  hitQuery,
  inProject,
  msgs,
  getExtras,
  onOpen,
  toast,
}: {
  chat: Chat;
  selected: boolean;
  pinned: boolean;
  live?: boolean;
  hit?: string;
  hitQuery?: string;
  inProject?: boolean;
  msgs: Msg[] | null;
  getExtras: (c: Chat) => ChatAction[];
  onOpen: (c: Chat) => void;
  toast: (m: string) => void;
}) {
  return (
    <ChatCard
      chat={chat}
      selected={selected}
      pinned={pinned}
      onUnpin={() => {
        const action = getExtras(chat).find((a) => a.id === "pin");
        if (action) void runAction(action.label, action.run);
      }}
      live={live}
      hit={hit}
      hitQuery={hitQuery}
      inProject={inProject}
      onOpen={() => onOpen(chat)}
      menu={<ChatMenu chat={chat} toast={toast} msgs={msgs} getExtras={() => getExtras(chat)} />}
      entries={() => chatEntries(chat, toast, msgs, getExtras(chat))}
      onError={(e) => toast(String(e))}
    />
  );
});

function Tip({ label, children }: { label: string; children: ReactNode }) {
  return (
    <Tooltip>
      <TooltipTrigger render={children as ReactElement} />
      <TooltipContent className="max-w-xs">{label}</TooltipContent>
    </Tooltip>
  );
}

function DeleteDialog({ chat, onClose, onConfirm }: { chat: Chat | null; onClose: () => void; onConfirm: (c: Chat) => Promise<void> }) {
  const [busy, setBusy] = useState(false);
  const tool = chat ? PROVIDER_NAME[chat.provider] : "";
  return (
    <AlertDialog open={!!chat} onOpenChange={(o) => !o && !busy && onClose()}>
      <AlertDialogContent className="sm:max-w-md">
        <AlertDialogHeader>
          <div className="flex items-start gap-3">
            <div className="flex size-9 shrink-0 items-center justify-center rounded-lg bg-muted">
              <Trash2 className="size-4 text-destructive" strokeWidth={1.5} />
            </div>
            <div className="min-w-0 space-y-1">
              <AlertDialogTitle className="text-base font-medium">Delete this chat?</AlertDialogTitle>
              <AlertDialogDescription>
                {!chat
                  ? null
                  : deleteKind(chat) === "opencode"
                    ? `“${chat.title}” is deleted from ${tool} for good. It cannot be restored from the Recycle Bin. A copy in your backup folder, if you made one, is kept.`
                    : `“${chat.title}” moves to the Recycle Bin and disappears from ${tool}. You can restore it from the Recycle Bin.${chat.backup ? " This is a backup copy, so nothing else is removed." : " A copy in your backup folder is kept."}`}
              </AlertDialogDescription>
            </div>
          </div>
        </AlertDialogHeader>
        <AlertDialogFooter className="grid grid-cols-2 gap-2 sm:flex">
          <AlertDialogCancel disabled={busy}>Cancel</AlertDialogCancel>
          <AlertDialogAction
            variant="destructive"
            disabled={busy}
            onClick={async (e) => {
              e.preventDefault();
              if (!chat) return;
              setBusy(true);
              try {
                await onConfirm(chat);
              } catch (err) {
                notify(String(err));
              } finally {
                setBusy(false);
              }
            }}
          >
            {busy ? <Loader2 className="size-4 animate-spin" /> : <Trash2 className="size-4" />} Delete
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}

function RenameDialog({ chat, current, onClose, onSave }: { chat: Chat | null; current: string; onClose: () => void; onSave: (name: string) => void }) {
  const [value, setValue] = useState("");
  useEffect(() => setValue(current || chat?.title || ""), [chat, current]);
  return (
    <Dialog open={!!chat} onOpenChange={(o) => !o && onClose()}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <div className="flex items-start gap-3">
            <div className="flex size-9 shrink-0 items-center justify-center rounded-lg bg-muted">
              <Pencil className="size-4 text-muted-foreground" strokeWidth={1.5} />
            </div>
            <div className="min-w-0">
              <DialogTitle className="text-base font-medium">Rename chat</DialogTitle>
              <DialogDescription>Only shown in ChatVault.</DialogDescription>
            </div>
          </div>
        </DialogHeader>
        <form
          onSubmit={(e) => {
            e.preventDefault();
            onSave(value);
          }}
          className="space-y-4"
        >
          <Input autoFocus value={value} onChange={(e) => setValue(e.target.value)} placeholder="Chat name" maxLength={120} />
          <DialogFooter className="grid grid-cols-2 gap-2 sm:flex">
            <Button type="button" variant="outline" onClick={onClose}>
              Cancel
            </Button>
            <Button type="submit">Save</Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

/** "Continue in Gemini CLI" / "Copy Codex command" under a heading that already says it: just "Gemini CLI", "Codex". */
const shortName = (a: ChatAction) => a.label.replace(/^Continue in /, "").replace(/^Copy /, "").replace(/ command$/, "");
const isToolCommand = (a: ChatAction) => a.id.startsWith("copy-") && !COPY_CONTENT_IDS.includes(a.id);

/**
 * The right-click menu of a chat (sidebar and overview). Ordered by what people do most: pick the chat up again, open it,
 * keep it tidy, move it to another tool, take a copy of it, find its folder, and last of all delete it.
 */
function chatEntries(chat: Chat, toast: (m: string) => void, msgs: Msg[] | null, extras: ChatAction[]): Entry[] {
  const all = [...extras, ...chatActions(chat, toast, msgs)];
  const byId = (id: string) => all.find((a) => a.id === id);
  const item = (a: ChatAction | undefined, label?: string): Entry[] => {
    if (!a) return [];
    return [{ type: "item", label: label ?? a.label, icon: <ActionIcon action={a} className="size-4" />, onSelect: () => runAction(a.label, a.run), danger: a.danger && !a.muted, muted: a.muted }];
  };
  const handoff = continueTargets(all);
  const commands = all.filter(isToolCommand);
  const more: Entry[] = [
    ...item(byId("copy-md"), "Markdown"),
    ...item(byId("copy-id"), "Chat ID"),
    ...item(byId("fork")),
    ...(commands.length ? ([{ type: "sep" }, { type: "label", label: "Copy the command to start" }, ...commands.flatMap((a) => item(a, shortName(a)))] as Entry[]) : []),
  ];
  const saveItems = all.filter((a) => a.group === "Save").flatMap((a) => item(a));
  // The same order as the "…" menu on the card and the right-click menu inside a chat.
  return [
    ...item(byId("view")),
    ...(handoff.length ? ([{ type: "sub", label: "Continue this chat in", icon: <ArrowRightLeft strokeWidth={1.5} />, entries: handoff.flatMap((a) => item(a, shortName(a))) }] as Entry[]) : []),
    { type: "sep" },
    ...item(byId("copy-prompt")),
    ...item(byId("copy-path")),
    ...(more.length ? ([{ type: "sub", label: "More copy options", icon: <Copy strokeWidth={1.5} />, entries: more }] as Entry[]) : []),
    ...(saveItems.length ? ([{ type: "sub", label: "Save as", icon: <Save strokeWidth={1.5} />, entries: saveItems }] as Entry[]) : []),
    { type: "sep" },
    ...["pin", "rename", "tags", "archive"].flatMap((id) => item(byId(id))),
    { type: "sep" },
    ...item(byId("folder")),
    { type: "sep" },
    ...item(byId("delete")),
  ];
}

const QUICK = ["continue-claude", "continue-codex", "fork", "folder", "copy-id"];

function ChatMenu({ chat, toast, msgs, getExtras }: { chat: Chat; toast: (m: string) => void; msgs: Msg[] | null; getExtras: () => ChatAction[] }) {
  const [open, setOpen] = useState(false);
  // Built only while the menu is open: the closed button needs none of it.
  const actions = !open ? [] : [...getExtras(), ...chatActions(chat, toast, msgs).filter((a) => QUICK.includes(a.id) || a.id.startsWith("continue-") || a.id.startsWith("copy-") || a.group === "Save")];
  useEffect(() => {
    if (!open) return;
    const close = () => setOpen(false);
    window.addEventListener("scroll", close, true);
    window.addEventListener("resize", close);
    return () => {
      window.removeEventListener("scroll", close, true);
      window.removeEventListener("resize", close);
    };
  }, [open]);
  const byId = (id: string) => actions.find((a) => a.id === id);
  // Grouped by what you want to do: open it, continue it, copy it for an AI, save it, tidy it, find its folder, delete it.
  const organise = ["pin", "rename", "tags", "archive"].map(byId).filter(Boolean) as ChatAction[];
  const view = ["view"].map(byId).filter(Boolean) as ChatAction[];
  const others = continueTargets(actions);
  const files = ["folder"].map(byId).filter(Boolean) as ChatAction[];
  const commands = actions.filter(isToolCommand);
  const copyIds = ["copy-md", "copy-id", "fork"].map(byId).filter(Boolean) as ChatAction[];
  // The two copies people reach for most sit on the menu itself, not inside "Copy".
  const quickCopies = ["copy-prompt", "copy-path"].map(byId).filter(Boolean) as ChatAction[];
  const saves = actions.filter((a) => a.group === "Save");
  // Rows that open a panel to the side, so the list itself stays short.
  const folds: { label: string; icon: LucideIcon; heading: string; items: ChatAction[]; sub?: { heading: string; items: ChatAction[] } }[] = [
    { label: "Continue this chat in", icon: ArrowRightLeft, heading: "Continue this chat in", items: others },
    { label: "More copy options", icon: Copy, heading: "Copy this chat", items: copyIds, sub: { heading: "Copy the command to start", items: commands } },
    { label: "Save as", icon: Save, heading: "Save this chat", items: saves },
  ].filter((f) => f.items.length || f.sub?.items.length);
  const sections: { items: ChatAction[]; folds?: string[] }[] = [
    { items: view },
    { items: [], folds: ["Continue this chat in"] },
    { items: quickCopies, folds: ["More copy options"] },
    { items: [], folds: ["Save as"] },
    { items: organise },
    { items: files },
    { items: actions.filter((a) => a.danger) },
  ];
  // Short names inside "More copy options": the panel title already says these are copies.
  const SHORT_COPY: Record<string, string> = { "copy-md": "Markdown", "copy-id": "Chat ID" };
  const renderItem = (a: ChatAction, short = false) => {
    return (
      <button
        key={a.id}
        className={cn("flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-sm hover:bg-muted disabled:opacity-60", a.danger && !a.muted && "text-destructive hover:bg-destructive/10", a.muted && "opacity-50")}
        onClick={() => {
          setOpen(false);
          void runAction(a.label, a.run);
        }}      >
        <ActionIcon action={a} className={cn("size-4", !a.logo && (a.danger && !a.muted ? "text-destructive" : "text-muted-foreground"))} />
        {short ? shortName(a) : SHORT_COPY[a.id] ?? a.label}
      </button>
    );
  };
  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger
        render={
          <Button
            variant="ghost"
            size="icon"
            aria-label="Chat actions"
            className={cn(
              "-my-1 size-7 shrink-0 text-muted-foreground opacity-0 transition-opacity group-hover:opacity-100 focus-visible:opacity-100",
              open && "opacity-100",
            )}
            onClick={(e) => e.stopPropagation()}
          />
        }
      >
        <MoreHorizontal className="size-4" />
      </PopoverTrigger>
      <PopoverContent side="right" align="start" sideOffset={8} className="w-64 gap-0 p-1" onClick={(e) => e.stopPropagation()}>
        {sections
          .filter((sec) => sec.items.length || folds.some((f) => sec.folds?.includes(f.label)))
          .map((sec, i) => (
            <div key={i}>
              {i > 0 && <Separator className="my-1" />}
              {sec.items.map((a) => renderItem(a))}
              {folds
                .filter((f) => sec.folds?.includes(f.label))
                .map((f) => (
                  <Popover key={f.label}>
                    <PopoverTrigger
                      openOnHover
                      delay={60}
                      render={<button className="flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-sm hover:bg-muted data-popup-open:bg-muted" />}
                    >
                      <f.icon className="size-4 text-muted-foreground" strokeWidth={1.5} />
                      <span className="flex-1">{f.label}</span>
                      <ChevronRight className="size-4 text-muted-foreground" strokeWidth={1.5} />
                    </PopoverTrigger>
                    <PopoverContent side="right" align="start" sideOffset={6} className="w-52 gap-0 p-1" onClick={(e) => e.stopPropagation()}>
                      <p className="px-2 pb-0.5 pt-1.5 text-xs text-muted-foreground">{f.heading}</p>
                      {f.items.map((a) => renderItem(a, f.label === "Continue this chat in"))}
                      {f.sub && f.sub.items.length > 0 && (
                        <>
                          <Separator className="my-1" />
                          <p className="px-2 pb-0.5 pt-1.5 text-xs text-muted-foreground">{f.sub.heading}</p>
                          {f.sub.items.map((a) => renderItem(a, true))}
                        </>
                      )}
                    </PopoverContent>
                  </Popover>
                ))}
            </div>
          ))}
      </PopoverContent>
    </Popover>
  );
}
