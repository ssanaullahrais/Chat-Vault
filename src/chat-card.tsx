import type { ReactNode } from "react";
import { Archive, Clock, FolderOpen, Pin } from "lucide-react";
import { cn } from "@/lib/utils";
import { ProviderLogo } from "@/claude-logo";
import { ContextArea, type Entry } from "@/context-area";
import { fmtDate, prettyProject, prettyTitle, type Chat } from "@/chat-actions";
import { Highlight } from "@/highlight";

/**
 * One chat as a card: tool logo, title, project and (on hover) how long ago. Used by the sidebar and by "Pick up where you left off".
 * `menu` is the three-dots button (it should fade in on `group-hover`), `entries` the right-click menu.
 */
export function ChatCard({
  chat,
  title,
  selected,
  pinned,
  live,
  hit,
  hitQuery,
  inProject,
  menu,
  entries,
  onError,
  onOpen,
  onUnpin,
}: {
  chat: Chat;
  title?: string;
  selected?: boolean;
  pinned?: boolean;
  live?: boolean;
  hit?: string;
  /** The search that found this chat: its words are highlighted in the title and the text shown. */
  hitQuery?: string;
  /** The card sits under its project heading, so the project name is replaced by the full date and time. */
  inProject?: boolean;
  menu?: ReactNode;
  entries?: () => Entry[];
  onError?: (e: unknown) => void;
  onOpen: () => void;
  onUnpin?: () => void;
}) {
  const card = (
    <div
      role="button"
      tabIndex={0}
      onClick={onOpen}
      onKeyDown={(e) => e.target === e.currentTarget && e.key === "Enter" && onOpen()}
      className={cn("group w-full cursor-pointer rounded-lg border border-transparent px-2.5 py-2 text-left transition hover:bg-muted dark:hover:bg-white/[0.06]", selected && "bg-muted dark:bg-white/[0.06]")}
    >
      <div className="flex items-start gap-2.5">
        <div className={cn("relative mt-0.5 flex size-8 shrink-0 items-center justify-center rounded-lg bg-muted group-hover:bg-white dark:group-hover:bg-white/10", selected && "bg-white dark:bg-white/10")}>
          <ProviderLogo provider={chat.provider} className="size-5" />
          {live && (
            <span className="absolute -right-1 -top-1 flex size-2" aria-label="An agent is working in this chat">
              <span className="absolute inline-flex size-full animate-ping rounded-full bg-emerald-400 opacity-75" />
              <span className="relative inline-flex size-2 rounded-full bg-emerald-500 ring-[1.5px] ring-background" />
            </span>
          )}
        </div>
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-1.5">
            <span className="min-w-0 flex-1 truncate text-[13px] font-medium">{hitQuery ? <Highlight text={prettyTitle(title ?? chat.title)} q={hitQuery} activeMark={false} /> : prettyTitle(title ?? chat.title)}</span>
            {pinned && (onUnpin ? (
              <button
                type="button"
                aria-label="Unpin chat"
                title="Unpin chat"
                className="flex size-5 shrink-0 items-center justify-center rounded text-muted-foreground hover:bg-muted hover:text-foreground focus-visible:outline-2 focus-visible:outline-ring"
                onClick={(e) => { e.stopPropagation(); onUnpin(); }}
              >
                <Pin className="size-3" />
              </button>
            ) : <Pin className="size-3 shrink-0 text-muted-foreground" />)}
            {(chat.archived || chat.backup) && <Archive className="size-3.5 shrink-0 text-muted-foreground" strokeWidth={1.5} />}
            {menu}
          </div>
          <div className="mt-0.5 flex items-center gap-1.5 text-xs text-muted-foreground">
            {inProject ? (
              <>
                <Clock className="size-3 shrink-0" strokeWidth={1.5} />
                <span className="-ml-0.5 min-w-0 truncate tabular-nums">{fmtDate(chat.lastMs)}</span>
              </>
            ) : (
              <>
                <FolderOpen className="size-3 shrink-0" strokeWidth={1.5} />
                <span className="-ml-0.5 shrink-0" title={prettyProject(chat.project)}>{fmtDate(chat.lastMs)}</span>
              </>
            )}
          </div>
          {hit && <p className="mt-1 line-clamp-2 text-xs text-muted-foreground">{hitQuery ? <Highlight text={hit} q={hitQuery} activeMark={false} /> : hit}</p>}
        </div>
      </div>
    </div>
  );
  return entries ? (
    <ContextArea entries={entries} onError={onError}>
      {card}
    </ContextArea>
  ) : (
    card
  );
}
