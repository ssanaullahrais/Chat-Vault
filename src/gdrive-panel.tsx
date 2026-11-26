import { driveJobActive, resumeDriveJobs, startDriveJob, useDriveJobs } from "@/drive-jobs";
import { describeDriveSync, type DriveStats } from "@/drive-stats";
import { BACKUP_AUTO_ENABLED, backupChanges, invalidateBackups } from "@/backup-refresh";
import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";
import { CloudDownload, CloudUpload, Loader2, LogOut, Monitor, ShieldCheck, TriangleAlert } from "lucide-react";
import { confirmAction } from "@/components/confirm";
import { AlertDialog, AlertDialogAction, AlertDialogCancel, AlertDialogContent, AlertDialogDescription, AlertDialogFooter, AlertDialogHeader, AlertDialogTitle } from "@/components/ui/alert-dialog";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Switch } from "@/components/ui/switch";
import { fmtDate } from "@/chat-actions";
import { PanelHeader } from "@/rail";
import { notifyError } from "@/toast";
import { LegalDialog, type LegalDoc } from "@/legal";
import driveLogo from "@/assets/google-drive.png";

/** The official Google Drive logo (from Google's brand assets), never recoloured. */
export function DriveLogo({ className }: { className?: string }) {
  return <img src={driveLogo} alt="" aria-hidden="true" draggable={false} className={className} />;
}

export type DriveDevice = { id: string; name: string; lastSyncMs: number | null; files: number; thisPc: boolean };
export type DriveStatus = {
  available: boolean;
  connected: boolean;
  email: string | null;
  name: string | null;
  thisPc: string;
  autoSync: boolean;
  lastSyncMs: number | null;
  lastError: string | null;
  lastResult: DriveStats | null;
  busy: boolean;
  /** Files the running sync has uploaded so far: what cancelling it would delete again. */
  runUploaded: number;
  /** The user has cancelled the running sync and it is stopping. */
  cancelling: boolean;
  devices: DriveDevice[];
};


const SETUP_GUIDE = "https://github.com/ssanaullahrais/Chat-Vault/blob/main/docs/GOOGLE_DRIVE_SETUP.md";

/** The connect button: the app's outline button with Google's full-colour "G" (never recoloured or stretched). */
function GoogleButton({ busy, disabled, onClick }: { busy: boolean; disabled: boolean; onClick: () => void }) {
  return (
    <Button variant="outline" className="shrink-0 gap-2" disabled={disabled} onClick={onClick}>
      {busy ? (
        <Loader2 className="size-4 animate-spin" />
      ) : (
        <DriveLogo className="size-4" />
      )}
      {busy ? "Waiting for Google…" : "Connect Google Drive"}
    </Button>
  );
}

