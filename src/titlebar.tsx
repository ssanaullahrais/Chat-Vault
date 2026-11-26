import { useEffect, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { Copy, Minus, Moon, RefreshCw, Square, Sun, X } from "lucide-react";
import { DriveJobsStatus } from "@/drive-jobs";
import { cn } from "@/lib/utils";

const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

function Control({ label, onClick, danger, children }: { label: string; onClick: () => void; danger?: boolean; children: React.ReactNode }) {
  return (
    <button
      type="button"
      aria-label={label}
      onClick={onClick}
      className={cn("mx-0.5 flex size-7 items-center justify-center rounded-md text-muted-foreground transition-colors hover:text-foreground", danger ? "hover:bg-red-600 hover:text-white" : "hover:bg-muted")}
    >
      {children}
    </button>
  );
}

/** Our own window bar: it blends with the app, drags the window, and drives the native minimize, maximize and close. */
export function TitleBar({ title, actions, sidebarOpen, busy, dark, onRefresh, onTheme, onDriveSettings }: { title?: string; actions?: React.ReactNode; sidebarOpen: boolean; busy: boolean; dark: boolean; onRefresh: () => void; onTheme: () => void; onDriveSettings: () => void }) {
  const [max, setMax] = useState(false);

  useEffect(() => {
    if (!inTauri) return;
    const w = getCurrentWindow();
    const sync = () => void w.isMaximized().then(setMax).catch(() => {});
    sync();
    let off: (() => void) | undefined;
    void w.onResized(sync).then((u) => (off = u));
    return () => off?.();
  }, []);

  if (!inTauri) return null;
  const w = getCurrentWindow();
  return (
    <div data-tauri-drag-region className="relative z-30 flex h-11 shrink-0 select-none items-center justify-between gap-3 bg-background pl-4 text-xs text-muted-foreground">
      <p data-tauri-drag-region className={cn("pointer-events-none min-w-0 flex-1 truncate text-sm font-medium text-foreground transition-[padding] duration-300", !sidebarOpen && "pl-10")}>
        {title}
      </p>
      <div className="flex h-full shrink-0 items-center pr-2">
        <DriveJobsStatus onSettings={onDriveSettings} />
        {actions && <div className="mx-2 flex shrink-0 items-center gap-2">{actions}</div>}
        <Control label="Scan this PC for new chats" onClick={onRefresh}>
          <RefreshCw className={cn("size-3.5", busy && "animate-spin")} strokeWidth={1.5} />
        </Control>
        <Control label={dark ? "Light theme" : "Dark theme"} onClick={onTheme}>
          {dark ? <Sun className="size-3.5" strokeWidth={1.5} /> : <Moon className="size-3.5" strokeWidth={1.5} />}
        </Control>
        <span className="mx-2.5 h-3.5 w-px shrink-0 bg-border" />
        <Control label="Minimize" onClick={() => void w.minimize()}>
          <Minus className="size-4" strokeWidth={1.5} />
        </Control>
        <Control label={max ? "Restore" : "Maximize"} onClick={() => void w.toggleMaximize()}>
          {max ? <Copy className="size-3.5" strokeWidth={1.5} /> : <Square className="size-3.5" strokeWidth={1.5} />}
        </Control>
        <Control label="Close" danger onClick={() => void w.close()}>
          <X className="size-4" strokeWidth={1.5} />
        </Control>
      </div>
    </div>
  );
}
