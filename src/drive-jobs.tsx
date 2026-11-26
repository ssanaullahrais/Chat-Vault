import { useEffect, useState, useSyncExternalStore } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Settings2 } from "lucide-react";
import driveLogo from "@/assets/google-drive.png";
import { Button } from "@/components/ui/button";
import { Popover, PopoverContent, PopoverTitle, PopoverTrigger } from "@/components/ui/popover";
import { ScrollArea } from "@/components/ui/scroll-area";
import { DRIVE_JOB_ACTION, DriveJobCard, type DriveJobState } from "@/drive-job-card";
import type { DriveStatus } from "@/gdrive-panel";
import { backupChanges, getBackupSnapshot, invalidateBackups } from "@/backup-refresh";
import { describeDriveSync, type DriveStats } from "@/drive-stats";
import { finishToast, notifyError } from "@/toast";

type Unit = { provider: string; key: string };
type Request =
  | { kind: "upload"; units: Unit[] }
  | { kind: "delete"; units: Unit[]; local: boolean; retryIds?: string[] }
  | { kind: "sync" | "restore" };
type Pending = { id: string; scope: string; request: Request };
type Job = Pending & {
  state: DriveJobState;
  resumable: boolean;
  automatic: boolean;
  started: boolean;
  done: number;
  total: number;
  phase: string;
  summary?: string;
  retry?: Request;
  onChanged: () => void;
};
export type Removed = {
  localFiles: number; driveFiles: number; failed: number; error: string | null; stopped: boolean; remaining: string[];
};

const STORAGE = "chatvault.pending-drive-uploads.v1";
const listeners = new Set<() => void>();
let jobs: Job[] = [];
let resuming: Promise<void> | undefined;
const restoredScopes = new Set<string>();
const statusListeners = new Set<() => void>();
let connection: DriveStatus | null = null;
let checkingConnection: Promise<void> | undefined;

function subscribeStatus(listener: () => void) {
  statusListeners.add(listener);
  return () => { statusListeners.delete(listener); };
}

function refreshConnection() {
  checkingConnection ??= invoke<DriveStatus>("gdrive_status").then((status) => {
    connection = status;
    statusListeners.forEach((listener) => listener());
  }).catch(() => {}).finally(() => { checkingConnection = undefined; });
  return checkingConnection;
}

function useDriveConnection() {
  const status = useSyncExternalStore(subscribeStatus, () => connection);
  useEffect(() => {
    void refreshConnection();
    return backupChanges(() => void refreshConnection());
  }, []);
  return status;
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => { listeners.delete(listener); };
}

export function useDriveJobs() {
  return useSyncExternalStore(subscribe, () => jobs);
}

function publish(next: Job[]) {
  jobs = next;
  listeners.forEach((listener) => listener());
}

function patch(id: string, changes: Partial<Job>) {
  publish(jobs.map((job) => job.id === id ? { ...job, ...changes } : job));
}

export function driveJobActive(job: Job) {
  return job.state === "queued" || job.state === "running" || job.state === "stopping";
}

function savedJobs(): Pending[] {
  const value = localStorage.getItem(STORAGE);
  if (!value) return [];
  try {
    const stored = JSON.parse(value);
    return Array.isArray(stored) ? stored.filter((job) => job && typeof job.id === "string" && typeof job.scope === "string"
      && (job.request?.kind === "sync" || (job.request?.kind === "upload" && Array.isArray(job.request.units)))) : [];
  } catch {
    return [];
  }
}

function savePending(job: Pending) {
  localStorage.setItem(STORAGE, JSON.stringify([...savedJobs().filter((item) => item.id !== job.id), job]));
}

function forgetPending(id: string) {
  localStorage.setItem(STORAGE, JSON.stringify(savedJobs().filter((item) => item.id !== id)));
}

function conflicts(a: Request, b: Request) {
  if (!("units" in a) || !("units" in b)) return true;
  if ((a.kind === "delete" && a.local) || (b.kind === "delete" && b.local)) return true;
  return a.units.some((x) => b.units.some((y) => x.provider === y.provider && x.key === y.key));
}

