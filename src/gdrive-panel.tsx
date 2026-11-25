import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";
import { CloudDownload, CloudUpload, Loader2, LogOut, Monitor, ShieldCheck, TriangleAlert } from "lucide-react";
import { confirmAction } from "@/components/confirm";
import { AlertDialog, AlertDialogAction, AlertDialogCancel, AlertDialogContent, AlertDialogDescription, AlertDialogFooter, AlertDialogHeader, AlertDialogTitle } from "@/components/ui/alert-dialog";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Progress } from "@/components/ui/progress";
import { Switch } from "@/components/ui/switch";
import { fmtDate, fmtSize } from "@/chat-actions";
import { PanelHeader } from "@/rail";
import { notifyError } from "@/toast";
import { LegalDialog, type LegalDoc } from "@/legal";
import driveLogo from "@/assets/google-drive.png";

/** The official Google Drive logo (from Google's brand assets), never recoloured. */
export function DriveLogo({ className }: { className?: string }) {
  return <img src={driveLogo} alt="" aria-hidden="true" draggable={false} className={className} />;
}

export type DriveStats = { uploaded: number; downloaded: number; conflicts: number; failed: number; unchanged: number; bytesUp: number; bytesDown: number; restored: number; cancelled: boolean };
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
type Prog = { phase: string; done: number; total: number; current: string };

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

const plural = (n: number) => `${n.toLocaleString()} ${n === 1 ? "file" : "files"}`;

/** One line about what the last sync did. */
export function describeDriveSync(s: DriveStats) {
  if (s.cancelled) {
    return s.uploaded ? `Sync paused after ${plural(s.uploaded)}. The next sync carries on from there.` : "Sync paused. The next sync carries on from there.";
  }
  const parts: string[] = [];
  if (s.uploaded) parts.push(`uploaded ${s.uploaded.toLocaleString()} ${s.uploaded === 1 ? "file" : "files"} (${fmtSize(s.bytesUp)})`);
  if (s.downloaded) parts.push(`downloaded ${s.downloaded.toLocaleString()} (${fmtSize(s.bytesDown)})`);
  if (s.restored) parts.push(`restored ${s.restored.toLocaleString()} into the tools`);
  if (!parts.length) parts.push("everything was already up to date");
  let text = parts.join(", ");
  text = text.charAt(0).toUpperCase() + text.slice(1) + ".";
  if (s.conflicts) text += ` ${s.conflicts} ${s.conflicts === 1 ? "chat was" : "chats were"} changed on two PCs, so both copies were kept.`;
  if (s.failed) text += ` ${s.failed} could not be synced and will be tried again next time.`;
  return text;
}