/** The Google Drive tab of Sync & backup: connect, sync, restore, the PCs that share the account, and disconnect. */
export function GoogleDrivePanel({ onChanged, toast, onBusy }: { onChanged: () => void; toast: (m: string) => void; onBusy: (busy: boolean) => void }) {
  const jobs = useDriveJobs();
  const active = jobs.some(driveJobActive);
  const syncJob = jobs.find((job) => job.request.kind === "sync" && driveJobActive(job));
  const restoreJob = jobs.find((job) => job.request.kind === "restore" && driveJobActive(job));
  const [st, setSt] = useState<DriveStatus | null>(null);
  const [working, setWorking] = useState<"connect" | "disconnect" | null>(null);
  const [confirmOut, setConfirmOut] = useState(false);
  const [legal, setLegal] = useState<LegalDoc | null>(null);
  const load = useCallback(() => {
    invoke<DriveStatus>("gdrive_status").then(setSt).catch(() => {});
  }, []);
  useEffect(() => {
    load();
    const off = backupChanges(load);
    const un = listen<{ phase: string }>("gdrive-progress", (e) => {
      if (e.payload.phase === "Done") load();
    });
    return () => {
      off();
      void un.then((f) => f());
    };
  }, [load]);
  useEffect(() => onBusy(!!working), [working, onBusy]);
  useEffect(load, [active, load]);

  const connect = async () => {
    const ok = await confirmAction({
      title: "Connect Google Drive?",
      description: "Google's sign-in opens in your browser. ChatVault asks only for its own hidden folder in your Drive, so it cannot see your other files.",
      action: "Continue",
      icon: <DriveLogo className="size-4" />,
    });
    if (!ok) return;
    setWorking("connect");
    try {
      const s = await invoke<DriveStatus>("gdrive_connect");
      setSt(s);
      toast(s.email ? `Google Drive connected (${s.email})` : "Google Drive connected");
      invalidateBackups({ clear: true });
      await resumeDriveJobs(onChanged).catch(notifyError);
      if (s.autoSync) window.dispatchEvent(new Event(BACKUP_AUTO_ENABLED));
    } catch (e) {
      notifyError(e);
    } finally {
      setWorking(null);
    }
  };

  const sync = async (restore: boolean) => {
    if (restore && !await confirmAction({
      title: "Restore missing chats from Google Drive?",
      description: "Available backups are downloaded to this PC and missing files are copied into their tools. Existing tool files are kept.",
      action: "Restore",
      icon: <CloudDownload className="size-4 text-muted-foreground" strokeWidth={1.5} />,
    })) return;
    await startDriveJob({ kind: restore ? "restore" : "sync" }, onChanged);
  };

  const disconnect = async () => {
    setWorking("disconnect");
    try {
      setSt(await invoke<DriveStatus>("gdrive_disconnect", { deleteBackup: false }));
      toast("Google Drive disconnected");
      invalidateBackups({ clear: true });
      setConfirmOut(false);
    } catch (e) {
      notifyError(e);
    } finally {
      setWorking(null);
    }
  };

  const header = <PanelHeader icon={<DriveLogo className="size-5" />} title="Google Drive" hint="Back up to your own Drive, and share it between your PCs." />;

  if (!st) {
    return (
      <>
        {header}
        <div className="flex items-center gap-2 rounded-xl border p-3.5 text-sm text-muted-foreground">
          <Loader2 className="size-4 animate-spin" /> Checking Google Drive…
        </div>
      </>
    );
  }

  if (!st.available) {
    return (
      <>
        {header}
        <div className="rounded-xl border p-3.5">
          <p className="text-sm font-medium">Not available in this copy of ChatVault</p>
          <p className="text-xs text-muted-foreground">Use the official release, or add your own Google project when building from source.</p>
          <Button variant="outline" size="sm" className="mt-3" onClick={() => void openUrl(SETUP_GUIDE)}>
            Open the setup guide
          </Button>
        </div>
      </>
    );
  }

  const busy = active || !!working || st.busy;
  const lastSync = st.lastResult;
  const upToDate = !busy && !st.lastError && !!lastSync && !lastSync.failed && !lastSync.cancelled;
  const syncMessage = !syncJob && !working && !st.lastError && lastSync && describeDriveSync(lastSync);

  return (
    <>
      {header}

      {!st.connected ? (
        <div className="rounded-xl border">
          <div className="flex flex-wrap items-center justify-between gap-x-4 gap-y-3 p-3.5">
            <div className="min-w-0">
              <p className="text-sm font-medium">Connect your Google Drive</p>
              <p className="text-xs text-muted-foreground">Sign in with Google in your browser.</p>
            </div>
            <GoogleButton busy={working === "connect"} disabled={busy} onClick={() => void connect()} />
          </div>
          {working === "connect" && (
            <div className="flex items-center justify-between gap-3 border-t bg-muted/20 px-3.5 py-2.5 text-xs">
              <span className="text-muted-foreground">Finish signing in in your browser, then come back here.</span>
              <Button variant="ghost" size="sm" onClick={() => void invoke("gdrive_cancel_connect")}>
                Cancel
              </Button>
            </div>
          )}
          <ul className="space-y-1 border-t px-3.5 py-3 text-xs text-muted-foreground">
            <li className="flex gap-2">
              <ShieldCheck className="mt-0.5 size-3.5 shrink-0" strokeWidth={1.5} />
              <span>Only ChatVault's own hidden folder. Your other Drive files stay private.</span>
            </li>
            <li className="flex gap-2">
              <CloudUpload className="mt-0.5 size-3.5 shrink-0" strokeWidth={1.5} />
              <span>Your backed-up chats, compressed, sent straight to your Drive.</span>
            </li>
            <li className="flex gap-2">
              <Monitor className="mt-0.5 size-3.5 shrink-0" strokeWidth={1.5} />
              <span>Use the same account on other PCs to share and restore chats.</span>
            </li>
          </ul>
        </div>
      ) : (
        <div className="overflow-hidden rounded-xl border">
          <div className="flex min-w-0 items-center gap-3 p-3.5">
            <div className="flex size-9 shrink-0 items-center justify-center rounded-full bg-muted text-sm font-medium uppercase">{(st.name || st.email || "G").charAt(0)}</div>
            <div className="min-w-0 flex-1">
              <div className="flex items-center gap-2">
                <p className="truncate text-sm font-medium">{st.name || st.email || "Google account"}</p>
                <Badge variant="secondary" className="h-4 shrink-0 px-1.5 text-[10px] font-normal">
                  Connected
                </Badge>
              </div>
              {st.email && st.name && <p className="truncate text-xs text-muted-foreground">{st.email}</p>}
            </div>
            <Button variant="outline" size="sm" className="shrink-0" disabled={busy} onClick={() => setConfirmOut(true)}>
              <LogOut className="size-4" /> Disconnect
            </Button>
          </div>
          <div className="grid grid-cols-2 divide-x border-t bg-muted/20">
            <div className="min-w-0 px-3.5 py-2.5">
              <p className="text-[11px] text-muted-foreground">Last synced</p>
              <p className="mt-0.5 truncate text-sm font-medium">{st.lastSyncMs ? fmtDate(st.lastSyncMs) : "Not yet"}</p>
            </div>
            <div className="min-w-0 px-3.5 py-2.5">
              <p className="text-[11px] text-muted-foreground">This PC</p>
              <p className="mt-0.5 truncate text-sm font-medium" title={st.thisPc}>{st.thisPc}</p>
            </div>
          </div>
          <div className="flex flex-wrap gap-2 border-t p-3.5">
            <Button
              variant={upToDate ? "outline" : "default"}
              className="min-w-40 flex-1"
              disabled={!!working || !!syncJob}
              title={upToDate ? "Check for new changes" : undefined}
              onClick={() => void sync(false)}
            >
              {upToDate ? (
                <span aria-hidden="true" className="size-2 rounded-full bg-emerald-500 motion-safe:animate-[pulse_3s_ease-in-out_infinite]" />
              ) : <CloudUpload className="size-4" />}
              {syncJob ? (syncJob.state === "queued" ? "Sync queued" : "Syncing in background") : upToDate ? "Up to date" : "Sync now"}
            </Button>
            <Button variant="outline" className="min-w-40 flex-1" disabled={!!working || !!restoreJob} onClick={() => void sync(true)}>
              <CloudDownload className="size-4" /> {restoreJob ? "Restore in background" : "Restore from Google Drive"}
            </Button>
          </div>
        </div>
      )}

      {st.connected && st.lastError && !working && (
        <div role="alert" className="mt-3 flex items-start gap-2 rounded-lg border border-destructive/40 p-3 text-xs">
          <TriangleAlert className="mt-0.5 size-3.5 shrink-0 text-destructive" strokeWidth={1.5} />
          <p>The last sync did not finish: {st.lastError} Your chats here are not affected, and ChatVault keeps working offline.</p>
        </div>
      )}
      {st.connected && <div role="status">
        {syncMessage && <p className="mt-3 rounded-lg border p-3 text-sm">{syncMessage}</p>}
      </div>}

      {st.connected && (
        <>
          <div className="mt-3 flex items-center gap-3 rounded-xl border p-3.5">
            <label className="min-w-0 flex-1 cursor-pointer">
              <p className="text-sm font-medium">Sync automatically</p>
              <p className="text-xs text-muted-foreground">When ChatVault opens, and every 10 minutes while it is open.</p>
            </label>
            <Switch
              checked={st.autoSync}
              aria-label="Sync with Google Drive automatically"
              onCheckedChange={(v) => {
                setSt({ ...st, autoSync: v });
                void invoke("gdrive_set_auto", { on: v }).then(() => {
                  if (v) window.dispatchEvent(new Event(BACKUP_AUTO_ENABLED));
                }).catch((e) => {
                  load();
                  notifyError(e);
                });
              }}
            />
          </div>

          {st.devices.length > 0 && (
            <div className="mt-3 overflow-hidden rounded-xl border">
              <p className="border-b px-3.5 py-2.5 text-xs font-medium text-muted-foreground">PCs using this Google Drive</p>
              <ul className="divide-y">
                {st.devices.map((d) => (
                  <li key={d.id} className="flex min-w-0 items-center gap-3 px-3.5 py-2.5">
                    <Monitor className="size-4 shrink-0 text-muted-foreground" strokeWidth={1.5} />
                    <span className="min-w-0 flex-1 truncate text-sm">{d.name}</span>
                    {d.thisPc && (
                      <Badge variant="secondary" className="h-4 shrink-0 px-1.5 text-[10px] font-normal">
                        This PC
                      </Badge>
                    )}
                    <span className="shrink-0 text-xs text-muted-foreground">{d.lastSyncMs ? fmtDate(d.lastSyncMs) : "Never"}</span>
                  </li>
                ))}
              </ul>
            </div>
          )}
        </>
      )}

      {/* One note for the tab (the dialog's general note is not shown here): what syncing promises. */}
      <div role="note" className="mt-5 flex items-start gap-2 rounded-lg bg-muted/40 p-3 text-xs text-muted-foreground">
        <ShieldCheck className="mt-0.5 size-3.5 shrink-0" strokeWidth={1.5} />
        <p>
          Syncing never deletes anything. If a chat changed on two PCs, both copies are kept, and backed-up chats stay visible after you sign out or reinstall. See the{" "}
          <button type="button" className="cursor-pointer underline underline-offset-4 hover:text-foreground" onClick={() => setLegal("privacy")}>
            privacy policy
          </button>
          .
        </p>
      </div>

      <LegalDialog doc={legal} onDoc={setLegal} onClose={() => setLegal(null)} />

      <AlertDialog open={confirmOut} onOpenChange={(o) => !working && setConfirmOut(o)}>
        <AlertDialogContent className="sm:max-w-md">
          <AlertDialogHeader>
            <div className="flex items-start gap-3">
              <div className="flex size-9 shrink-0 items-center justify-center rounded-lg bg-muted">
                <LogOut className="size-4 text-destructive" strokeWidth={1.5} />
              </div>
              <div className="min-w-0">
                <AlertDialogTitle className="text-base font-medium">Disconnect Google Drive?</AlertDialogTitle>
                <AlertDialogDescription>ChatVault stops syncing on this PC. Your backup stays in Google Drive, and your chats and backup folder here are not touched. Connect again at any time to carry on.</AlertDialogDescription>
              </div>
            </div>
          </AlertDialogHeader>
          <AlertDialogFooter className="grid grid-cols-2 gap-2 sm:flex">
            <AlertDialogCancel disabled={!!working}>Cancel</AlertDialogCancel>
            <AlertDialogAction
              variant="destructive"
              disabled={!!working}
              onClick={(e) => {
                e.preventDefault();
                void disconnect();
              }}
            >
              {working === "disconnect" && <Loader2 className="size-4 animate-spin" />} Disconnect
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </>
  );
}
