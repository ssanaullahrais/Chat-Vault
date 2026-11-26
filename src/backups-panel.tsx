import { useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Archive, ChevronDown, SearchX, CloudDownload, Layers, CloudUpload, FolderOpen, HardDrive, Loader2, MoreHorizontal, RefreshCw, Search, Trash2, TriangleAlert } from "lucide-react";
import { AlertDialog, AlertDialogAction, AlertDialogCancel, AlertDialogContent, AlertDialogDescription, AlertDialogFooter, AlertDialogHeader, AlertDialogTitle } from "@/components/ui/alert-dialog";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { Progress } from "@/components/ui/progress";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { ProviderLogo } from "@/claude-logo";
import { DataTable, type Column } from "@/components/data-table";
import { DriveLogo } from "@/gdrive-panel";
import { describeDriveSync, type DriveStats } from "@/drive-stats";
import { PROVIDERS, PROVIDER_NAME, fmtDate, fmtSize, type Provider } from "@/chat-actions";

import { backupChanges, invalidateBackups, readBackupEntries, useBackupSnapshot } from "@/backup-refresh";
import { PanelHeader } from "@/rail";
import { notifyError } from "@/toast";
import { confirmAction } from "@/components/confirm";
import { driveJobActive, startDriveJob, useDriveJobs, type Removed } from "@/drive-jobs";

type Status = "synced" | "upload" | "download" | "differs" | "unchecked";
export type Entry = {
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
  status: Status;
  original: boolean;
  shared: boolean;
};
type Prog = { phase: string; done: number; total: number; current: string };

const STATUS_LABEL: Record<Status, string> = {
  synced: "Up to date",
  upload: "Upload available",
  download: "Download available",
  differs: "Versions differ",
  unchecked: "—",
};
const STATUS_ITEMS: Record<string, string> = { all: "Every status", synced: "Up to date", upload: "Upload available", download: "Download available", differs: "Versions differ" };

const unit = (e: Entry) => ({ provider: e.provider, key: e.key });
const plural = (n: number, one: string, many = `${one}s`) => `${n.toLocaleString()} ${n === 1 ? one : many}`;