/** The Google Drive tab of Sync & backup: connect, sync, restore, the PCs that share the account, and disconnect. */
export function GoogleDrivePanel({ onChanged, toast, onBusy }: { onChanged: () => void; toast: (m: string) => void; onBusy: (busy: boolean) => void }) {
  const [st, setSt] = useState<DriveStatus | null>(null);
  const [working, setWorking] = useState<"connect" | "sync" | "restore" | "disconnect" | null>(null);
  const [prog, setProg] = useState<Prog | null>(null);
  const [confirmOut, setConfirmOut] = useState(false);
  const [result, setResult] = useState<string | null>(null);
  const [legal, setLegal] = useState<LegalDoc | null>(null);
  const [confirmCancel, setConfirmCancel] = useState(false);
  const [cancelling, setCancelling] = useState(false);
  // From the moment you confirm until the sync has really stopped, the button says so (stopping can take a moment: a
  // piece being sent has to finish first).
  const [stopRequested, setStopRequested] = useState(false);

  const load = useCallback(() => {
    invoke<DriveStatus>("gdrive_status").then(setSt).catch(() => {});
  }, []);
  useEffect(() => {
    load();
    // Then ask Google whether the connection still works (the access may have been removed in the Google account).
    invoke<DriveStatus>("gdrive_check").then(setSt).catch(() => {});
    const un = listen<Prog>("gdrive-progress", (e) => setProg(e.payload.phase === "Done" ? null : e.payload));
    return () => {
      void un.then((f) => f());
    };
  }, [load]);
  useEffect(() => onBusy(!!working), [working, onBusy]);
  // While a sync runs (started here or by the automatic sync), keep the count of files it has uploaded fresh: that is what
  // cancelling would delete again.
  const running = !!st && (working === "sync" || working === "restore" || st.busy);
  useEffect(() => {
    if (!running) setStopRequested(false);
  }, [running]);
  useEffect(() => {
    if (!running) return;
    const id = window.setInterval(load, 1500);
    return () => window.clearInterval(id);
  }, [running, load]);

  const cancelSync = async () => {
    setCancelling(true);
    try {
      await invoke("gdrive_cancel_sync");
      setStopRequested(true);
      setConfirmCancel(false);
      load();
    } catch (e) {
      notifyError(e);
    } finally {
      setCancelling(false);
    }
  };

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
    } catch (e) {
      notifyError(e);
    } finally {
      setWorking(null);
    }
  };

  const sync = async (restore: boolean) => {
    const ok = await confirmAction(
      restore
        ? { title: "Restore from Google Drive?", description: "The chats from your other PCs are downloaded into this PC's backup folder and copied into the tools. Nothing that is already here is overwritten.", action: "Restore", icon: <CloudDownload className="size-4 text-muted-foreground" strokeWidth={1.5} /> }
        : { title: "Sync with Google Drive now?", description: "New and changed chats on this PC are uploaded, and the other PCs' chats are downloaded into the backup folder. Syncing never deletes anything.", action: "Sync now", icon: <CloudUpload className="size-4 text-muted-foreground" strokeWidth={1.5} /> },
    );
    if (!ok) return;
    setWorking(restore ? "restore" : "sync");
    setResult(null);
    setProg(null);
    try {
      const s = await invoke<DriveStats>("gdrive_sync", { restore });
      setResult(describeDriveSync(s));
      toast(s.cancelled ? (restore ? "Restore stopped" : "Sync paused") : restore ? "Restored from Google Drive" : "Synced with Google Drive");
      onChanged();
    } catch (e) {
      notifyError(e);
    } finally {
      setWorking(null);
      setProg(null);
      load();
    }
  };

  const disconnect = async () => {
    setWorking("disconnect");
    try {
      setSt(await invoke<DriveStatus>("gdrive_disconnect", { deleteBackup: false }));
      toast("Google Drive disconnected");
      setConfirmOut(false);
      setResult(null);
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

  const busy = !!working || st.busy;
  const pct = prog && prog.total ? Math.round((prog.done / prog.total) * 100) : null;

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
            {/* While a sync runs the same button cancels it, after you confirm. */}
            {running && working !== "restore" ? (
              <Button variant="destructive" className="min-w-40 flex-1" disabled={stopRequested || st.cancelling} onClick={() => { load(); setConfirmCancel(true); }}>
                <Loader2 className="size-4 animate-spin" /> {stopRequested || st.cancelling ? "Cancelling…" : "Cancel sync"}
              </Button>
            ) : (
              <Button className="min-w-40 flex-1" disabled={busy} onClick={() => void sync(false)}>
                <CloudUpload className="size-4" /> Sync now
              </Button>
            )}
            {/* A restore never overwrites anything, so stopping it needs no confirmation: what it copied stays. */}
            {working === "restore" ? (
              <Button variant="destructive" className="min-w-40 flex-1" disabled={stopRequested || st.cancelling || cancelling} onClick={() => void cancelSync()}>
                <Loader2 className="size-4 animate-spin" /> {stopRequested || st.cancelling ? "Cancelling…" : "Cancel restore"}
              </Button>
            ) : (
              <Button variant="outline" className="min-w-40 flex-1" disabled={busy} onClick={() => void sync(true)}>
                <CloudDownload className="size-4" /> Restore from Google Drive
              </Button>
            )}
          </div>
          {(working === "sync" || working === "restore" || st.busy) && (
            <div className="min-w-0 space-y-1.5 border-t px-3.5 py-3">
              <Progress value={pct} />
              <p className="truncate text-xs text-muted-foreground">
                {prog ? `${prog.phase}${prog.total ? ` ${prog.done.toLocaleString()} of ${prog.total.toLocaleString()}` : "…"}${prog.current ? ` · ${prog.current.split("/").pop()}` : ""}` : "Getting ready…"}
              </p>
            </div>
          )}
        </div>
      )}

      {st.connected && st.lastError && !working && (
        <div role="alert" className="mt-3 flex items-start gap-2 rounded-lg border border-destructive/40 p-3 text-xs">
          <TriangleAlert className="mt-0.5 size-3.5 shrink-0 text-destructive" strokeWidth={1.5} />
          <p>The last sync did not finish: {st.lastError} Your chats here are not affected, and ChatVault keeps working offline.</p>
        </div>
      )}
      {result && !working && <p className="mt-3 rounded-lg border p-3 text-sm">{result}</p>}

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
                void invoke("gdrive_set_auto", { on: v });
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

      <AlertDialog open={confirmCancel} onOpenChange={(o) => !cancelling && setConfirmCancel(o)}>
        <AlertDialogContent className="sm:max-w-md">
          <AlertDialogHeader>
            <div className="flex items-start gap-3">
              <div className="flex size-9 shrink-0 items-center justify-center rounded-lg bg-muted">
                <TriangleAlert className="size-4 text-destructive" strokeWidth={1.5} />
              </div>
              <div className="min-w-0">
                <AlertDialogTitle className="text-base font-medium">Cancel this sync?</AlertDialogTitle>
                <AlertDialogDescription>
                  {st && st.runUploaded > 0
                    ? `ChatVault stops now. The ${plural(st.runUploaded)} already uploaded stay in your Google Drive, and the next sync carries on from there.`
                    : "ChatVault stops now. The next sync carries on from where this one stopped."}
                </AlertDialogDescription>
              </div>
            </div>
          </AlertDialogHeader>

          <AlertDialogFooter className="grid grid-cols-2 gap-2 sm:flex">
            <AlertDialogCancel disabled={cancelling}>Keep syncing</AlertDialogCancel>
            <AlertDialogAction
              variant="destructive"
              disabled={cancelling}
              onClick={(e) => {
                e.preventDefault();
                void cancelSync();
              }}
            >
              {cancelling && <Loader2 className="size-4 animate-spin" />} Cancel sync
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>

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
