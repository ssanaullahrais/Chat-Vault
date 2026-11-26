import { fmtSize } from "@/chat-actions";

export type DriveStats = { backedUp?: number; uploaded: number; downloaded: number; conflicts: number; failed: number; unchanged: number; bytesUp: number; bytesDown: number; restored: number; cancelled: boolean };

const plural = (n: number) => `${n.toLocaleString()} ${n === 1 ? "file" : "files"}`;

/** One line about what the last sync did. */
export function describeDriveSync(s: DriveStats) {
  if (s.cancelled) {
    return s.uploaded ? `Sync paused after ${plural(s.uploaded)}. The next sync carries on from there.` : "Sync paused. The next sync carries on from there.";
  }
  const parts: string[] = [];
  if (s.uploaded) parts.push(`uploaded ${s.uploaded.toLocaleString()} ${s.uploaded === 1 ? "file" : "files"} (${fmtSize(s.bytesUp)})`);
  if (s.downloaded) parts.push(`downloaded ${plural(s.downloaded)} (${fmtSize(s.bytesDown)})`);
  if (s.restored) parts.push(`restored ${plural(s.restored)} into the tools`);
  if (!parts.length && !s.conflicts) parts.push(s.failed ? "no files were copied" : "everything was already up to date");
  let text = parts.join(", ");
  if (text) text = text.charAt(0).toUpperCase() + text.slice(1) + ".";
  if (s.conflicts) text += ` ${s.conflicts} ${s.conflicts === 1 ? "chat was" : "chats were"} changed on two PCs, so both copies were kept.`;
  if (s.failed) text += ` ${s.failed} files could not be copied. Try again to finish.`;
  return text.trim();
}
