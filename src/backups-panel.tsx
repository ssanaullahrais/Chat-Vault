import { useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Archive, ArrowLeft, Info, SearchX, CloudDownload, ArchiveRestore, X, Layers, CloudUpload, FolderOpen, HardDrive, Loader2, MoreHorizontal, RefreshCw, Search, Trash2, TriangleAlert } from "lucide-react";
import { AlertDialog, AlertDialogAction, AlertDialogCancel, AlertDialogContent, AlertDialogDescription, AlertDialogFooter, AlertDialogHeader, AlertDialogTitle } from "@/components/ui/alert-dialog";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { Progress } from "@/components/ui/progress";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { ProviderLogo } from "@/claude-logo";
import { DataTable, type Column } from "@/components/data-table";
import { DriveLogo, describeDriveSync, type DriveStats, type DriveStatus } from "@/gdrive-panel";
import { PROVIDERS, PROVIDER_NAME, fmtDate, fmtSize, type Provider } from "@/chat-actions";

/** "just now", "5 min ago", "3 h ago", "2 days ago", then the date. */
function fmtAgo(ms: number) {
  const s = Math.max(0, (Date.now() - ms) / 1000);
  if (s < 60) return "Just now";
  if (s < 3600) return `${Math.floor(s / 60)} min ago`;
  if (s < 86400) return `${Math.floor(s / 3600)} h ago`;
  if (s < 7 * 86400) return `${Math.floor(s / 86400)} ${Math.floor(s / 86400) === 1 ? "day" : "days"} ago`;
  return new Date(ms).toLocaleDateString(undefined, { month: "short", day: "numeric", year: "numeric" });
}
import { PanelHeader } from "@/rail";
import { notifyError } from "@/toast";
import { confirmAction } from "@/components/confirm";

type Status = "synced" | "upload" | "download" | "differs" | "unchecked";
type Entry = {
  id: string;
  provider: Provider;
  key: string;
  title: string;
  project: string | null;
  lastMs: number;
  localFiles: number;
  localBytes: number;
  driveFiles: number;
  driveBytes: number;
  devices: string[];
  deviceIds: string[];
  status: Status;
  original: boolean;
  shared: boolean;
};
export type BackupSet = {
  id: string;
  kind: "local" | "drive";
  name: string;
  path: string | null;
  device: string | null;
  thisPc: boolean;
  files: number;
  bytes: number;
  chats: number;
  lastMs: number | null;
  onlyCopy: number;
  pending: number;
  restorable: number;
};
type Removed = { localFiles: number; driveFiles: number; failed: number };
type Prog = { phase: string; done: number; total: number; current: string };

const STATUS_LABEL: Record<Status, string> = {
  synced: "In sync",
  upload: "Not in Drive yet",
  download: "Only in Drive",
  differs: "Differs",
  unchecked: "This PC only",
};
const STATUS_ITEMS: Record<string, string> = { all: "Every status", synced: "In sync", upload: "Not in Drive yet", download: "Only in Drive", differs: "Differs" };

const unit = (e: Entry) => ({ provider: e.provider, key: e.key });
const plural = (n: number, one: string, many = `${one}s`) => `${n.toLocaleString()} ${n === 1 ? one : many}`;

