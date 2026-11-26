import { DriveJobsStatus } from "@/drive-jobs";
import { BACKUP_AUTO_ENABLED, backupChanges, invalidateBackups } from "@/backup-refresh";
import { notifyError } from "@/toast";
import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { DatabaseBackup, FileDown, Undo2, FolderInput, FolderOpen, Archive, HardDrive, Info, Loader2, RefreshCw, RotateCcw, Trash2, TriangleAlert, UploadCloud } from "lucide-react";
import { open as pickFolder } from "@tauri-apps/plugin-dialog";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Progress } from "@/components/ui/progress";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Switch } from "@/components/ui/switch";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { fmtDate, fmtSize } from "@/chat-actions";
import { startBulkExport, useBulkExport } from "@/bulk-export";
import { PanelHeader, RAIL_TAB, RailLabel } from "@/rail";
import { AUTO_SYNC_KEY } from "@/store";
import { DriveLogo, GoogleDrivePanel } from "@/gdrive-panel";
import { BackupsPanel } from "@/backups-panel";
import { confirmAction } from "@/components/confirm";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";

type Status = { path: string; exists: boolean; files: number; bytes: number; lastSyncMs: number | null };
type Stats = { copied: number; skipped: number; failed: number; bytes: number; total: number };
type Prog = { phase: string; done: number; total: number; current: string };
type Where = { path: string; isDefault: boolean; defaultPath: string; previous: string | null; available: boolean };
type Check = { path: string; filesToMove: number; bytesToMove: number; isExistingBackup: boolean };

export { AUTO_SYNC_KEY } from "@/store";

function Stat({ label, value }: { label: string; value: string }) {
  return (
    <div className="min-w-0 px-3.5 py-2.5">
      <p className="text-[11px] text-muted-foreground">{label}</p>
      <p className="mt-0.5 whitespace-nowrap text-sm font-medium">{value}</p>
    </div>
  );
}

