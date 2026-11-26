import { Check, CloudDownload, CloudUpload, RefreshCw, Square, Trash2, X } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Progress } from "@/components/ui/progress";
import { cn } from "@/lib/utils";

export type DriveJobKind = "upload" | "delete" | "sync" | "restore";
export type DriveJobState = "queued" | "running" | "stopping" | "done" | "stopped" | "failed";

export const DRIVE_JOB_TITLE = { upload: "Drive upload", delete: "Drive deletion", sync: "Drive sync", restore: "Drive restore" };
export const DRIVE_JOB_ACTION = { upload: "Uploading", delete: "Deleting", sync: "Syncing", restore: "Restoring" };
const ICON = { upload: CloudUpload, delete: Trash2, sync: RefreshCw, restore: CloudDownload };

export function DriveJobCard({
  kind, state, phase, summary, done, total, canStop, onStop, onRetry, onDismiss,
}: {
  kind: DriveJobKind;
  state: DriveJobState;
  phase: string;
  summary?: string;
  done: number;
  total: number;
  canStop: boolean;
  onStop?: () => void;
  onRetry?: () => void;
  onDismiss?: () => void;
}) {
  const Icon = ICON[kind];
  const running = state === "running" || state === "stopping";
  const attention = state === "failed" || state === "stopped";
  const title = DRIVE_JOB_TITLE[kind];
  const status = state === "queued" ? "Queued" : state === "stopping" ? "Stopping…" : running ? phase || "Preparing files" : state === "failed" ? "Needs attention" : state === "stopped" ? "Stopped" : "";
  const detail = summary || status;
  return (
    <article aria-label={title} className="flex min-w-0 items-start gap-2 rounded-lg border bg-background px-2 py-1.5 text-xs">
      <span className={cn("flex size-7 shrink-0 items-center justify-center rounded-md bg-muted text-muted-foreground", attention && "bg-amber-500/10 text-amber-700 dark:text-amber-400")}>
        <Icon aria-hidden="true" className="size-3.5" />
      </span>
      <div className="min-w-0 flex-1 self-center">
        <div className="flex items-center gap-1.5">
          <p className="truncate font-medium leading-4">{title}</p>
          {state === "done" && <Check aria-label="Complete" className="size-3 shrink-0 text-emerald-600 dark:text-emerald-400" />}
        </div>
        {(detail || (running && total > 0)) && (
          <div className="flex min-w-0 items-center gap-1.5 text-[11px] leading-4 text-muted-foreground">
            {detail && <p className="min-w-0 flex-1 truncate" title={detail}>{detail}</p>}
            {running && total > 0 && <p className="shrink-0 tabular-nums" aria-label={`${done} of ${total} files`}>{done.toLocaleString()} / {total.toLocaleString()}</p>}
          </div>
        )}
        {running && <Progress className="mt-1" aria-label={`${title} progress`} value={total ? Math.min(100, done * 100 / total) : null} />}
      </div>
      {(onStop || onRetry || onDismiss) && (
        <div className="flex shrink-0 items-center gap-0.5 self-center">
          {onStop && (
            <Button
              variant="ghost" size="icon" className="size-7 text-muted-foreground" disabled={!canStop}
              aria-label={state === "queued" ? "Cancel job" : `Stop ${DRIVE_JOB_ACTION[kind].toLowerCase()}`}
              title={state === "queued" ? "Cancel job" : `Stop ${DRIVE_JOB_ACTION[kind].toLowerCase()}`}
              onClick={onStop}
            >
              {state === "queued" ? <X className="size-3.5" /> : <Square className="size-3" />}
            </Button>
          )}
          {onRetry && (
            <Button variant="ghost" size="icon" className="size-7 text-muted-foreground"
              aria-label={kind === "delete" ? "Retry remaining" : `Retry ${title.toLowerCase()}`}
              title={kind === "delete" ? "Retry remaining" : "Retry"} onClick={onRetry}>
              <RefreshCw className="size-3.5" />
            </Button>
          )}
          {onDismiss && (
            <Button variant="ghost" size="icon" className="size-7 text-muted-foreground" aria-label={`Dismiss ${title.toLowerCase()}`} title="Dismiss" onClick={onDismiss}>
              <X className="size-3.5" />
            </Button>
          )}
        </div>
      )}
    </article>
  );
}