function pump() {
  const running = jobs.filter((job) => job.state === "running" || job.state === "stopping");
  const pending = jobs.filter((job) => job.state === "queued");
  for (const [i, job] of pending.entries()) {
    if (running.length >= 2) break;
    if (running.some((other) => conflicts(job.request, other.request))
      || pending.slice(0, i).some((other) => conflicts(job.request, other.request))) continue;
    running.push(job);
    patch(job.id, { state: "running" });
    void run(job);
  }
}

function enqueue(pending: Pending, onChanged: () => void, resumable: boolean, automatic = false) {
  const active = jobs.filter(driveJobActive);
  const history = jobs.filter((job) => !driveJobActive(job)).slice(-8);
  publish([...history, ...active, { ...pending, onChanged, resumable, automatic, state: "queued", started: false, done: 0, total: 0, phase: "" }]);
  pump();
}

export function resumeDriveJobs(onChanged: () => void) {
  if (resuming) return resuming;
  resuming = (async () => {
    const [scope, connected] = await invoke<[string, boolean]>("backup_scope");
    if (!connected || restoredScopes.has(scope)) return;
    const pending = savedJobs().filter((job) => job.scope === scope && (job.request.kind === "upload" || job.request.kind === "sync"));
    restoredScopes.add(scope);
    for (const job of pending) {
      if (!jobs.some((existing) => existing.id === job.id)) enqueue(job, onChanged, true);
    }
  })().finally(() => { resuming = undefined; });
  return resuming;
}

export async function startDriveJob(request: Request, onChanged: () => void, automatic = false, expectedScope?: string) {
  try {
    await resumeDriveJobs(onChanged);
    if (request.kind === "sync" && jobs.some((job) => job.request.kind === "sync" && driveJobActive(job))) return true;
    const [scope, connected] = await invoke<[string, boolean]>("backup_scope");
    if (!connected) throw Error("Connect Google Drive first.");
    if (expectedScope && scope !== expectedScope) throw Error("Reconnect the original Google Drive account and backup folder before retrying.");
    if (request.kind === "sync" && jobs.some((job) => job.request.kind === "sync" && driveJobActive(job))) return true;
    const pending = { id: crypto.randomUUID(), scope, request };
    const resumable = request.kind === "upload" || (request.kind === "sync" && !automatic);
    if (resumable) savePending(pending);
    enqueue(pending, onChanged, resumable, automatic);
    return true;
  } catch (error) {
    if (!automatic) notifyError(error);
    return false;
  }
}

function summarizeDelete(result: Removed, request: Extract<Request, { kind: "delete" }>) {
  const { driveFiles, localFiles, failed, remaining, stopped, error } = result;
  const parts = [
    stopped && remaining.length ? "Deletion stopped" : "",
    driveFiles ? `${driveFiles.toLocaleString()} ${driveFiles === 1 ? "file" : "files"} deleted from Drive` : "",
    remaining.length ? `${remaining.length.toLocaleString()} remaining` : "",
    localFiles ? `${localFiles.toLocaleString()} moved to the Recycle Bin` : "",
    failed ? `${failed.toLocaleString()} could not be removed from this PC` : "",
    request.local && (stopped || (error && remaining.length)) ? "PC copy kept" : "",
    error && remaining.length ? "Drive deletion interrupted" : "",
  ];
  return {
    summary: parts.filter(Boolean).join(" · ") || "The selected copies were already gone",
    failed: !!error || failed > 0,
    stopped: stopped && remaining.length > 0,
    retry: remaining.length ? { ...request, retryIds: remaining } : undefined,
  };
}