export function BackupsPanel({ onChanged, toast, onBusy }: { onChanged: () => void; toast: (m: string) => void; onBusy: (busy: boolean) => void }) {
  const { rows, drive, error: loadError, pending } = useBackupSnapshot();
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
  const jobs = useDriveJobs();
  const driveActive = jobs.some(driveJobActive);
  const jobByEntry = useMemo(() => {
    const index = new Map<string, { label: string; queued: boolean }>();
    const activeJobs = jobs.filter(driveJobActive);
    for (const job of [...activeJobs.filter((item) => item.state !== "queued"), ...activeJobs.filter((item) => item.state === "queued")]) {
      if (!("units" in job.request)) continue;
      const queued = job.state === "queued";
      const upload = job.request.kind === "upload";
      const label = queued ? (upload ? "Upload queued" : "Delete queued")
        : job.state === "stopping" ? "Stopping…" : upload ? "Uploading…" : "Deleting…";
      for (const unit of job.request.units) {
        const key = `${unit.provider}:${unit.key}`;
        if (!index.has(key)) index.set(key, { label, queued });
      }
    }
    return index;
  }, [jobs]);
  const actionsBusy = !!working || pending || !!loadError;
  const load = useCallback(async (refresh = false) => {
    if (refresh) setChecking(true);
    try {
      await readBackupEntries(refresh);
    } finally {
      if (refresh) setChecking(false);
    }
  }, []);

  useEffect(() => {
    void load();
    const off = backupChanges(() => void load());
    const un = listen<Prog>("gdrive-progress", (e) => setProg(e.payload.phase === "Done" ? null : e.payload));
    return () => {
      off();
      void un.then((f) => f());
    };
  }, [load]);
  useEffect(() => {
    onBusy(!!working);
    return () => onBusy(false);
  }, [working, onBusy]);

  const connected = !!drive?.connected;
  const visible = useMemo(() => {
    const needle = q.trim().toLowerCase();
    return (rows ?? []).filter((r) => (tool === "all" || r.provider === tool) && (status === "all" || r.status === status) && (!needle || `${r.title} ${r.project ?? ""} ${r.key}`.toLowerCase().includes(needle)));
  }, [rows, q, tool, status]);
  const picked = useMemo(() => visible.filter((r) => selected.has(r.id)), [visible, selected]);

  const toolItems = useMemo(() => ({ all: "Every tool", ...Object.fromEntries(PROVIDERS.map((p) => [p, PROVIDER_NAME[p]])) }), []);

  const finish = (message: string) => {
    toast(message);
    setSelected(new Set());
    onChanged();
  };

  const transfer = async (list: Entry[], up: boolean) => {
    if (!list.length || actionsBusy || (!up && driveActive)) return;
    const what = list.length === 1 ? `“${list[0].title}”` : plural(list.length, "chat");
    const ok = await confirmAction(
      up
        ? { title: "Upload to Google Drive?", description: `${what} ${list.length === 1 ? "is" : "are"} uploaded from this PC's backup folder to your Google Drive.`, action: "Upload", icon: <CloudUpload className="size-4 text-muted-foreground" strokeWidth={1.5} /> }
        : { title: "Download from Google Drive?", description: `${what} ${list.length === 1 ? "is" : "are"} downloaded into this PC's backup folder. A file changed on both sides keeps both copies.`, action: "Download", icon: <CloudDownload className="size-4 text-muted-foreground" strokeWidth={1.5} /> },
    );
    if (!ok) return;
    if (up) {
      if (await startDriveJob({ kind: "upload", units: list.map(unit) }, onChanged)) {
        setNote(null);
        setSelected(new Set());
      }
      return;
    }
    setWorking("download");
    setNote(null);
    setProg(null);
    try {
      const s = await invoke<DriveStats>("backup_transfer", { units: list.map(unit), up });
      setNote(describeDriveSync(s));
      finish(describeDriveSync(s));
    } catch (e) {
      notifyError(e);
    } finally {
      setWorking(null);
      setProg(null);
      invalidateBackups();
    }
  };

  const confirmDelete = async () => {
    if (!ask?.rows.length || actionsBusy) return;
    const units = ask.rows.map(unit);
    if (ask.drive) {
      if (!await startDriveJob({ kind: "delete", units, local: ask.local }, onChanged)) return;
      setAsk(null);
      setTyped("");
      setNote(null);
      setSelected(new Set());
      return;
    }

    if (driveActive) return;
    setWorking("delete");
    setNote(null);
    try {
      const r = await invoke<Removed>("backup_delete", { units, local: true, drive: false });
      setAsk(null);
      setTyped("");
      const moved = r.localFiles ? `${plural(r.localFiles, "file")} moved to the Recycle Bin` : "";
      const failed = r.failed ? `${plural(r.failed, "file")} could not be removed` : "";
      finish([moved, failed].filter(Boolean).join(". ") || "The selected copies were already gone");
    } catch (e) {
      notifyError(e);
    } finally {
      setWorking(null);
      invalidateBackups();
    }
  };

  const columns: Column<Entry>[] = [
    {
      id: "chat",
      header: "Chat",
      sort: (r) => r.title.toLowerCase(),
      className: "max-w-0 w-[46%]",
      cell: (r) => (
        <div className="flex min-w-0 items-center gap-2.5">
          <span className="flex size-7 shrink-0 items-center justify-center rounded-md bg-muted">
            <ProviderLogo provider={r.provider} className="size-4" />
          </span>
          <div className="min-w-0">
            <p className="truncate text-[13px] font-medium" title={r.title}>
              {r.title}
            </p>
            <p className="truncate text-xs text-muted-foreground" title={r.lastMs ? `Changed ${fmtDate(r.lastMs)}` : undefined}>
              {r.shared ? "Shared files" : r.project || PROVIDER_NAME[r.provider]}
              {r.original ? "" : r.shared ? "" : " · Missing from tool"}
            </p>
          </div>
        </div>
      ),
    },
    {
      id: "where",
      header: "Stored in",
      className: "whitespace-nowrap",
      cell: (r) => (
        <div className="inline-flex flex-nowrap items-center gap-1">
          {r.localFiles > 0 && (
            <Badge variant="secondary" className="h-5 shrink-0 gap-1 whitespace-nowrap px-1.5 text-[10px] font-normal" title={`${plural(r.localFiles, "file")} · ${fmtSize(r.localBytes)}`}>
              <HardDrive className="size-3" /> This PC
            </Badge>
          )}
          {r.driveFiles > 0 && (
            <Badge variant="secondary" className="h-5 shrink-0 gap-1 whitespace-nowrap px-1.5 text-[10px] font-normal" title={`${plural(r.driveFiles, "file")} · ${fmtSize(r.driveBytes)}${r.devices.length ? ` · from ${r.devices.join(", ")}` : ""}`}>
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
      cell: (r) => {
        const job = jobByEntry.get(r.id);
        if (job) {
          return (
            <Badge variant="outline" className="h-5 gap-1 px-1.5 text-[10px] font-normal text-amber-700 dark:text-amber-400">
              {!job.queued && <Loader2 aria-hidden="true" className="size-3 motion-safe:animate-spin" />}
              {job.label}
            </Badge>
          );
        }
        return (
          <Badge variant={r.status === "synced" ? "secondary" : "outline"} className={`h-5 px-1.5 text-[10px] font-normal ${r.status === "differs" ? "text-amber-600 dark:text-amber-400" : ""}`}>
            {pending ? "Checking…" : connected ? STATUS_LABEL[r.status] : "—"}
          </Badge>
        );
      },
    },
    {
      id: "actions",
      header: <span className="sr-only">Actions</span>,
      className: "w-9 pl-0",
      cell: (r) => (
        <DropdownMenu>
          <DropdownMenuTrigger render={<Button variant="ghost" size="icon" className="size-7 text-muted-foreground" aria-label={`Actions for ${r.title}`} disabled={actionsBusy} />}>
            <MoreHorizontal className="size-4" />
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end" className="w-60">
            {connected && (r.status === "upload" || r.status === "differs") && r.localFiles > 0 && (
              <DropdownMenuItem onClick={() => void transfer([r], true)}>
                <CloudUpload /> Upload to Drive
              </DropdownMenuItem>
            )}
            {connected && !driveActive && (r.status === "download" || r.status === "differs") && r.driveFiles > 0 && (
              <DropdownMenuItem onClick={() => void transfer([r], false)}>
                <CloudDownload /> Download from Drive
              </DropdownMenuItem>
            )}
            <DropdownMenuItem onClick={() => void invoke("open_backup_folder").catch(notifyError)}>
              <FolderOpen /> Open folder
            </DropdownMenuItem>
            <DropdownMenuSeparator />
            {r.localFiles > 0 && (
              <DropdownMenuItem variant="destructive" disabled={driveActive} onClick={() => setAsk({ rows: [r], local: true, drive: false })}>
                <Trash2 /> Delete on PC
              </DropdownMenuItem>
            )}
            {connected && r.driveFiles > 0 && (
              <DropdownMenuItem variant="destructive" onClick={() => setAsk({ rows: [r], local: false, drive: true })}>
                <Trash2 /> Delete on Drive
              </DropdownMenuItem>
            )}
            {connected && r.localFiles > 0 && r.driveFiles > 0 && (
              <DropdownMenuItem variant="destructive" onClick={() => setAsk({ rows: [r], local: true, drive: true })}>
                <Trash2 /> Delete both
              </DropdownMenuItem>
            )}
          </DropdownMenuContent>
        </DropdownMenu>
      ),
    },
  ];

  const total = rows?.filter((r) => !r.shared).length ?? 0;

  // What the delete box says goes, in plain words.
  const files = (list: Entry[], key: "localFiles" | "driveFiles") => list.reduce((n, r) => n + r[key], 0);
  // Chats that would be lost for good: their original is gone from their own tool and no other copy would remain.
  const lastCopy = ask ? ask.rows.filter((r) => !r.original && !r.shared && (ask.local ? 0 : r.localFiles) + (ask.drive ? 0 : r.driveFiles) === 0) : [];
  const needsWord = !!ask && ask.rows.length > 1;

  return (
    <>
      <PanelHeader
        icon={<Archive className="size-4 text-muted-foreground" strokeWidth={1.5} />}
        title="Backups"
        hint={rows
          ? pending ? (drive?.connected ? "Checking PC and Drive…" : "Checking this PC…") : `${plural(total, "chat")} backed up`
          : loadError ? "Backup list unavailable" : "Loading backups…"}
        action={
          <Button variant="outline" size="sm" className="shrink-0" disabled={!!working || checking || pending} onClick={() => void load(true)}>
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
          <span className="mr-auto text-xs text-muted-foreground">{plural(picked.length, "entry", "entries")} selected</span>
          {connected && (
            <>
              <Button variant="outline" size="sm" disabled={actionsBusy || !picked.some((r) => r.localFiles)} onClick={() => void transfer(picked.filter((r) => r.localFiles > 0), true)}>
                <CloudUpload className="size-4" /> Upload
              </Button>
              <Button variant="outline" size="sm" disabled={actionsBusy || driveActive || !picked.some((r) => r.driveFiles)} onClick={() => void transfer(picked.filter((r) => r.driveFiles > 0), false)}>
                <CloudDownload className="size-4" /> Download
              </Button>
            </>
          )}
          <DropdownMenu>
            <DropdownMenuTrigger render={<Button variant="outline" size="sm" disabled={actionsBusy} />}>
              <Trash2 className="size-4" /> Delete <ChevronDown className="size-3" />
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end" className="w-36">
              <DropdownMenuItem variant="destructive" disabled={driveActive || !picked.some((r) => r.localFiles)} onClick={() => setAsk({ rows: picked.filter((r) => r.localFiles > 0), local: true, drive: false })}>
                <HardDrive /> On PC
              </DropdownMenuItem>
              {connected && (
                <DropdownMenuItem variant="destructive" disabled={!picked.some((r) => r.driveFiles)} onClick={() => setAsk({ rows: picked.filter((r) => r.driveFiles > 0), local: false, drive: true })}>
                  <DriveLogo className="size-4" /> On Drive
                </DropdownMenuItem>
              )}
            </DropdownMenuContent>
          </DropdownMenu>
        </div>
      )}

      <DataTable
        rows={visible}
        columns={columns}
        rowKey={(r) => r.id}
        selectable
        selected={selected}
        onSelectedChange={setSelected}
        loading={rows === null && !loadError}
        pageSize={4}
        dense
        noun="entries"
        empty={
          rows?.length
            ? { icon: <SearchX className="size-5" strokeWidth={1.5} />, title: "No chats found", description: "Try another search, or pick a different tool or status." }
            : { icon: <Archive className="size-5" strokeWidth={1.5} />, title: "No chats yet", description: "Use Backup folder to make your first backup." }
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
                    ? `This removes ${plural(files(ask.rows, "localFiles"), "file")} from this PC's backup folder (to the Recycle Bin) and ${plural(files(ask.rows, "driveFiles"), "file")} from Google Drive, from every PC. The Drive copy is deleted permanently. Automatic backup can recreate the copies if the chat is still in its tool.`
                    : ask?.drive
                      ? `This deletes ${plural(files(ask.rows, "driveFiles"), "file")} from Google Drive, from every PC that uploaded them. It cannot be undone. If a copy is still available on this PC, the next sync uploads it again.`
                      : `This moves ${plural(ask ? files(ask.rows, "localFiles") : 0, "file")} from this PC's backup folder to the Recycle Bin, so you can still bring them back from there. The tools and Google Drive keep their copies. Automatic backup can recreate the copy from its tool.`}
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
              disabled={actionsBusy || (needsWord && typed.trim() !== "CONFIRM")}
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

    </>
  );
}
