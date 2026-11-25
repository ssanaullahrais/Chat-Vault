import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { DatabaseBackup, FolderOpen, Info, Loader2, Settings } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Skeleton } from "@/components/ui/skeleton";
import { Switch } from "@/components/ui/switch";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { ProviderLogo } from "@/claude-logo";
import { fmtDate, fmtSize, type Provider } from "@/chat-actions";
import { notifyError } from "@/toast";
import { PanelHeader, RAIL_TAB, RailLabel } from "@/rail";
import { DriveLogo, type DriveStatus } from "@/gdrive-panel";

type Location = { label: string; path: string; isFile: boolean; exists: boolean; items: number | null; bytes: number | null };
type Source = { provider: Provider; name: string; enabled: boolean; locations: Location[] };
type Where = { path: string; isDefault: boolean; defaultPath: string; available: boolean };

/** One place chats are read from: what it is, where it is, whether it is there, and how much is in it. */
function LocationCard({ loc, onOpen }: { loc: Location; onOpen: () => void }) {
  const facts = [loc.items !== null ? `${loc.items.toLocaleString()} ${loc.items === 1 ? "chat" : "chats"}` : null, loc.bytes !== null ? fmtSize(loc.bytes) : null].filter(Boolean).join(" · ");
  return (
    <div className="flex min-w-0 items-start gap-3 rounded-lg border p-3">
      <div className="min-w-0 flex-1 space-y-1.5">
        <div className="flex flex-wrap items-center gap-2">
          <span className="text-sm font-medium">{loc.label}</span>
          <Badge variant={loc.exists ? "secondary" : "outline"} className="h-4 px-1.5 text-[10px] font-normal">
            {loc.exists ? "Found" : "Not found"}
          </Badge>
          {facts && <span className="text-xs text-muted-foreground">{facts}</span>}
        </div>
        <p className="break-all font-mono text-xs text-muted-foreground">{loc.path}</p>
      </div>
      <Button variant="ghost" size="icon" className="size-7 shrink-0 text-muted-foreground" disabled={!loc.exists} aria-label={loc.isFile ? "Show the file in Explorer" : "Open the folder"} onClick={onOpen}>
        <FolderOpen className="size-4" strokeWidth={1.5} />
      </Button>
    </div>
  );
}