async function run(job: Job) {
  const { id, scope, request, onChanged } = job;
  const selected = "units" in request;
  let off: (() => void) | undefined;
  try {
    const event = selected ? `backup-${request.kind}-progress` : "gdrive-progress";
    off = await listen<{ id?: string; done: number; total: number; phase?: string }>(event, ({ payload }) => {
      if (selected && payload.id !== id) return;
      if (payload.phase === "Done") return;
      const current = jobs.find((item) => item.id === id);
      patch(id, { started: true, done: Math.max(current?.phase === (payload.phase ?? "") ? current.done : 0, payload.done), total: payload.total, phase: payload.phase ?? "" });
    });
    const args = { jobId: id, scope };
    if (request.kind === "delete") {
      const result = await invoke<Removed>("backup_delete", { ...args, units: request.units, local: request.local, drive: true, retryIds: request.retryIds });
      const outcome = summarizeDelete(result, request);
      patch(id, { ...outcome, state: outcome.failed ? "failed" : outcome.stopped ? "stopped" : "done" });
      finishToast(id, outcome.summary, !outcome.failed);
      invalidateBackups();
      if (result.localFiles) onChanged();
    } else {
      const result = request.kind === "upload"
        ? await invoke<DriveStats>("backup_upload", { ...args, units: request.units })
        : await invoke<DriveStats>("gdrive_sync", { restore: request.kind === "restore", scope });
      const summary = describeDriveSync(result);
      if (job.resumable && (!result.failed || result.cancelled)) forgetPending(id);
      patch(id, {
        state: result.cancelled ? "stopped" : result.failed ? "failed" : "done",
        summary,
        retry: result.cancelled || result.failed ? request : undefined,
      });
      if (!job.automatic) finishToast(id, summary, !result.failed);
      if (job.automatic && !result.cancelled && !result.failed && !result.uploaded && !result.downloaded && !result.conflicts) {
        publish(jobs.filter((item) => item.id !== id));
      }
      if (result.backedUp || result.uploaded || result.downloaded || result.conflicts || result.restored || getBackupSnapshot()?.error) invalidateBackups();
      if (result.downloaded || result.conflicts || result.restored) onChanged();
    }
  } catch (error) {
    const stopped = jobs.find((item) => item.id === id)?.state === "stopping";
    const summary = stopped ? "Job stopped" : String(error).replace(/^Error:\s*/, "");
    if (stopped && job.resumable) forgetPending(id);
    patch(id, { state: stopped ? "stopped" : "failed", summary, retry: request.kind !== "delete" || request.retryIds ? request : undefined });
    if (!job.automatic) finishToast(id, summary, false);
    const currentScope = await invoke<[string, boolean]>("backup_scope").catch(() => null);
    if (currentScope && !currentScope[1]) invalidateBackups({ clear: true });
    if (request.kind === "upload") invalidateBackups();
  } finally {
    off?.();
    void refreshConnection();
    pump();
  }
}

async function stopJob(job: Job) {
  try {
    if (job.resumable) forgetPending(job.id);
    if (job.state === "queued") {
      patch(job.id, { state: "stopped", summary: "Stopped before starting", retry: job.request });
      pump();
      return;
    }
    patch(job.id, { state: "stopping" });
    const selected = "units" in job.request;
    await invoke(selected ? "stop_backup_delete" : "gdrive_cancel_sync", selected ? { jobId: job.id } : undefined);
  } catch (e) {
    if (job.resumable) savePending({ id: job.id, scope: job.scope, request: job.request });
    if (jobs.find((item) => item.id === job.id)?.state === "stopping") patch(job.id, { state: "running" });
    notifyError(e);
  }
}

function dismiss(job: Job) {
  try {
    if (job.resumable) forgetPending(job.id);
    publish(jobs.filter((item) => item.id !== job.id));
  } catch (error) {
    notifyError(error);
  }
}

async function retryJob(job: Job) {
  try {
    if (job.retry && await startDriveJob(job.retry, job.onChanged, false, job.scope)) dismiss(job);
  } catch (error) {
    notifyError(error);
  }
}

