import { Channel, invoke } from "@tauri-apps/api/core";
import { useSyncExternalStore } from "react";
import type { Entry } from "@/backups-panel";
import type { DriveStatus } from "@/gdrive-panel";

const STORAGE = "chatvault.backup-list.v1";
const listeners = new Set<() => void>();
const snapshots = new Set<() => void>();
let revision = 0;
type Snapshot = { drive: DriveStatus | null; rows: Entry[] | null; error: string | null; pending: boolean };
type Stored = { scope: string; rows: Entry[] };
let snapshot: Snapshot = { drive: null, rows: null, error: null, pending: true };
let loadedVersion = -1;
let visibleScope: string | undefined;
let loading: Promise<void> | undefined;

export function backupChanges(listener: () => void) {
  listeners.add(listener);
  return () => { listeners.delete(listener); };
}

function subscribe(listener: () => void) {
  snapshots.add(listener);
  return () => { snapshots.delete(listener); };
}

export function useBackupSnapshot() {
  return useSyncExternalStore(subscribe, getBackupSnapshot);
}

export function getBackupSnapshot() {
  return snapshot;
}

function publish(value: Snapshot) {
  snapshot = value;
  snapshots.forEach((listener) => listener());
}

export function invalidateBackups({ clear = false } = {}) {
  revision++;
  if (clear) {
    visibleScope = undefined;
    publish({ drive: null, rows: null, error: null, pending: true });
  } else if (!snapshot.pending) {
    publish({ ...snapshot, pending: true });
  }
  listeners.forEach((listener) => listener());
}

function savedRows(scope: string) {
  try {
    const previous: Stored | null = JSON.parse(localStorage.getItem(STORAGE) ?? "null");
    return previous?.scope === scope && Array.isArray(previous.rows) ? previous.rows : null;
  } catch {
    return null;
  }
}

function rememberRows(scope: string, rows: Entry[]) {
  try {
    localStorage.setItem(STORAGE, JSON.stringify({ scope, rows }));
  } catch {
    // The live list remains available when storage is full or unavailable.
  }
}

export async function readBackupEntries(refresh = false) {
  if (loading) return loading;
  if (!refresh && loadedVersion === revision) return;

  loading = (async () => {
    while (true) {
      const version = revision;
      let scope: string | undefined;
      let drive: DriveStatus | null = null;
      let acceptingPreviews = true;
      try {
        const [identity, status] = await Promise.all([
          invoke<[string, boolean]>("backup_scope"),
          invoke<DriveStatus>("gdrive_status").catch(() => null),
        ]);
        if (version !== revision) continue;
        scope = identity[0];
        drive = status;
        const rows = visibleScope === scope && snapshot.rows ? snapshot.rows : savedRows(scope);
        visibleScope = scope;
        publish({ rows, drive, error: null, pending: true });
        const preview = new Channel<Entry[]>((items) => {
          if (acceptingPreviews && version === revision && items.length) {
            publish({ rows: items, drive, error: null, pending: true });
          }
        });
        const result = await invoke<Entry[]>("backup_entries", { drive: identity[1], refresh, scope, stream: !rows, preview });
        acceptingPreviews = false;
        if (version !== revision) {
          refresh = false;
          continue;
        }
        rememberRows(scope, result);
        loadedVersion = version;
        publish({ rows: result, drive, error: null, pending: false });
        return;
      } catch (error) {
        acceptingPreviews = false;
        if (version !== revision) {
          refresh = false;
          continue;
        }
        loadedVersion = version;
        publish({
          rows: scope && visibleScope === scope ? snapshot.rows : null,
          drive,
          error: String(error),
          pending: false,
        });
        return;
      }
    }
  })();
  try {
    await loading;
  } finally {
    loading = undefined;
  }
}

export const BACKUP_AUTO_ENABLED = "chatvault:backup-auto-enabled";
