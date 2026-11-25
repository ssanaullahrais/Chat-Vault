import { AppLogo } from "@/claude-logo";
import { cn } from "@/lib/utils";

/** Shown while ChatVault starts: just the logo, so the list never flashes its skeleton on the first load. */
export function Splash({ leaving }: { leaving: boolean }) {
  return (
    <div
      className={cn("fixed inset-0 z-[70] flex flex-col items-center justify-center gap-4 bg-background transition-opacity duration-300", leaving ? "pointer-events-none opacity-0" : "opacity-100")}
      aria-busy="true"
      aria-label="Starting ChatVault"
      // The window has no title bar while this shows, so the whole screen moves it, logo and text included.
      data-tauri-drag-region
    >
      <AppLogo className="pointer-events-none size-16 animate-pulse rounded-2xl" />
      <p className="pointer-events-none select-none text-sm text-muted-foreground">Getting your chats ready…</p>
    </div>
  );
}