export function SyncDialog({
  open,
  onOpenChange,
  onChanged,
  onMove,
  toast,
  initialTab = "backups",
}: {
  open: boolean;
  onOpenChange: (o: boolean) => void;
  onChanged: () => void;
  /** A new backup folder was chosen and checked; the confirm step opens in its own dialog. */
  onMove: (c: Check) => void;
  toast: (m: string) => void;
  /** The tab shown first: "drive" when opened from Settings' Google Drive card. */
  initialTab?: "backups" | "backup" | "drive";
}) {
  const [status, setStatus] = useState<Status | null>(null);
  const [busy, setBusy] = useState<"sync" | "restore" | null>(null);
  const [prog, setProg] = useState<Prog | null>(null);
  const [last, setLast] = useState<string | null>(null);
  const exp = useBulkExport();
  const [auto, setAuto] = useState(() => localStorage.getItem(AUTO_SYNC_KEY) !== "0");
  const [where, setWhere] = useState<Where | null>(null);
  const [removing, setRemoving] = useState(false);
  const [tab, setTab] = useState<string>(initialTab);
  // A Google Drive sign-in or sync in progress keeps the dialog open, like a local backup does.
  const [driveBusy, setDriveBusy] = useState(false);

  const load = useCallback(() => {
    invoke<Status>("backup_status").then(setStatus).catch(() => {});
    invoke<Where>("backup_location_info").then(setWhere).catch(() => {});
  }, []);
  useEffect(() => {
    if (!open) return;
    return backupChanges(() => {
      if (tab === "backup") load();
    });
  }, [open, tab, load]);
  useEffect(() => {
    if (open && tab === "backup") load();
  }, [open, tab, load]);
  useEffect(() => {
    const un = listen<Prog>("sync-progress", (e) => setProg(e.payload));
    return () => {
      void un.then((f) => f());
    };
  }, []);

  /** Checks a folder for the backup. Nothing changes until the choice is confirmed. */
  const propose = async (path: string) => {
    const c = await invoke<Check>("check_backup_location", { path });
    setLast(null);
    // Sync & backup closes; the confirm step opens on its own, and this one comes back when it is finished.
    onOpenChange(false);
    onMove(c);
  };
  const chooseFolder = async () => {
    try {
      const picked = await pickFolder({ directory: true, multiple: false, title: "Choose the backup folder", defaultPath: status?.path || undefined });
      if (typeof picked === "string") await propose(picked);
    } catch (e) {
      notifyError(e);
    }
  };
  const useDefaultFolder = async () => {
    if (!where) return;
    const ok = await confirmAction({
      title: "Use the default folder?",
      description: `The backup moves back to ${where.defaultPath}. In the next step you choose whether the files you have now are moved there too.`,
      action: "Continue",
      icon: <Undo2 className="size-4 text-muted-foreground" strokeWidth={1.5} />,
    });
    if (!ok) return;
    try {
      await propose(where.defaultPath);
    } catch (e) {
      notifyError(e);
    }
  };
  const removePrevious = async () => {
    const ok = await confirmAction({
      title: "Remove the old backup folder?",
      description: `${where?.previous ?? "The old folder"} moves to the Recycle Bin, so you can still bring it back from there. The backup you use now is not touched.`,
      action: "Remove",
      destructive: true,
      icon: <Trash2 className="size-4 text-destructive" strokeWidth={1.5} />,
    });
    if (!ok) return;
    setRemoving(true);
    try {
      await invoke("delete_previous_backup");
      toast("The old backup folder is in the Recycle Bin");
      load();
    } catch (e) {
      notifyError(e);
    } finally {
      setRemoving(false);
    }
  };

  const run = async (kind: "sync" | "restore") => {
    const ok = kind === "sync" || await confirmAction(
      { title: "Restore missing chats?", description: "Chats in the backup that are missing from their tools are copied back. Chats that are already there are never overwritten.", action: "Restore", icon: <RotateCcw className="size-4 text-muted-foreground" strokeWidth={1.5} /> },
    );
    if (!ok) return;
    setBusy(kind);
    setLast(null);
    setProg(null);
    try {
      const s = await invoke<Stats>(kind === "sync" ? "sync_chats" : "restore_chats");
      const message = s.copied || s.failed
        ? `${kind === "sync" ? "Copied" : "Restored"} ${s.copied} ${s.copied === 1 ? "file" : "files"} (${fmtSize(s.bytes)}).${s.failed ? ` ${s.failed} files could not be copied.` : ""}`
        : kind === "sync" ? "The backup folder is up to date." : "No missing files to restore from this PC.";
      setLast(message);
      toast(message);
      if (s.copied > 0) onChanged();
    } catch (e) {
      notifyError(e);
    } finally {
      setBusy(null);
      invalidateBackups();
    }
  };

  const pct = prog && prog.total ? Math.round((prog.done / prog.total) * 100) : 0;
  const icon = "size-4 text-muted-foreground";

  return (
    <Dialog open={open} onOpenChange={(o) => !busy && !driveBusy && onOpenChange(o)}>
      <DialogContent className="flex max-h-[min(88dvh,40rem)] flex-col gap-0 overflow-hidden p-0 sm:max-w-4xl">
        <DialogHeader className="border-b px-5 py-3.5">
          <div className="flex items-center gap-3">
            <div className="flex size-9 shrink-0 items-center justify-center rounded-lg bg-muted">
              <DatabaseBackup className={icon} strokeWidth={1.5} />
            </div>
            <div className="min-w-0">
              <DialogTitle className="text-base font-medium">Sync &amp; backup</DialogTitle>
              <DialogDescription>Keep a copy of your chats outside the tools.</DialogDescription>
            </div>
            <div className="ml-auto mr-6">
              <DriveJobsStatus onSettings={() => { if (!busy && !driveBusy) setTab("drive"); }} />
            </div>
          </div>
        </DialogHeader>

        <Tabs orientation="vertical" value={tab} onValueChange={(v) => { if (!busy && !driveBusy) setTab(String(v)); }} className="min-h-0 flex-1 flex-col gap-0 sm:flex-row">
          <ScrollArea className="shrink-0 border-b sm:w-52 sm:border-b-0 sm:border-r" viewportClassName="max-h-[min(70dvh,32rem)]">
            <TabsList variant="line" className="h-auto w-full flex-col items-stretch gap-0.5 p-2" aria-label="Sync and backup sections">
              {/* Your backups first; then where and when they are made, the copy in Google Drive, and getting chats back. */}
              <RailLabel first>Backup</RailLabel>
              <TabsTrigger value="backups" className={RAIL_TAB}>
                <Archive className="size-4 shrink-0" strokeWidth={1.5} /> Backups
              </TabsTrigger>
              <TabsTrigger value="backup" className={RAIL_TAB}>
                <HardDrive className="size-4 shrink-0" strokeWidth={1.5} /> Backup folder
              </TabsTrigger>
              <TabsTrigger value="auto" className={RAIL_TAB}>
                <RefreshCw className="size-4 shrink-0" strokeWidth={1.5} />
                <span className="min-w-0 flex-1 truncate text-left">Automatic</span>
                <span className="shrink-0 text-[10px] text-muted-foreground">{auto ? "On" : "Off"}</span>
              </TabsTrigger>
              <TabsTrigger value="drive" className={RAIL_TAB}>
                <DriveLogo className="size-4 shrink-0" /> Google Drive
              </TabsTrigger>
              <RailLabel>Get chats back</RailLabel>
              <TabsTrigger value="restore" className={RAIL_TAB}>
                <RotateCcw className="size-4 shrink-0" strokeWidth={1.5} /> Restore
              </TabsTrigger>
              <TabsTrigger value="export" className={RAIL_TAB}>
                <FileDown className="size-4 shrink-0" strokeWidth={1.5} /> Export
              </TabsTrigger>
            </TabsList>
          </ScrollArea>

          <div className="flex min-h-0 min-w-0 flex-1 flex-col">
            <ScrollArea className="min-h-0 min-w-0 flex-1" viewportClassName="max-h-[min(70dvh,32rem)]">
              <div className="p-5">
                <TabsContent value="backup">
                  <PanelHeader icon={<HardDrive className={icon} strokeWidth={1.5} />} title="Backup folder"
                    hint="Where copies of your chats are kept."
                    action={
                      <Button size="sm" disabled={!!busy} className="shrink-0" onClick={() => run("sync")}>
                        {busy === "sync" ? <Loader2 className="size-4 animate-spin" /> : <UploadCloud className="size-4" />} Back up now
                      </Button>
                    }
                  />
                  {where && !where.available && (
                    <div role="alert" className="mb-3 flex items-start gap-2 rounded-lg border border-destructive/40 p-3 text-xs">
                      <TriangleAlert className="mt-0.5 size-3.5 shrink-0 text-destructive" strokeWidth={1.5} />
                      <p>The backup folder cannot be reached right now. Is the drive connected? Backups wait until it is back, and nothing is saved anywhere else.</p>
                    </div>
                  )}
                  <div className="overflow-hidden rounded-xl border">
                    <div className="flex min-w-0 items-start gap-3 p-3.5">
                      <div className="min-w-0 flex-1">
                        <div className="flex items-center gap-2">
                          <p className="text-sm font-medium">Location</p>
                          {where && (
                            <Badge variant="secondary" className="h-4 px-1.5 text-[10px] font-normal">
                              {where.isDefault ? "Default" : "Custom"}
                            </Badge>
                          )}
                        </div>
                        <p className="mt-0.5 break-all font-mono text-xs text-muted-foreground">{status?.path || "…"}</p>
                      </div>
                      <div className="flex shrink-0 items-center gap-1">
                        {where && !where.isDefault && (
                          <Tooltip>
                            <TooltipTrigger render={<Button variant="ghost" size="icon" className="size-8 text-muted-foreground" aria-label="Use the default folder" disabled={!!busy} onClick={() => void useDefaultFolder()} />}>
                              <Undo2 className="size-4" strokeWidth={1.5} />
                            </TooltipTrigger>
                            <TooltipContent>Use the default folder</TooltipContent>
                          </Tooltip>
                        )}
                        <Button variant="ghost" size="icon" className="size-8 text-muted-foreground" aria-label="Open the backup folder" onClick={() => invoke("open_backup_folder").catch((e) => toast(String(e)))}>
                          <FolderOpen className="size-4" strokeWidth={1.5} />
                        </Button>
                        <Button variant="outline" size="sm" disabled={!!busy} onClick={() => void chooseFolder()}>
                          <FolderInput className="size-4" /> Change…
                        </Button>
                      </div>
                    </div>
                    <div className="grid grid-cols-[minmax(0,0.7fr)_minmax(0,0.8fr)_minmax(0,1.5fr)] divide-x border-t bg-muted/20">
                      <Stat label="Files" value={status ? status.files.toLocaleString() : "…"} />
                      <Stat label="Size" value={status ? fmtSize(status.bytes) : "…"} />
                      <Stat label="Last synced" value={status?.lastSyncMs ? fmtDate(status.lastSyncMs) : "Never"} />
                    </div>
                    {where?.previous && (
                      <div className="min-w-0 space-y-2 border-t p-3.5 text-xs">
                        <div>
                          <p className="font-medium">Previous backup folder</p>
                          <p className="mt-0.5 break-all font-mono text-muted-foreground">{where.previous}</p>
                          <p className="mt-1 text-muted-foreground">It is not used any more. Remove it once you have checked that the new backup is complete.</p>
                        </div>
                        <div className="flex gap-2">
                          <Button variant="outline" size="sm" onClick={() => void invoke("open_folder", { path: where.previous })}>
                            <FolderOpen className="size-4" /> Open
                          </Button>
                          <Button variant="outline" size="sm" disabled={removing} onClick={() => void removePrevious()}>
                            {removing ? <Loader2 className="size-4 animate-spin" /> : <Trash2 className="size-4" />} Move to Recycle Bin
                          </Button>
                        </div>
                      </div>
                    )}
                  </div>
                  {last && !busy && tab === "backup" && <p className="mt-3 rounded-lg border p-3 text-sm">{last}</p>}
                </TabsContent>

                <TabsContent value="backups">
                  <BackupsPanel onChanged={onChanged} toast={toast} onBusy={setDriveBusy} />
                </TabsContent>

                <TabsContent value="restore">
                  <PanelHeader icon={<RotateCcw className={icon} strokeWidth={1.5} />} title="Restore" hint="Bring back chats that are gone from their tool." />
                  <div className="rounded-xl border p-3.5">
                    <div className="flex items-center justify-between gap-6">
                      <div className="min-w-0">
                        <p className="text-sm font-medium">Restore missing chats</p>
                        <p className="text-xs text-muted-foreground">From the backup folder on this PC. Existing tool files are kept.</p>
                      </div>
                      <Button disabled={!!busy} className="shrink-0" onClick={() => run("restore")}>
                        {busy === "restore" ? <Loader2 className="size-4 animate-spin" /> : <RotateCcw className="size-4" />} Restore missing
                      </Button>
                    </div>
                  </div>
                  {last && !busy && tab === "restore" && <p className="mt-3 rounded-lg border p-3 text-sm">{last}</p>}
                </TabsContent>

                <TabsContent value="auto">
                  <PanelHeader icon={<RefreshCw className={icon} strokeWidth={1.5} />} title="Automatic backup" hint="Keep the backup up to date without asking." />
                  <div className="flex items-center gap-3 rounded-xl border p-3.5">
                    <label className="min-w-0 flex-1 cursor-pointer">
                      <p className="text-sm font-medium">Keep backing up automatically</p>
                      <p className="text-xs text-muted-foreground">Runs when ChatVault opens, and every 10 minutes while it is open.</p>
                    </label>
                    <Switch
                      checked={auto}
                      aria-label="Keep backing up automatically"
                      onCheckedChange={(v) => {
                        setAuto(v);
                        localStorage.setItem(AUTO_SYNC_KEY, v ? "1" : "0");
                        if (v) window.dispatchEvent(new Event(BACKUP_AUTO_ENABLED));
                      }}
                    />
                  </div>
                </TabsContent>

                <TabsContent value="drive">
                  <GoogleDrivePanel onChanged={onChanged} toast={toast} onBusy={setDriveBusy} />
                </TabsContent>

                <TabsContent value="export">
                  <PanelHeader icon={<FileDown className={icon} strokeWidth={1.5} />} title="Export" hint="Save your chats as plain files." />
                  <div className="rounded-xl border p-3.5">
                    <div className="flex items-center justify-between gap-3">
                      <div className="min-w-0">
                        <p className="text-sm font-medium">Export all chats as Markdown</p>
                        <p className="text-xs text-muted-foreground">One .md file per chat. Exporting again replaces them.</p>
                      </div>
                      <Button variant="outline" className="shrink-0" disabled={exp.running} onClick={() =>
                        void confirmAction({
                          title: "Export all chats?",
                          description: "Every chat is saved as a Markdown file, one file per chat. Exporting again replaces the files from the last export.",
                          action: "Export",
                          icon: <FileDown className="size-4 text-muted-foreground" strokeWidth={1.5} />,
                        }).then((ok) => {
                          if (ok) void startBulkExport();
                        })
                      }>
                        {exp.running ? <Loader2 className="size-4 animate-spin" /> : <FileDown className="size-4" />} {exp.running ? "Exporting" : "Export"}
                      </Button>
                    </div>
                    {exp.running && (
                      <div className="mt-3 space-y-1.5">
                        <Progress value={exp.total ? Math.round((exp.done / exp.total) * 100) : null} />
                        <p className="truncate text-xs text-muted-foreground">
                          {exp.total ? `${exp.done.toLocaleString()} of ${exp.total.toLocaleString()}${exp.current ? ` · ${exp.current}` : ""}` : "Getting ready…"} · you can close this window
                        </p>
                      </div>
                    )}
                  </div>
                </TabsContent>

                {/* The Backups and Google Drive tabs show this in their own note, merged with what they add. */}
                {tab !== "backups" && tab !== "drive" && (
                  <div role="note" className="mt-5 flex items-start gap-2 rounded-lg bg-muted/40 p-3 text-xs text-muted-foreground">
                    <Info className="mt-0.5 size-3.5 shrink-0" strokeWidth={1.5} />
                    <p>Backed-up chats stay visible after you sign out or reinstall.</p>
                  </div>
                )}
              </div>
            </ScrollArea>

            {busy && (
              <div className="min-w-0 space-y-1.5 border-t px-5 py-3">
                <Progress value={pct} />
                <p className="block w-full min-w-0 truncate text-xs text-muted-foreground">
                  {prog ? `${prog.phase} ${prog.done.toLocaleString()} of ${prog.total.toLocaleString()} · ${prog.current}` : "Getting ready…"}
                </p>
              </div>
            )}
          </div>
        </Tabs>
      </DialogContent>
    </Dialog>
  );
}