export function SettingsDialog({
  open,
  onOpenChange,
  onOpenBackup,
  onToolsChanged,
}: {
  open: boolean;
  onOpenChange: (o: boolean) => void;
  /** Opens Sync & backup, where the backup folder is changed. */
  onOpenBackup: (tab?: "backup" | "drive") => void;
  /** A tool was switched on or off: the lists are redrawn and re-read. */
  onToolsChanged: () => void | Promise<void>;
}) {
  const [sources, setSources] = useState<Source[] | null>(null);
  const [backup, setBackup] = useState<Where | null>(null);
  const [drive, setDrive] = useState<DriveStatus | null>(null);
  const [tab, setTab] = useState<string>("claude");

  const load = useCallback(() => {
    setSources(null);
    invoke<Source[]>("chat_sources")
      .then(setSources)
      .catch((e) => {
        setSources([]);
        notifyError(e);
      });
    invoke<Where>("backup_location_info").then(setBackup).catch(() => {});
    invoke<DriveStatus>("gdrive_status").then(setDrive).catch(() => {});
  }, []);
  useEffect(() => {
    if (open) load();
  }, [open, load]);

  const openPlace = (path: string) => void invoke("open_chat_source", { path }).catch(notifyError);
  const [switching, setSwitching] = useState<string | null>(null);
  const toggle = async (s: Source, enabled: boolean) => {
    setSwitching(s.provider);
    try {
      await invoke("set_integration", { provider: s.provider, enabled });
      setSources((list) => (list ?? []).map((x) => (x.provider === s.provider ? { ...x, enabled } : x)));
      await onToolsChanged();
    } catch (e) {
      notifyError(e);
    } finally {
      setSwitching(null);
    }
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="flex max-h-[min(88dvh,40rem)] flex-col gap-0 overflow-hidden p-0 sm:max-w-3xl">
        <DialogHeader className="border-b px-5 py-3.5">
          <div className="flex items-center gap-3">
            <div className="flex size-9 shrink-0 items-center justify-center rounded-lg bg-muted">
              <Settings className="size-4 text-muted-foreground" strokeWidth={1.5} />
            </div>
            <div className="min-w-0">
              <DialogTitle className="text-base font-medium">Settings</DialogTitle>
              <DialogDescription>Where ChatVault finds your chats, tool by tool.</DialogDescription>
            </div>
          </div>
        </DialogHeader>

        <Tabs orientation="vertical" value={tab} onValueChange={(v) => setTab(String(v))} className="min-h-0 flex-1 flex-col gap-0 sm:flex-row">
          <ScrollArea className="shrink-0 border-b sm:w-52 sm:border-b-0 sm:border-r" viewportClassName="max-h-[min(70dvh,32rem)]">
            <TabsList variant="line" className="h-auto w-full flex-col items-stretch gap-0.5 p-2" aria-label="Settings sections">
              <RailLabel first>Integrations</RailLabel>
              {sources === null &&
                [88, 64, 72, 96, 80, 56].map((w, i) => (
                  <div key={i} className="flex items-center gap-2.5 px-2.5 py-2" aria-hidden="true">
                    <Skeleton className="size-4 rounded" />
                    <Skeleton className="h-3.5" style={{ width: w }} />
                  </div>
                ))}
              {(sources ?? []).map((s) => {
                const found = s.locations.filter((l) => l.exists).length;
                return (
                  <TabsTrigger key={s.provider} value={s.provider} className={RAIL_TAB}>
                    <ProviderLogo provider={s.provider} className="size-4 shrink-0" />
                    <span className="min-w-0 flex-1 truncate text-left">{s.name}</span>
                    {s.enabled ? (
                      <span className={`size-1.5 shrink-0 rounded-full ${found ? "bg-emerald-500" : "bg-muted-foreground/40"}`} aria-label={found ? "On, found on this PC" : "On, nothing found"} />
                    ) : (
                      <span className="shrink-0 text-[10px] text-muted-foreground">Off</span>
                    )}
                  </TabsTrigger>
                );
              })}
              <RailLabel>General</RailLabel>
              <TabsTrigger value="backup" className={RAIL_TAB}>
                <DatabaseBackup className="size-4 shrink-0" strokeWidth={1.5} /> Backup
              </TabsTrigger>
            </TabsList>
          </ScrollArea>

          <ScrollArea className="min-h-0 min-w-0 flex-1" viewportClassName="max-h-[min(70dvh,32rem)]">
            <div className="p-5">
              <TabsContent value="backup">
                <PanelHeader icon={<DatabaseBackup className="size-4 text-muted-foreground" strokeWidth={1.5} />} title="Backup" hint="Copies of your chats, kept outside the tools." />
                <div className="flex min-w-0 items-start gap-3 rounded-lg border p-3">
                  <div className="min-w-0 flex-1 space-y-1.5">
                    <div className="flex flex-wrap items-center gap-2">
                      <span className="text-sm font-medium">Backup folder</span>
                      {backup === null ? (
                        <Skeleton className="h-4 w-28" />
                      ) : (
                        <>
                          <Badge variant={backup.available ? "secondary" : "outline"} className={`h-4 px-1.5 text-[10px] font-normal ${backup.available ? "" : "text-destructive"}`}>
                            {backup.available ? "Found" : "Not reachable"}
                          </Badge>
                          <span className="text-xs text-muted-foreground">{backup.isDefault ? "The default folder in Documents" : "A folder you chose"}</span>
                        </>
                      )}
                    </div>
                    {backup === null ? <Skeleton className="h-3.5 w-3/4" /> : <p className="break-all font-mono text-xs text-muted-foreground">{backup.path}</p>}
                  </div>
                  <Button variant="ghost" size="icon" className="size-7 shrink-0 text-muted-foreground" disabled={!backup?.available} aria-label="Open the backup folder" onClick={() => backup && void invoke("open_folder", { path: backup.path }).catch(notifyError)}>
                    <FolderOpen className="size-4" strokeWidth={1.5} />
                  </Button>
                </div>
                {/* Google Drive: where the backup also goes; it is set up in Sync & backup. */}
                <div className="mt-3 flex min-w-0 items-center gap-3 rounded-lg border p-3">
                  <DriveLogo className="size-5 shrink-0" />
                  <div className="min-w-0 flex-1">
                    <div className="flex flex-wrap items-center gap-2">
                      <span className="text-sm font-medium">Google Drive</span>
                      {drive === null ? (
                        <Skeleton className="h-4 w-20" />
                      ) : (
                        <Badge variant={drive.available && drive.connected ? "secondary" : "outline"} className="h-4 px-1.5 text-[10px] font-normal">
                          {!drive.available ? "Not in this build" : drive.connected ? "Connected" : "Not connected"}
                        </Badge>
                      )}
                    </div>
                    <p className="truncate text-xs text-muted-foreground">
                      {drive?.connected ? `${drive.email ?? "Google account"}${drive.lastSyncMs ? ` · synced ${fmtDate(drive.lastSyncMs)}` : ""}` : "Back up to your own Drive and share chats between your PCs."}
                    </p>
                  </div>
                  {drive?.available && (
                    <Button variant="outline" size="sm" className="shrink-0" onClick={() => { onOpenChange(false); onOpenBackup("drive"); }}>
                      {drive.connected ? "Manage…" : "Connect…"}
                    </Button>
                  )}
                </div>
                <div className="mt-3 flex items-center justify-between gap-3">
                  <p className="text-xs text-muted-foreground">Backed-up chats stay visible after you sign out or reinstall.</p>
                  <Button variant="outline" size="sm" className="shrink-0" onClick={() => { onOpenChange(false); onOpenBackup(); }}>
                    Change folder…
                  </Button>
                </div>
              </TabsContent>

              {sources === null && tab !== "backup" && (
                <div aria-hidden="true">
                  <div className="mb-4 flex items-center gap-3">
                    <Skeleton className="size-9 rounded-lg" />
                    <div className="space-y-1.5">
                      <Skeleton className="h-3.5 w-28" />
                      <Skeleton className="h-3 w-44" />
                    </div>
                  </div>
                  <div className="space-y-2">
                    {[0, 1, 2].map((i) => (
                      <div key={i} className="space-y-2 rounded-lg border p-3">
                        <div className="flex items-center gap-2">
                          <Skeleton className="h-4 w-20" />
                          <Skeleton className="h-4 w-12 rounded-full" />
                          <Skeleton className="h-3.5 w-24" />
                        </div>
                        <Skeleton className="h-3.5 w-4/5" />
                      </div>
                    ))}
                  </div>
                </div>
              )}
              {(sources ?? []).map((s) => {
                const found = s.locations.filter((l) => l.exists).length;
                const chats = s.locations.reduce((n, l) => n + (l.exists ? (l.items ?? 0) : 0), 0);
                return (
                  <TabsContent key={s.provider} value={s.provider}>
                    <PanelHeader
                      icon={<ProviderLogo provider={s.provider} className="size-5" />}
                      title={s.name}
                      hint={!s.enabled ? "Switched off" : found === 0 ? "Nothing found on this PC." : `${found} of ${s.locations.length} ${s.locations.length === 1 ? "place" : "places"} found${chats ? ` · ${chats.toLocaleString()} chats counted` : ""}`}
                      action={
                        <label className="flex shrink-0 cursor-pointer items-center gap-2 text-xs text-muted-foreground">
                          {switching === s.provider ? <Loader2 className="size-3.5 animate-spin" /> : s.enabled ? "On" : "Off"}
                          <Switch checked={s.enabled} disabled={switching !== null} onCheckedChange={(v) => void toggle(s, v)} aria-label={`${s.enabled ? "Switch off" : "Switch on"} ${s.name}`} />
                        </label>
                      }
                    />
                    {!s.enabled && (
                      <div role="status" className="mb-3 flex items-start gap-2 rounded-lg border bg-muted/40 p-3 text-xs text-muted-foreground">
                        <Info className="mt-0.5 size-3.5 shrink-0" strokeWidth={1.5} />
                        <p>{s.name} is switched off. Its chats are not listed, counted or backed up. Nothing is deleted, and switching it on again brings everything back.</p>
                      </div>
                    )}
                    <div className={`space-y-2 ${s.enabled ? "" : "opacity-60"}`}>
                      {s.locations.map((l) => (
                        <LocationCard key={`${l.label}:${l.path}`} loc={l} onOpen={() => openPlace(l.path)} />
                      ))}
                    </div>
                  </TabsContent>
                );
              })}

              <div role="note" className="mt-5 flex items-start gap-2 rounded-lg bg-muted/40 p-3 text-xs text-muted-foreground">
                <Info className="mt-0.5 size-3.5 shrink-0" strokeWidth={1.5} />
                <p>ChatVault reads these places on this PC. Nothing is uploaded, and nothing in them is changed unless you delete a chat yourself.</p>
              </div>
            </div>
          </ScrollArea>
        </Tabs>
      </DialogContent>
    </Dialog>
  );
}