/** The chats of one backup (this PC's folder, or one PC's backup in Google Drive): what is where, and what to do about it. */
function BackupChats({ scope, scopeName, onBack, onChanged, toast, onBusy }: { scope: BackupSet; scopeName: string; onBack: () => void; onChanged: () => void; toast: (m: string) => void; onBusy: (busy: boolean) => void }) {
  const [rows, setRows] = useState<Entry[] | null>(null);
  const [drive, setDrive] = useState<DriveStatus | null>(null);
  const [checking, setChecking] = useState(false);
  const [q, setQ] = useState("");
  const [tool, setTool] = useState("all");
  const [status, setStatus] = useState("all");
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [working, setWorking] = useState<string | null>(null);
  const [prog, setProg] = useState<Prog | null>(null);
  const [ask, setAsk] = useState<{ rows: Entry[]; local: boolean; drive: boolean } | null>(null);
  const [typed, setTyped] = useState("");
  const [note, setNote] = useState<string | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);

  const load = useCallback(async () => {
    setLoadError(null);
    try {
      const st = await invoke<DriveStatus>("gdrive_check").catch(() => null);
      setDrive(st);
      // This PC's list first, so the tab is useful at once; Google Drive's part follows.
      setRows(await invoke<Entry[]>("backup_entries", { drive: false }));
      if (st?.connected) {
        setChecking(true);
        try {
          setRows(await invoke<Entry[]>("backup_entries", { drive: true }));
        } catch (e) {
          setLoadError(String(e));
        } finally {
          setChecking(false);
        }
      }
    } catch (e) {
      setRows([]);
      setLoadError(String(e));
    }
  }, []);

  useEffect(() => {
    void load();
    const un = listen<Prog>("gdrive-progress", (e) => setProg(e.payload.phase === "Done" ? null : e.payload));
    return () => {
      void un.then((f) => f());
    };
  }, [load]);
  useEffect(() => onBusy(!!working), [working, onBusy]);

  const connected = !!drive?.connected;
  const visible = useMemo(() => {
    const needle = q.trim().toLowerCase();
    const inScope = (r: Entry) => (scope.kind === "local" ? r.localFiles > 0 : !!scope.device && r.deviceIds.includes(scope.device));
    return (rows ?? []).filter((r) => inScope(r) && (tool === "all" || r.provider === tool) && (status === "all" || r.status === status) && (!needle || `${r.title} ${r.project ?? ""} ${r.key}`.toLowerCase().includes(needle)));
  }, [rows, q, tool, status, scope]);
  // Bulk actions work on the selected rows you can see: a row hidden by the search or a filter is never touched.
  const picked = useMemo(() => visible.filter((r) => selected.has(r.id)), [visible, selected]);

  const toolItems = useMemo(() => ({ all: "Every tool", ...Object.fromEntries(PROVIDERS.map((p) => [p, PROVIDER_NAME[p]])) }), []);

  const finish = async (message: string) => {
    toast(message);
    setSelected(new Set());
    await load();
    onChanged();
  };

  const transfer = async (list: Entry[], up: boolean) => {
    if (!list.length) return;
    const what = list.length === 1 ? `“${list[0].title}”` : plural(list.length, "chat");
    const ok = await confirmAction(
      up
        ? { title: "Upload to Google Drive?", description: `${what} ${list.length === 1 ? "is" : "are"} uploaded from this PC's backup folder to your Google Drive.`, action: "Upload", icon: <CloudUpload className="size-4 text-muted-foreground" strokeWidth={1.5} /> }
        : { title: "Download from Google Drive?", description: `${what} ${list.length === 1 ? "is" : "are"} downloaded into this PC's backup folder. A file changed on both sides keeps both copies.`, action: "Download", icon: <CloudDownload className="size-4 text-muted-foreground" strokeWidth={1.5} /> },
    );
    if (!ok) return;
    setWorking(up ? "upload" : "download");
    setNote(null);
    setProg(null);
    try {
      const s = await invoke<DriveStats>("backup_transfer", { units: list.map(unit), up });
      setNote(describeDriveSync(s));
      await finish(up ? "Uploaded to Google Drive" : "Downloaded from Google Drive");
    } catch (e) {
      notifyError(e);
    } finally {
      setWorking(null);
      setProg(null);
    }
  };

  const confirmDelete = async () => {
    if (!ask) return;
    setWorking("delete");
    try {
      const r = await invoke<Removed>("backup_delete", { units: ask.rows.map(unit), local: ask.local, drive: ask.drive });
      setAsk(null);
      setTyped("");
      setNote(
        [r.localFiles ? `${plural(r.localFiles, "file")} moved to the Recycle Bin` : "", r.driveFiles ? `${plural(r.driveFiles, "file")} deleted from Google Drive` : "", r.failed ? `${plural(r.failed, "file")} could not be removed` : ""].filter(Boolean).join(". ") + ".",
      );
      await finish("Backup deleted");
    } catch (e) {
      notifyError(e);
    } finally {
      setWorking(null);
    }
  };

  const columns: Column<Entry>[] = [
    {
      id: "chat",
      header: "Chat",
      sort: (r) => r.title.toLowerCase(),
      className: "max-w-0 w-[34%]",
      cell: (r) => (
        <div className="flex min-w-0 items-center gap-2.5">
          <span className="flex size-7 shrink-0 items-center justify-center rounded-md bg-muted">
            <ProviderLogo provider={r.provider} className="size-4" />
          </span>
          <div className="min-w-0">
            <p className="truncate text-[13px] font-medium" title={r.title}>
              {r.title}
            </p>
            <p className="truncate text-xs text-muted-foreground">
              {r.shared ? "Shared files" : r.project || PROVIDER_NAME[r.provider]}
              {r.original ? "" : r.shared ? "" : " · original gone"}
            </p>
          </div>
        </div>
      ),
    },
    {
      id: "where",
      header: "Where",
      className: "whitespace-nowrap",
      cell: (r) => (
        <div className="flex flex-wrap gap-1">
          {r.localFiles > 0 && (
            <Badge variant="secondary" className="h-5 gap-1 px-1.5 text-[10px] font-normal" title={`${plural(r.localFiles, "file")} · ${fmtSize(r.localBytes)}`}>
              <HardDrive className="size-3" /> This PC
            </Badge>
          )}
          {r.driveFiles > 0 && (
            <Badge variant="secondary" className="h-5 gap-1 px-1.5 text-[10px] font-normal" title={`${plural(r.driveFiles, "file")} · ${fmtSize(r.driveBytes)}${r.devices.length ? ` · from ${r.devices.join(", ")}` : ""}`}>
              <DriveLogo className="size-3" /> Drive
            </Badge>
          )}
        </div>
      ),
    },
    {
      id: "status",
      header: "Status",
      sort: (r) => r.status,
      className: "whitespace-nowrap",
      cell: (r) => (
        <Badge variant={r.status === "synced" ? "secondary" : "outline"} className={`h-5 px-1.5 text-[10px] font-normal ${r.status === "differs" ? "text-amber-600 dark:text-amber-400" : ""}`}>
          {connected ? STATUS_LABEL[r.status] : "This PC only"}
        </Badge>
      ),
    },
    {
      id: "size",
      header: "Size",
      sort: (r) => Math.max(r.localBytes, r.driveBytes),
      className: "whitespace-nowrap tabular-nums text-xs text-muted-foreground",
      cell: (r) => fmtSize(Math.max(r.localBytes, r.driveBytes)),
    },
    {
      id: "changed",
      header: "Changed",
      sort: (r) => r.lastMs,
      className: "whitespace-nowrap text-xs text-muted-foreground",
      cell: (r) => (r.lastMs ? fmtDate(r.lastMs) : "—"),
    },
    {
      id: "actions",
      header: <span className="sr-only">Actions</span>,
      className: "w-9 pl-0",
      cell: (r) => (
        <DropdownMenu>
          <DropdownMenuTrigger render={<Button variant="ghost" size="icon" className="size-7 text-muted-foreground" aria-label={`Actions for ${r.title}`} disabled={!!working} />}>
            <MoreHorizontal className="size-4" />
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end" className="w-60">
            {connected && (r.status === "upload" || r.status === "differs") && r.localFiles > 0 && (
              <DropdownMenuItem onClick={() => void transfer([r], true)}>
                <CloudUpload /> Upload to Google Drive
              </DropdownMenuItem>
            )}
            {connected && (r.status === "download" || r.status === "differs") && r.driveFiles > 0 && (
              <DropdownMenuItem onClick={() => void transfer([r], false)}>
                <CloudDownload /> Download from Google Drive
              </DropdownMenuItem>
            )}
            <DropdownMenuItem onClick={() => void invoke("open_backup_folder").catch(notifyError)}>
              <FolderOpen /> Open the backup folder
            </DropdownMenuItem>
            <DropdownMenuSeparator />
            {r.localFiles > 0 && (
              <DropdownMenuItem variant="destructive" onClick={() => setAsk({ rows: [r], local: true, drive: false })}>
                <Trash2 /> Delete from this PC
              </DropdownMenuItem>
            )}
            {connected && r.driveFiles > 0 && (
              <DropdownMenuItem variant="destructive" onClick={() => setAsk({ rows: [r], local: false, drive: true })}>
                <Trash2 /> Delete from Google Drive
              </DropdownMenuItem>
            )}
            {connected && r.localFiles > 0 && r.driveFiles > 0 && (
              <DropdownMenuItem variant="destructive" onClick={() => setAsk({ rows: [r], local: true, drive: true })}>
                <Trash2 /> Delete everywhere
              </DropdownMenuItem>
            )}
          </DropdownMenuContent>
        </DropdownMenu>
      ),
    },
  ];

  const inScope = (rows ?? []).filter((r) => (scope.kind === "local" ? r.localFiles > 0 : !!scope.device && r.deviceIds.includes(scope.device)));
  const total = inScope.length;
  const insync = inScope.filter((r) => r.status === "synced").length;

  // What the delete box says goes, in plain words.
  const files = (list: Entry[], key: "localFiles" | "driveFiles") => list.reduce((n, r) => n + r[key], 0);
  // Chats that would be lost for good: their original is gone from their own tool and no other copy would remain.
  const lastCopy = ask ? ask.rows.filter((r) => !r.original && !r.shared && (ask.local ? 0 : r.localFiles) + (ask.drive ? 0 : r.driveFiles) === 0) : [];
  const needsWord = !!ask && ask.rows.length > 1;

  return (
    <>
      <PanelHeader
        icon={
          <button type="button" onClick={onBack} aria-label="Back to the list of backups" className="flex size-full items-center justify-center rounded-lg hover:bg-accent">
            <ArrowLeft className="size-4 text-muted-foreground" strokeWidth={1.5} />
          </button>
        }
        title={scopeName}
        hint={`${rows === null ? "Loading chats" : plural(total, "chat")}${scope.kind === "local" ? (scope.path ? ` in ${scope.path}` : "") : " in Google Drive"}${checking ? " · checking Google Drive…" : connected && rows !== null ? ` · ${insync.toLocaleString()} in sync` : ""}`}
        action={
          <Button variant="outline" size="sm" className="shrink-0" disabled={!!working || checking} onClick={() => void load()}>
            {checking ? <Loader2 className="size-4 animate-spin" /> : <RefreshCw className="size-4" />} Refresh
          </Button>
        }
      />

      {loadError && (
        <div role="alert" className="mb-3 flex items-start gap-2 rounded-lg border border-destructive/40 p-3 text-xs">
          <TriangleAlert className="mt-0.5 size-3.5 shrink-0 text-destructive" strokeWidth={1.5} />
          <p>{loadError}</p>
        </div>
      )}

      <div className="mb-3 flex flex-wrap items-center gap-2">
        <div className="relative min-w-40 flex-1">
          <Search className="pointer-events-none absolute left-2.5 top-2 size-4 text-muted-foreground" />
          <Input value={q} onChange={(e) => setQ(e.target.value)} placeholder="Search chats" className="h-8 pl-8 text-sm" aria-label="Search chats" />
        </div>
        {/* The tool filter shows each tool's logo, like the tool picker at the top of the sidebar. */}
        <Select value={tool} onValueChange={(v) => v && setTool(v)} items={toolItems}>
          <SelectTrigger className="h-8 w-44">
            {tool === "all" ? <Layers className="size-4 shrink-0 text-muted-foreground" strokeWidth={1.5} /> : <ProviderLogo provider={tool as Provider} className="size-4 shrink-0" />}
            <SelectValue />
          </SelectTrigger>
          <SelectContent align="end" alignItemWithTrigger={false} sideOffset={6}>
            {Object.entries(toolItems).map(([v, label]) => (
              <SelectItem key={v} value={v}>
                {v === "all" ? <Layers className="size-4 shrink-0 text-muted-foreground" strokeWidth={1.5} /> : <ProviderLogo provider={v as Provider} className="size-4 shrink-0" />}
                <span className="flex-1">{label}</span>
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        {connected && (
          <Select value={status} onValueChange={(v) => v && setStatus(v)} items={STATUS_ITEMS}>
            <SelectTrigger className="h-8 w-40">
              <SelectValue />
            </SelectTrigger>
            <SelectContent align="end">
              {Object.entries(STATUS_ITEMS).map(([v, label]) => (
                <SelectItem key={v} value={v}>
                  {label}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        )}
      </div>

      {picked.length > 0 && (
        <div className="mb-3 flex flex-wrap items-center gap-2 rounded-lg border bg-muted/30 px-3 py-2">
          <span className="mr-auto text-xs text-muted-foreground">{plural(picked.length, "chat")} selected</span>
          {connected && (
            <>
              <Button variant="outline" size="sm" disabled={!!working} onClick={() => void transfer(picked.filter((r) => r.localFiles > 0), true)}>
                <CloudUpload className="size-4" /> Upload
              </Button>
              <Button variant="outline" size="sm" disabled={!!working} onClick={() => void transfer(picked.filter((r) => r.driveFiles > 0), false)}>
                <CloudDownload className="size-4" /> Download
              </Button>
            </>
          )}
          <Button variant="outline" size="sm" disabled={!!working} onClick={() => setAsk({ rows: picked.filter((r) => r.localFiles > 0), local: true, drive: false })}>
            <Trash2 className="size-4" /> Delete from this PC
          </Button>
          {connected && (
            <Button variant="outline" size="sm" disabled={!!working} onClick={() => setAsk({ rows: picked.filter((r) => r.driveFiles > 0), local: false, drive: true })}>
              <Trash2 className="size-4" /> Delete from Drive
            </Button>
          )}
        </div>
      )}

      <DataTable
        rows={visible}
        columns={columns}
        rowKey={(r) => r.id}
        selectable
        selected={selected}
        onSelectedChange={setSelected}
        loading={rows === null}
        pageSize={4}
        dense
        noun="chats"
        initialSort={{ id: "changed", desc: true }}
        empty={
          total
            ? { icon: <SearchX className="size-5" strokeWidth={1.5} />, title: "No chats found", description: "Try another search, or pick a different tool or status." }
            : { icon: <Archive className="size-5" strokeWidth={1.5} />, title: "No chats yet", description: "Chats appear here once they are in this backup." }
        }
      />

      {working && (working === "upload" || working === "download") && (
        <div className="mt-3 min-w-0 space-y-1.5">
          <Progress value={prog && prog.total ? Math.round((prog.done / prog.total) * 100) : null} />
          <p className="truncate text-xs text-muted-foreground">{prog ? `${prog.phase} ${prog.done.toLocaleString()} of ${prog.total.toLocaleString()} · ${prog.current.split("/").pop()}` : "Getting ready…"}</p>
        </div>
      )}
      {note && !working && <p className="mt-3 rounded-lg border p-3 text-sm">{note}</p>}

      <AlertDialog open={!!ask} onOpenChange={(o) => !o && !working && (setAsk(null), setTyped(""))}>
        <AlertDialogContent className="sm:max-w-md">
          <AlertDialogHeader>
            <div className="flex items-start gap-3">
              <div className="flex size-9 shrink-0 items-center justify-center rounded-lg bg-muted">
                <Trash2 className="size-4 text-destructive" strokeWidth={1.5} />
              </div>
              <div className="min-w-0">
                <AlertDialogTitle className="text-base font-medium">
                  {ask && ask.rows.length === 1 ? `Delete “${ask.rows[0].title}”?` : `Delete ${ask ? plural(ask.rows.length, "chat") : ""}?`}
                </AlertDialogTitle>
                <AlertDialogDescription>
                  {ask?.local && ask.drive
                    ? `This removes ${plural(files(ask.rows, "localFiles"), "file")} from this PC's backup folder (to the Recycle Bin) and ${plural(files(ask.rows, "driveFiles"), "file")} from Google Drive, from every PC. The Drive copy cannot be brought back.`
                    : ask?.drive
                      ? `This deletes ${plural(files(ask.rows, "driveFiles"), "file")} from Google Drive, from every PC that uploaded them. It cannot be undone. The backup on this PC stays, and will not be uploaded again unless it changes.`
                      : `This moves ${plural(ask ? files(ask.rows, "localFiles") : 0, "file")} from this PC's backup folder to the Recycle Bin, so you can still bring them back from there. Google Drive keeps its copy, and it will not be downloaded again unless it changes.`}
                </AlertDialogDescription>
              </div>
            </div>
          </AlertDialogHeader>
          {lastCopy.length > 0 && (
            <div role="alert" className="flex items-start gap-2 rounded-lg border border-destructive/40 p-3 text-xs">
              <TriangleAlert className="mt-0.5 size-3.5 shrink-0 text-destructive" strokeWidth={1.5} />
              <p>
                {lastCopy.length === 1 ? "This is the only copy of this chat." : `${plural(lastCopy.length, "of these chat")} exist only in this backup.`} The chat is no longer in its own tool, so deleting this backup loses it for good.
              </p>
            </div>
          )}
          {needsWord && (
            <label className="block text-xs text-muted-foreground">
              Type <span className="font-mono font-medium text-foreground">CONFIRM</span> to delete {ask?.rows.length.toLocaleString()} chats
              <Input value={typed} onChange={(e) => setTyped(e.target.value)} className="mt-1.5 h-9 text-sm" autoFocus aria-label="Type CONFIRM" />
            </label>
          )}
          <AlertDialogFooter className="grid grid-cols-2 gap-2 sm:flex">
            <AlertDialogCancel disabled={working === "delete"}>Cancel</AlertDialogCancel>
            <AlertDialogAction
              variant="destructive"
              disabled={working === "delete" || (needsWord && typed.trim() !== "CONFIRM")}
              onClick={(e) => {
                e.preventDefault();
                void confirmDelete();
              }}
            >
              {working === "delete" && <Loader2 className="size-4 animate-spin" />} Delete
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
      <BackupsNote drive={drive} />
    </>
  );
}

type Ask = { kind: "restore" | "delete-local" | "delete-drive"; set: BackupSet };

/** The note under the Backups tab: one box, with the Google Drive hint in it while Drive is not connected. */
function BackupsNote({ drive }: { drive: DriveStatus | null }) {
  return (
    <div role="note" className="mt-5 flex items-start gap-2 rounded-lg bg-muted/40 p-3 text-xs text-muted-foreground">
      <Info className="mt-0.5 size-3.5 shrink-0" strokeWidth={1.5} />
      <p>
        Backed-up chats stay visible after you sign out or reinstall.
        {drive?.available && !drive.connected && " Connect Google Drive (the Google Drive tab) to see the backups of all your PCs here."}
      </p>
    </div>
  );
}

/** The Backups tab: one row per backup (this PC's backup folder, then each PC's backup in Google Drive). "View chats"
 *  opens the chats of that one backup. */
export function BackupsPanel({ onChanged, toast, onBusy }: { onChanged: () => void; toast: (m: string) => void; onBusy: (busy: boolean) => void }) {
  const [sets, setSets] = useState<BackupSet[] | null>(null);
  const [drive, setDrive] = useState<DriveStatus | null>(null);
  const [checking, setChecking] = useState(false);
  const [open, setOpen] = useState<BackupSet | null>(null);
  const [working, setWorking] = useState<{ id: string; what: string } | null>(null);
  const [prog, setProg] = useState<Prog | null>(null);
  const [ask, setAsk] = useState<Ask | null>(null);
  const [typed, setTyped] = useState("");
  const [note, setNote] = useState<string | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [stopping, setStopping] = useState(false);

  const load = useCallback(async () => {
    setLoadError(null);
    try {
      const st = await invoke<DriveStatus>("gdrive_check").catch(() => null);
      setDrive(st);
      // This PC's backup first, so the tab is useful at once; the Google Drive backups follow.
      setSets(await invoke<BackupSet[]>("backup_sets", { drive: false }));
      if (st?.connected) {
        setChecking(true);
        try {
          setSets(await invoke<BackupSet[]>("backup_sets", { drive: true }));
        } catch (e) {
          setLoadError(String(e));
        } finally {
          setChecking(false);
        }
      }
    } catch (e) {
      setSets([]);
      setLoadError(String(e));
    }
  }, []);

  useEffect(() => {
    void load();
    const a = listen<Prog>("gdrive-progress", (e) => setProg(e.payload.phase === "Done" ? null : e.payload));
    const b = listen<Prog>("sync-progress", (e) => setProg(e.payload.phase === "Done" ? null : e.payload));
    return () => {
      void a.then((f) => f());
      void b.then((f) => f());
    };
  }, [load]);
  useEffect(() => onBusy(!!working), [working, onBusy]);

  const driveBusy = !!drive?.busy;

  const run = async (set: BackupSet, what: string, job: () => Promise<string | null>, done: string) => {
    setWorking({ id: set.id, what });
    setNote(null);
    setProg(null);
    try {
      const n = await job();
      setNote(n);
      toast(done);
      await load();
      onChanged();
    } catch (e) {
      notifyError(e);
    } finally {
      setWorking(null);
      setProg(null);
      setStopping(false);
    }
  };

  const copied = (s: { copied: number; failed: number }, what: string, none: string) =>
    s.copied ? `${plural(s.copied, "file")} ${what}.${s.failed ? ` ${plural(s.failed, "file")} could not be copied.` : ""}` : none;

  const confirmThen = (o: Parameters<typeof confirmAction>[0], go: () => Promise<void>) =>
    void confirmAction(o).then((ok) => {
      if (ok) void go();
    });
  const backUpNow = (set: BackupSet) =>
    confirmThen({ title: "Back up now?", description: `New and changed chats are copied into ${set.path ?? "the backup folder"}. Nothing in the tools is changed.`, action: "Back up now", icon: <Archive className="size-4 text-muted-foreground" strokeWidth={1.5} /> }, () => backUpNowGo(set));
  const syncNow = (set: BackupSet) =>
    confirmThen({ title: "Sync with Google Drive now?", description: "New and changed chats on this PC are uploaded, and the other PCs' chats are downloaded into the backup folder. Syncing never deletes anything.", action: "Sync now", icon: <RefreshCw className="size-4 text-muted-foreground" strokeWidth={1.5} /> }, () => syncNowGo(set));
  const download = (set: BackupSet) =>
    confirmThen({ title: `Download ${set.name}'s backup?`, description: `${plural(set.files, "file")} from ${set.name}'s backup in Google Drive are downloaded into this PC's backup folder. A file changed on both sides keeps both copies.`, action: "Download", icon: <CloudDownload className="size-4 text-muted-foreground" strokeWidth={1.5} /> }, () => downloadGo(set));
  const backUpNowGo = (set: BackupSet) => run(set, "backup", async () => copied(await invoke("sync_chats"), "backed up", "The backup was already up to date."), "Backed up");
  const syncNowGo = (set: BackupSet) => run(set, "sync", async () => describeDriveSync(await invoke<DriveStats>("gdrive_sync", { restore: false })), "Synced with Google Drive");
  const downloadGo = (set: BackupSet) => run(set, "download", async () => describeDriveSync(await invoke<DriveStats>("gdrive_download_device", { device: set.device })), "Downloaded to this PC");
  const cancelSync = async () => {
    setStopping(true);
    await invoke("gdrive_cancel_sync").catch(notifyError);
  };

  const confirm = async () => {
    if (!ask) return;
    const { kind, set } = ask;
    setAsk(null);
    setTyped("");
    if (kind === "restore") {
      await run(set, "restore", async () => copied(await invoke("restore_chats"), "restored into the tools", "Nothing was missing: every chat in the backup is already in its tool."), "Restored");
    } else if (kind === "delete-local") {
      await run(
        set,
        "delete",
        async () => {
          const r = await invoke<Removed>("backup_delete_local_all");
          return `${plural(r.localFiles, "file")} moved to the Recycle Bin.${r.failed ? ` ${plural(r.failed, "file")} could not be removed.` : ""}`;
        },
        "Backup deleted",
      );
    } else {
      await run(set, "delete", async () => `${plural(await invoke<number>("gdrive_delete_device", { device: set.device }), "file")} deleted from Google Drive.`, "Backup deleted from Google Drive");
    }
  };

  if (open) {
    return (
      <BackupChats
        scope={open}
        scopeName={open.kind === "local" ? `${open.name} · This PC` : `${open.name} · Google Drive`}
        onBack={() => {
          setOpen(null);
          void load();
        }}
        onChanged={onChanged}
        toast={toast}
        onBusy={onBusy}
      />
    );
  }

  const statusOf = (s: BackupSet): { text: string; tone: "ok" | "todo" | "none" } => {
    if (s.kind === "local") {
      if (!s.files) return { text: "Empty", tone: "none" };
      return s.restorable ? { text: `${s.restorable.toLocaleString()} to restore`, tone: "todo" } : { text: "Up to date", tone: "ok" };
    }
    if (!s.files) return { text: "Empty", tone: "none" };
    if (s.thisPc) return s.pending ? { text: `${s.pending.toLocaleString()} to upload`, tone: "todo" } : { text: "In sync", tone: "ok" };
    return s.pending ? { text: `${s.pending.toLocaleString()} to download`, tone: "todo" } : { text: "On this PC", tone: "ok" };
  };
  const WORKING_LABEL: Record<string, string> = { backup: "Backing up…", sync: "Syncing…", download: "Downloading…", restore: "Restoring…", delete: "Deleting…" };
  const TONE = { ok: "bg-emerald-500", todo: "bg-amber-500", none: "bg-muted-foreground/40" };

  const busyHere = (s: BackupSet) => working?.id === s.id;
  const columns: Column<BackupSet>[] = [
    {
      id: "backup",
      header: "Backup",
      className: "max-w-0 w-[30%]",
      cell: (s) => (
        <button type="button" className="flex w-full min-w-0 items-center gap-2 text-left" onClick={() => setOpen(s)} disabled={!!working} title="View chats">
          {s.kind === "local" ? <HardDrive className="size-4 shrink-0 text-muted-foreground" strokeWidth={1.5} /> : <DriveLogo className="size-4 shrink-0" />}
          <span className="min-w-0 truncate text-[13px]">
            <span className="font-medium">{s.kind === "local" ? "This PC" : s.name}</span>
            <span className="text-muted-foreground" title={s.path ?? undefined}>
              {" · "}
              {s.kind === "local" ? s.path : s.thisPc ? "Google Drive, this PC" : "Google Drive"}
            </span>
          </span>
        </button>
      ),
    },
    {
      id: "size",
      header: "Size",
      className: "whitespace-nowrap text-xs tabular-nums",
      cell: (s) => (
        <span title={`${plural(s.files, "file")}`}>
          {plural(s.chats, "chat")} <span className="text-muted-foreground">· {fmtSize(s.bytes)}</span>
        </span>
      ),
    },
    {
      id: "updated",
      header: "Updated",
      className: "whitespace-nowrap text-xs text-muted-foreground",
      cell: (s) => (s.lastMs ? <span title={fmtDate(s.lastMs)}>{fmtAgo(s.lastMs)}</span> : "Never"),
    },
    {
      id: "status",
      header: "Status",
      className: "whitespace-nowrap text-xs",
      cell: (s) => {
        if (busyHere(s) && working) {
          return (
            <span className="flex items-center gap-1.5 text-muted-foreground">
              <Loader2 className="size-3 animate-spin" /> {stopping ? "Stopping…" : WORKING_LABEL[working.what]}
            </span>
          );
        }
        const st = statusOf(s);
        return (
          <span className="flex items-center gap-1.5">
            <span className={`size-1.5 shrink-0 rounded-full ${TONE[st.tone]}`} />
            {st.text}
          </span>
        );
      },
    },
    {
      id: "actions",
      header: <span className="sr-only">Actions</span>,
      className: "w-9 pl-0",
      cell: (s) =>
        busyHere(s) && (working?.what === "sync" || working?.what === "download") ? (
          <Button variant="ghost" size="icon" className="size-7 text-destructive" aria-label={working?.what === "sync" ? "Cancel sync" : "Cancel download"} disabled={stopping} onClick={() => void cancelSync()}>
            <X className="size-4" />
          </Button>
        ) : (
          <DropdownMenu>
            <DropdownMenuTrigger render={<Button variant="ghost" size="icon" className="size-7 text-muted-foreground" aria-label={`Actions for ${s.name}`} disabled={!!working} />}>
              <MoreHorizontal className="size-4" />
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end" className="w-60">
              <DropdownMenuItem onClick={() => setOpen(s)}>
                <Layers /> View chats
              </DropdownMenuItem>
              {s.kind === "local" ? (
                <>
                  <DropdownMenuItem onClick={() => void invoke("open_backup_folder").catch(notifyError)}>
                    <FolderOpen /> Open folder
                  </DropdownMenuItem>
                  <DropdownMenuItem onClick={() => void backUpNow(s)}>
                    <Archive /> Back up now
                  </DropdownMenuItem>
                  <DropdownMenuItem disabled={!s.files} onClick={() => setAsk({ kind: "restore", set: s })}>
                    <ArchiveRestore /> Restore missing chats
                  </DropdownMenuItem>
                  <DropdownMenuSeparator />
                  <DropdownMenuItem variant="destructive" disabled={!s.files} onClick={() => setAsk({ kind: "delete-local", set: s })}>
                    <Trash2 /> Delete this backup
                  </DropdownMenuItem>
                </>
              ) : (
                <>
                  {s.thisPc ? (
                    <DropdownMenuItem disabled={driveBusy} onClick={() => void syncNow(s)}>
                      <RefreshCw /> Sync now
                    </DropdownMenuItem>
                  ) : (
                    <DropdownMenuItem disabled={driveBusy || !s.files} onClick={() => void download(s)}>
                      <CloudDownload /> Download to this PC
                    </DropdownMenuItem>
                  )}
                  <DropdownMenuSeparator />
                  <DropdownMenuItem variant="destructive" disabled={driveBusy || !s.files} onClick={() => setAsk({ kind: "delete-drive", set: s })}>
                    <Trash2 /> Delete from Drive
                  </DropdownMenuItem>
                </>
              )}
            </DropdownMenuContent>
          </DropdownMenu>
        ),
    },
  ];

  const needsWord = ask?.kind === "delete-local" || ask?.kind === "delete-drive";
  const owner = ask ? (ask.set.thisPc ? "this PC" : ask.set.name) : "";

  let title = "";
  let body = "";
  if (ask?.kind === "restore") {
    title = "Restore missing chats?";
    body = ask.set.restorable
      ? `${plural(ask.set.restorable, "file")} in the backup ${ask.set.restorable === 1 ? "is" : "are"} not in the tools. They are copied back so the tools show those chats again. Nothing that is already there is overwritten.`
      : "Every chat in the backup is already in its tool, so there is nothing to copy.";
  } else if (ask?.kind === "delete-local") {
    title = "Delete the backup on this PC?";
    body = `${plural(ask.set.files, "file")} (${fmtSize(ask.set.bytes)}) in ${ask.set.path} move to the Recycle Bin, so you can still bring them back from there. The chats in the tools and the backups in Google Drive are not touched. The next backup makes a new copy of the chats that are still in the tools.`;
  } else if (ask?.kind === "delete-drive") {
    title = `Delete ${ask.set.thisPc ? "this PC's" : `${ask.set.name}'s`} backup from Google Drive?`;
    body = `${plural(ask.set.files, "file")} that ${owner} uploaded are deleted from Google Drive. This cannot be undone. The other PCs' backups and every backup folder stay as they are.${ask.set.thisPc ? " This PC does not upload those files again unless they change." : ""}`;
  }

  return (
    <>
      <PanelHeader
        icon={<Archive className="size-4 text-muted-foreground" strokeWidth={1.5} />}
        title="Backups"
        hint={checking ? "Checking Google Drive…" : "Your backups on this PC and in Google Drive."}
        action={
          <Button variant="outline" size="sm" className="shrink-0" disabled={!!working || checking} onClick={() => void load()}>
            {checking ? <Loader2 className="size-4 animate-spin" /> : <RefreshCw className="size-4" />} Refresh
          </Button>
        }
      />

      {loadError && (
        <div role="alert" className="mb-3 flex items-start gap-2 rounded-lg border border-destructive/40 p-3 text-xs">
          <TriangleAlert className="mt-0.5 size-3.5 shrink-0 text-destructive" strokeWidth={1.5} />
          <p>{loadError}</p>
        </div>
      )}

      <DataTable rows={sets ?? []} columns={columns} rowKey={(s) => s.id} loading={sets === null} pageSize={4} noun="backups" footer={false} dense empty={{ icon: <Archive className="size-5" strokeWidth={1.5} />, title: "No backups yet", description: "Use Back up now to make the first one." }} />

      {working && working.what !== "delete" && (
        <div className="mt-3 min-w-0 space-y-1.5">
          <Progress value={prog && prog.total ? Math.round((prog.done / prog.total) * 100) : null} />
          <p className="truncate text-xs text-muted-foreground">
            {prog ? `${prog.phase}${prog.total ? ` ${prog.done.toLocaleString()} of ${prog.total.toLocaleString()}` : ""}${prog.current ? ` · ${prog.current.split("/").pop()}` : ""}` : "Getting ready…"}
          </p>
        </div>
      )}
      {note && !working && <p className="mt-3 rounded-lg border p-3 text-sm">{note}</p>}

      <AlertDialog
        open={!!ask}
        onOpenChange={(o) => {
          if (!o) {
            setAsk(null);
            setTyped("");
          }
        }}
      >
        <AlertDialogContent className="sm:max-w-md">
          <AlertDialogHeader>
            <div className="flex items-start gap-3">
              <div className="flex size-9 shrink-0 items-center justify-center rounded-lg bg-muted">
                {ask?.kind === "restore" ? <ArchiveRestore className="size-4 text-muted-foreground" strokeWidth={1.5} /> : <Trash2 className="size-4 text-destructive" strokeWidth={1.5} />}
              </div>
              <div className="min-w-0">
                <AlertDialogTitle className="text-base font-medium">{title}</AlertDialogTitle>
                <AlertDialogDescription>{body}</AlertDialogDescription>
              </div>
            </div>
          </AlertDialogHeader>
          {ask && ask.kind !== "restore" && ask.set.onlyCopy > 0 && (
            <div role="alert" className="flex items-start gap-2 rounded-lg border border-destructive/40 p-3 text-xs">
              <TriangleAlert className="mt-0.5 size-3.5 shrink-0 text-destructive" strokeWidth={1.5} />
              <p>
                {ask.set.onlyCopy === 1 ? "1 chat exists only in this backup." : `${plural(ask.set.onlyCopy, "chat")} exist only in this backup.`} They are no longer in their tool or in any other backup, so deleting it loses them
                {ask.kind === "delete-local" ? " once the Recycle Bin is emptied" : " for good"}.
              </p>
            </div>
          )}
          {needsWord && (
            <label className="block text-xs text-muted-foreground">
              Type <span className="font-mono font-medium text-foreground">CONFIRM</span> to delete this backup
              <Input value={typed} onChange={(e) => setTyped(e.target.value)} className="mt-1.5 h-9 text-sm" autoFocus aria-label="Type CONFIRM" />
            </label>
          )}
          <AlertDialogFooter className="grid grid-cols-2 gap-2 sm:flex">
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction
              variant={ask?.kind === "restore" ? "default" : "destructive"}
              disabled={(needsWord && typed.trim() !== "CONFIRM") || (ask?.kind === "restore" && !ask.set.restorable)}
              onClick={(e) => {
                e.preventDefault();
                void confirm();
              }}
            >
              {ask?.kind === "restore" ? "Restore" : "Delete"}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
      <BackupsNote drive={drive} />
    </>
  );
}