export function DriveJobsStatus({ onSettings }: { onSettings: () => void }) {
  const items = useDriveJobs();
  const status = useDriveConnection();
  const [open, setOpen] = useState(false);
  const active = items.filter(driveJobActive);
  const item = active[0];
  const queued = active.filter((job) => job.state === "queued").length;
  const visible = [...active, ...items.filter((job) => !driveJobActive(job)).reverse()];
  const attention = !!status?.lastError || !!status?.lastResult?.failed || items.some((job) => job.state === "failed");
  const upToDate = status?.lastResult && !status.lastResult.failed && !status.lastResult.cancelled && !status.lastError;
  const connectionLabel = !status ? "Google Drive" : !status.available ? "Drive unavailable" : !status.connected ? "Drive disconnected"
    : attention ? "Drive needs attention" : upToDate ? "Drive up to date" : "Drive connected";
  const label = active.length > 1 ? `${active.length} Drive jobs`
    : item?.state === "running" ? `${DRIVE_JOB_ACTION[item.request.kind]}${item.total ? ` ${item.done}/${item.total}` : "…"}`
    : item?.state === "stopping" ? "Stopping…"
    : item?.state === "queued" ? "Drive job queued"
    : connectionLabel;
  const stateDot = active.length ? "bg-amber-500"
    : !status?.connected ? "bg-muted-foreground/60" : attention ? "bg-amber-500" : "bg-emerald-500";
  const pulseDot = active.length > 0 || status?.connected;
  const triggerClass = "h-7 shrink-0 gap-1.5 whitespace-nowrap px-2 text-xs";
  if (status?.available && !status.connected && active.length === 0) {
    return (
      <Button variant="secondary" size="sm" className={triggerClass} onClick={onSettings}>
        <img src={driveLogo} alt="" className="size-3.5" />
        Connect Google Drive
      </Button>
    );
  }
  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger render={<Button variant="secondary" size="sm" className={triggerClass} />}>
        <img src={driveLogo} alt="" className="size-3.5 shrink-0" />
        {label}
        {(status || active.length > 0) && (
          <span aria-hidden="true" className={`size-1.5 shrink-0 rounded-full ${stateDot}${pulseDot ? " motion-safe:animate-pulse" : ""}`} />
        )}
      </PopoverTrigger>
      <PopoverContent align="end" sideOffset={6} className="w-72 max-w-[calc(100vw-1.5rem)] gap-0 overflow-hidden p-0">
        <div className="flex items-center gap-2 px-2.5 py-2">
          <span className="flex size-7 shrink-0 items-center justify-center rounded-md bg-muted">
            <img src={driveLogo} alt="" className="size-4" />
          </span>
          <div className="min-w-0 flex-1">
            <PopoverTitle>Drive activity</PopoverTitle>
            <p className="truncate text-xs text-muted-foreground">
              {active.length
                ? [active.length - queued ? `${active.length - queued} running` : "", queued ? `${queued} queued` : ""].filter(Boolean).join(" · ")
                : connectionLabel}
            </p>
          </div>
          <Button variant="ghost" size="icon" className="size-7 shrink-0" aria-label="Drive settings" title="Drive settings" onClick={() => { setOpen(false); onSettings(); }}>
            <Settings2 className="size-4" />
          </Button>
        </div>
        {visible.length > 0 && (
          <ScrollArea className="min-w-0 border-t bg-muted/30" viewportClassName="max-h-[min(13rem,45vh)]">
            <div className="grid min-w-0 grid-cols-[minmax(0,1fr)] gap-1.5 p-2">
              {visible.map((job) => {
                const active = driveJobActive(job);
                return (
                  <DriveJobCard
                    key={job.id}
                    kind={job.request.kind} state={job.state} phase={job.phase} summary={job.summary} done={job.done} total={job.total}
                    canStop={job.state === "queued" || (job.state === "running" && job.started)}
                    onStop={active ? () => void stopJob(job) : undefined}
                    onRetry={!active && job.retry ? () => void retryJob(job) : undefined}
                    onDismiss={!active ? () => dismiss(job) : undefined}
                  />
                );
              })}
            </div>
          </ScrollArea>
        )}
      </PopoverContent>
    </Popover>
  );
}
