import { useEffect, useId, useMemo, useState, type ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Area, AreaChart, CartesianGrid, XAxis } from "recharts";
import { ChartContainer, ChartTooltip, ChartTooltipContent, type ChartConfig } from "@/components/ui/chart";
import { FadedScroll } from "@/faded-scroll";
import { Activity, ArrowUpRight, DatabaseBackup, HardDrive, FolderOpen, Bug, Copy, History, MessagesSquare, MoreHorizontal, Play, Tag, Moon, RefreshCw, Search, Sun } from "lucide-react";
import { ContextArea, type Entry } from "@/context-area";
import { notify } from "@/toast";
import { openUrl } from "@tauri-apps/plugin-opener";
import { REPO_URL } from "@/made-with";
import { OverviewFooter } from "@/made-with";
import { Button } from "@/components/ui/button";
import { AppLogo, GithubIcon, ProviderLogo } from "@/claude-logo";
import { CORE_PROVIDERS, PROVIDERS, PROVIDER_SHORT, chatActions, fmtSize, type Chat, type Provider } from "@/chat-actions";
import { ChatCard } from "@/chat-card";
import { ChatRowSkeleton } from "@/skeletons";
import { Skeleton } from "@/components/ui/skeleton";
import { Carousel, CarouselContent, CarouselDots, CarouselItem } from "@/components/ui/carousel";

type Overview = { total: number; accounts: { id: string; count: number }[]; latestMs: number | null };
type Backup = { path: string; exists: boolean; files: number; bytes: number; lastSyncMs: number | null };
type Stats = {
  daily: { day: string; provider: Provider; count: number }[];
  projects: { name: string; count: number; claude: number; codex: number; cursor: number; lastMs: number }[];
  sizeClaude: number;
  sizeCodex: number;
  sizes?: Record<string, number>;
};

const DAYS = 14;
const CLAUDE = "#D97757";
const CODEX = "#7a9dff";
const CURSOR = "#2dd4bf";
const COLOR: Record<Provider, string> = { claude: CLAUDE, codex: CODEX, cursor: CURSOR, gemini: "#c084fc", opencode: "#a3e635", cline: "#fb7185" };
const chartConfig: ChartConfig = Object.fromEntries(PROVIDERS.map((p) => [p, { label: PROVIDER_SHORT[p], color: COLOR[p] }])) as ChartConfig;
const zero = (): Record<Provider, number> => ({ claude: 0, codex: 0, cursor: 0, gemini: 0, opencode: 0, cline: 0 });

/** A tiny line of the last days, so a card shows which way a tool is heading. */
function Spark({ values, color }: { values: number[]; color: string }) {
  const id = useId().replace(/:/g, "");
  const data = values.map((v, i) => ({ i, v }));
  const idle = values.every((v) => v === 0);
  // The line fades out at both ends so it melts into the card instead of stopping at an edge.
  const fade = "linear-gradient(to right, transparent, #000 28%, #000 72%, transparent)";
  return (
    <ChartContainer
      config={{ v: { label: "Chats", color } }}
      className="pointer-events-none absolute right-3 top-4 aspect-auto h-12 w-28"
      style={{ maskImage: fade, WebkitMaskImage: fade, opacity: idle ? 0.35 : 1 }}
    >
      <AreaChart data={data} margin={{ left: 0, right: 0, top: 4, bottom: 4 }}>
        <defs>
          <linearGradient id={`sp-${id}`} x1="0" y1="0" x2="0" y2="1">
            <stop offset="0%" stopColor={color} stopOpacity={0.35} />
            <stop offset="100%" stopColor={color} stopOpacity={0} />
          </linearGradient>
        </defs>
        <Area dataKey="v" type="monotone" stroke={color} strokeWidth={2} strokeLinecap="round" fill={`url(#sp-${id})`} isAnimationActive={false} />
      </AreaChart>
    </ChartContainer>
  );
}

function greeting() {
  const h = new Date().getHours();
  if (h < 5) return "Working late";
  if (h < 12) return "Good morning";
  if (h < 18) return "Good afternoon";
  return "Good evening";
}

function when(ms: number) {
  const d = new Date(ms);
  const now = new Date();
  const time = d.toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" });
  if (d.toDateString() === now.toDateString()) return `Today, ${time}`;
  const y = new Date(now);
  y.setDate(now.getDate() - 1);
  if (d.toDateString() === y.toDateString()) return `Yesterday, ${time}`;
  return d.toLocaleDateString(undefined, { month: "short", day: "numeric", year: d.getFullYear() === now.getFullYear() ? undefined : "numeric" });
}

/** Chats started from a handoff file are named after the instruction; show what they really are. */
function niceTitle(title: string) {
  if (!/^Read the file\s/i.test(title) || !/handoffs?/i.test(title)) return title;
  const file = /handoffs?[\/]+([^\/\s]+)/i.exec(title)?.[1] ?? "";
  const name = file.replace(/\.(md|txt)$/i, "").replace(/_+/g, " ").replace(/\s+/g, " ").trim();
  return name ? `Continued from handoff: ${name}` : "Continued from a handoff file";
}

const dayKey = (d: Date) => `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;

function Card({ title, hint, action, icon, children, className = "", ctx }: { icon?: ReactNode; title?: string; hint?: string; action?: ReactNode; children: ReactNode; className?: string; ctx?: string }) {
  return (
    <section data-ctx={ctx} className={`rounded-xl border bg-card ${className}`}>
      {(title || action) && (
        <div className="flex items-center justify-between gap-2 px-4 pt-3.5">
          <div className="flex min-w-0 items-center gap-3">
            {icon && <div className="flex size-8 shrink-0 items-center justify-center rounded-lg bg-muted">{icon}</div>}
            <div className="min-w-0">
              <h3 className="text-sm font-medium">{title}</h3>
              {hint && <p className="text-xs text-muted-foreground">{hint}</p>}
            </div>
          </div>
          {action}
        </div>
      )}
      {children}
    </section>
  );
}

function IconBox({ children }: { children: ReactNode }) {
  return <div className="flex size-8 shrink-0 items-center justify-center rounded-lg bg-muted">{children}</div>;
}

export function OverviewPage({
  overview,
  accountCount,
  reloadKey,
  titleOf,
  onOpenChat,
  onSync,
  onSearch,
  actions,
  indexing,
  userName,
  dark,
  onShowTool,
  onRefresh,
  onTheme,
  onOpenBackup,
  rowEntries,
}: {
  overview: Record<Provider, Overview | null>;
  accountCount: number;
  reloadKey: number;
  titleOf: (c: Chat) => string;
  onOpenChat: (c: Chat) => void;
  onSync: () => void;
  onSearch: () => void;
  actions?: ReactNode;
  /** While ChatVault is scanning, the line under the greeting shows the progress instead. */
  indexing?: { done: number; total: number } | null;
  userName?: string;
  dark?: boolean;
  onShowTool?: (p: Provider) => void;
  onRefresh?: () => void;
  onTheme?: () => void;
  onOpenBackup?: () => void;
  /** The actions of one chat row, shared with the sidebar menu. */
  rowEntries?: (c: Chat) => Entry[];
}) {
  // The card always shows the four latest chats.
  const shown = 4;
  const [recent, setRecent] = useState<Chat[] | null>(null);
  const [backup, setBackup] = useState<Backup | null>(null);
  const [stats, setStats] = useState<Stats | null>(null);

  useEffect(() => {
    let stale = false;
    // The four newest chats across every tool, newest first.
    invoke<{ items: Chat[] }>("query_chats", { provider: "all", account: "all", q: "", offset: 0, limit: 4, sort: "recent", scope: "all" })
      .then((r) => !stale && setRecent(r.items))
      .catch(() => !stale && setRecent([]));
    invoke<Backup>("backup_status").then((b) => !stale && setBackup(b)).catch(() => {});
    invoke<Stats>("overview_stats", { days: DAYS }).then((s) => !stale && setStats(s)).catch(() => {});
    return () => {
      stale = true;
    };
  }, [reloadKey]);

  const ready = overview.claude !== null && overview.codex !== null && overview.cursor !== null;
  const total = PROVIDERS.reduce((n, p) => n + (overview[p]?.total ?? 0), 0);

  // One column per day, oldest first, with a count for each tool.
  const days = useMemo(() => {
    const map = new Map<string, Record<Provider, number>>();
    stats?.daily.forEach((d) => {
      const e = map.get(d.day) ?? zero();
      if (d.provider in e) e[d.provider] += d.count;
      map.set(d.day, e);
    });
    return Array.from({ length: DAYS }, (_, i) => {
      const date = new Date();
      date.setDate(date.getDate() - (DAYS - 1 - i));
      const e = map.get(dayKey(date)) ?? zero();
      return { date, ...e, total: PROVIDERS.reduce((n, p) => n + e[p], 0) };
    });
  }, [stats]);
  const chartData = days.map((d) => ({ label: d.date.toLocaleDateString(undefined, { month: "short", day: "numeric" }), ...Object.fromEntries(PROVIDERS.map((p) => [p, d[p]])) }));
  // Only tools with activity in these days get an area: empty ones would just trace the top edge of the others.
  const drawn = PROVIDERS.filter((p) => days.some((d) => d[p] > 0));
  const shownTools = PROVIDERS.filter((p) => CORE_PROVIDERS.includes(p) || (overview[p]?.total ?? 0) > 0);
  const week = days.slice(-7).reduce((n, d) => n + d.total, 0);

  // One card per tool that has chats, then the storage card. Swipe or use the arrows when there are more than fit.
  const cards = [
    ...shownTools.map((p) => ({ key: p, color: COLOR[p], spark: days.map((d) => d[p]), icon: <ProviderLogo provider={p} className="size-4" />, value: overview[p]?.total.toLocaleString(), label: `${PROVIDER_SHORT[p]} chats` })),
    { key: "disk", color: undefined as string | undefined, spark: undefined as number[] | undefined, icon: <HardDrive className="size-4 text-muted-foreground" strokeWidth={1.5} />, value: stats ? fmtSize(PROVIDERS.reduce((n, p) => n + (stats.sizes?.[p] ?? 0), 0)) : undefined, label: "Stored on this PC" },
  ];

  return (
    <ContextArea
      className="flex min-h-0 flex-1 flex-col"
      entries={(target) => {
        const ctx = target.closest<HTMLElement>("[data-ctx]")?.dataset.ctx;
        const tool = target.closest<HTMLElement>("[data-tool]")?.dataset.tool as Provider | undefined;
        const copy = (text: string, what: string) => async () => {
          await navigator.clipboard.writeText(text);
          notify(`${what} copied`);
        };
        const web = (url: string) => () => void openUrl(url).catch(() => {});
        const active = PROVIDERS.filter((p) => CORE_PROVIDERS.includes(p) || (overview[p]?.total ?? 0) > 0);
        const latest = async (p: Provider) => {
          const r = await invoke<{ items: Chat[] }>("query_chats", { provider: p, account: "all", q: "", offset: 0, limit: 1, sort: "recent", scope: "all" });
          return r.items[0] ?? null;
        };
        const summary = `ChatVault: ${total.toLocaleString()} chats across ${accountCount} accounts (${active.map((p) => `${PROVIDER_SHORT[p]} ${overview[p]?.total ?? 0}`).join(", ")}), ${week} active this week.`;

        const refresh: Entry[] = onRefresh ? [{ type: "item", label: "Refresh", icon: <RefreshCw strokeWidth={1.5} />, onSelect: onRefresh }, { type: "sep" }] : [];

        // What is specific to the spot that was clicked.
        let here: Entry[] = [];
        if (tool && onShowTool) {
          const name = PROVIDER_SHORT[tool];
          here = [
            { type: "label", label: `${name} · ${(overview[tool]?.total ?? 0).toLocaleString()} chats` },
            { type: "item", label: "Show these chats", icon: <ProviderLogo provider={tool} className="size-4" />, onSelect: () => onShowTool(tool) },
            { type: "item", label: "Open latest", icon: <History strokeWidth={1.5} />, onSelect: async () => { const c = await latest(tool); if (c) onOpenChat(c); } },
            {
              type: "item",
              label: "Resume latest",
              icon: <Play strokeWidth={1.5} />,
              onSelect: async () => {
                const c = await latest(tool);
                const go = c ? chatActions(c, notify, null).find((a) => a.id === `continue-${tool}`) : undefined;
                if (go) await go.run();
                else notify(`${name} chats can't be resumed from here. Open the chat and use Actions.`);
              },
            },
            { type: "item", label: "Copy count", icon: <Copy strokeWidth={1.5} />, onSelect: copy(`${overview[tool]?.total ?? 0} ${name} chats`, "Count") },
            { type: "sep" },
          ];
        } else if (ctx === "chart") {
          const lines = shownTools.map((p) => `${PROVIDER_SHORT[p]}: ${days.reduce((n, d) => n + d[p], 0)}`);
          here = [
            { type: "label", label: "Activity" },
            { type: "item", label: "Copy this summary", icon: <Copy strokeWidth={1.5} />, onSelect: copy(`Chats in the last ${DAYS} days, ${lines.join(", ")}. ${week} in the last 7 days.`, "Summary") },
            { type: "sep" },
          ];
        } else if (ctx === "storage") {
          const lines = shownTools.map((p) => `${PROVIDER_SHORT[p]}: ${fmtSize(stats?.sizes?.[p] ?? 0)}`);
          here = [
            { type: "label", label: "Storage" },
            { type: "item", label: "Copy this summary", icon: <Copy strokeWidth={1.5} />, onSelect: copy(`ChatVault storage by tool, ${lines.join(", ")}.`, "Summary") },
            { type: "sep" },
          ];
        } else if (ctx === "backup") {
          here = [
            { type: "label", label: "Backup" },
            { type: "item", label: "Back up now", icon: <DatabaseBackup strokeWidth={1.5} />, onSelect: onSync },
            ...(backup?.path ? ([{ type: "item", label: "Copy the backup path", icon: <Copy strokeWidth={1.5} />, onSelect: copy(backup.path, "Path") }] as Entry[]) : []),
            { type: "sep" },
          ];
        }

        // The same on every part of the page.
        return [
          ...refresh,
          ...here,
          { type: "item", label: "Search all chats", icon: <Search strokeWidth={1.5} />, onSelect: onSearch },
          { type: "item", label: "Sync & backup", icon: <DatabaseBackup strokeWidth={1.5} />, onSelect: onSync },
          { type: "sep" },
          ...(onShowTool
            ? ([
                {
                  type: "sub",
                  label: "Go to",
                  icon: <ArrowUpRight strokeWidth={1.5} />,
                  entries: active.map((p) => ({ type: "item", label: `${PROVIDER_SHORT[p]} chats`, icon: <ProviderLogo provider={p} className="size-4" />, onSelect: () => onShowTool(p) }) as Entry),
                },
              ] as Entry[])
            : []),
          {
            type: "sub",
            label: "Open",
            icon: <FolderOpen strokeWidth={1.5} />,
            entries: [
              ...(onOpenBackup ? ([{ type: "item", label: "The backup folder", icon: <FolderOpen strokeWidth={1.5} />, onSelect: onOpenBackup }] as Entry[]) : []),
              { type: "item", label: "The project on GitHub", icon: <GithubIcon className="size-4" />, onSelect: web(REPO_URL) },
              { type: "item", label: "Release notes", icon: <Tag strokeWidth={1.5} />, onSelect: web(`${REPO_URL}/releases`) },
              { type: "item", label: "Report a problem", icon: <Bug strokeWidth={1.5} />, onSelect: web(`${REPO_URL}/issues`) },
            ],
          },
          {
            type: "sub",
            label: "Copy",
            icon: <Copy strokeWidth={1.5} />,
            entries: [
              { type: "item", label: "Overview summary", icon: <Copy strokeWidth={1.5} />, onSelect: copy(summary, "Summary") },
              ...(backup?.path ? ([{ type: "item", label: "Backup folder path", icon: <Copy strokeWidth={1.5} />, onSelect: copy(backup.path, "Path") }] as Entry[]) : []),
            ],
          },
          ...(onTheme ? ([{ type: "sep" }, { type: "item", label: dark ? "Light theme" : "Dark theme", icon: dark ? <Sun strokeWidth={1.5} /> : <Moon strokeWidth={1.5} />, onSelect: onTheme }] as Entry[]) : []),
        ];
      }}
    >
    <FadedScroll className="flex-1">
      <div className="@container flex min-h-full items-start justify-center px-6 pb-10 pt-6 min-[1200px]:pt-10">
        <div className="w-full max-w-4xl space-y-4 @min-[1080px]:max-w-5xl @min-[1400px]:max-w-[1160px]">
          <div className="flex items-center gap-3">
            <AppLogo className="size-10 shrink-0 rounded-xl ring-1 ring-border" />
            <div className="min-w-0 flex-1">
              <h2 className="text-base font-medium leading-tight">
                {greeting()}
                {userName ? `, ${userName}` : ""}
              </h2>
              <p className="text-sm text-muted-foreground">
                {indexing
                  ? indexing.total
                    ? `Indexing ${indexing.done.toLocaleString()} of ${indexing.total.toLocaleString()} chats…`
                    : "Scanning this computer for chats…"
                  : ready
                  ? `${total.toLocaleString()} chats across ${accountCount} accounts${stats ? `, ${week.toLocaleString()} active this week` : ""}`
                  : "Looking for chats on this computer…"}
              </p>
            </div>
            <div className="flex shrink-0 items-center gap-2">{actions}</div>
          </div>

          <div>
            <Carousel opts={{ align: "start", slidesToScroll: 1, dragFree: false }}>
              <CarouselContent>
                {cards.map((s) => (
                  <CarouselItem key={s.key} className="basis-1/2 @min-[820px]:basis-1/4">
                    <div data-tool={s.key === "disk" ? undefined : s.key} data-ctx={s.key === "disk" ? "storage" : undefined} className="relative overflow-hidden rounded-xl border bg-card p-4">
                      {s.color && (
                        <div
                          className="pointer-events-none absolute -right-8 -top-10 size-28 rounded-full opacity-[0.12] blur-2xl"
                          style={{ background: s.color }}
                        />
                      )}
                      <IconBox>{s.icon}</IconBox>
                      <div className="mt-3 flex h-7 items-center">
                        {s.value === undefined ? <Skeleton className="h-5 w-16" /> : <p className="truncate text-xl font-medium tracking-tight">{s.value}</p>}
                      </div>
                      <p className="text-xs text-muted-foreground">{s.label}</p>
                      {s.spark && s.color && <Spark values={s.spark} color={s.color} />}
                    </div>
                  </CarouselItem>
                ))}
              </CarouselContent>
              <CarouselDots />
            </Carousel>
          </div>

              <Card
                ctx="chart"
                className="overflow-hidden"
                icon={<Activity className="size-4 text-muted-foreground" strokeWidth={1.5} />}
                title="Activity"
                hint="Chats you used each day"
                action={
                  <div className="flex items-center gap-3 pt-0.5 text-xs text-muted-foreground">
                    {shownTools.map((p) => (
                      <span key={p} className="flex items-center gap-1.5">
                        <span className="size-2 rounded-full" style={{ background: COLOR[p] }} /> {PROVIDER_SHORT[p]}
                      </span>
                    ))}
                  </div>
                }
              >
                <div className="pt-4">
                  {stats === null ? (
                    <Skeleton className="mx-4 mb-4 h-40 @min-[1080px]:h-52" />
                  ) : (
                    <ChartContainer config={chartConfig} className="h-40 w-full @min-[1080px]:h-52 [&_.recharts-cartesian-grid-horizontal_line:first-child]:stroke-transparent">
                      <AreaChart data={chartData} margin={{ left: 0, right: 0, top: 8, bottom: 0 }}>
                        <defs>
                          {/* The outline of a layer follows the filled shape exactly, and fades out wherever the layer has no chats, so empty layers never pile their lines on the edge of the one below. */}
                          {drawn.map((p) => (
                            <linearGradient key={`edge-${p}`} id={`edge-${p}`} x1="0" y1="0" x2="1" y2="0">
                              {chartData.map((row, i) => (
                                // One stop per day: solid where the layer has chats, clear where it has none, and a smooth
                                // fade in between, so a layer's line thins out as the layer does and the line below takes over.
                                <stop
                                  key={i}
                                  offset={`${chartData.length > 1 ? ((i / (chartData.length - 1)) * 100).toFixed(3) : 0}%`}
                                  stopColor={`var(--color-${p})`}
                                  stopOpacity={Number(row[p as keyof typeof row] ?? 0) > 0 ? 1 : 0}
                                />
                              ))}
                            </linearGradient>
                          ))}
                          {drawn.map((p) => (
                            <linearGradient key={p} id={`fill-${p}`} x1="0" y1="0" x2="0" y2="1">
                              <stop offset="5%" stopColor={`var(--color-${p})`} stopOpacity={0.6} />
                              <stop offset="95%" stopColor={`var(--color-${p})`} stopOpacity={0.05} />
                            </linearGradient>
                          ))}
                        </defs>
                        <CartesianGrid vertical={false} strokeDasharray="3 3" />
                        <XAxis dataKey="label" tickLine={false} axisLine={false} tickMargin={8} minTickGap={32} padding={{ left: 0, right: 0 }} tick={{ fontSize: 11 }} />
                        <ChartTooltip cursor={false} content={<ChartTooltipContent indicator="dot" />} />
                        {shownTools.map((p) =>
                          drawn.includes(p) ? (
                            <Area key={p} dataKey={p} type="monotone" stackId="a" stroke={`url(#edge-${p})`} strokeWidth={1.5} fill={`url(#fill-${p})`} />
                          ) : (
                            // No chats in these days: nothing to draw, but the tooltip still lists the tool with 0.
                            <Area key={p} dataKey={p} type="monotone" stackId="a" stroke={`var(--color-${p})`} strokeOpacity={0} fill={`var(--color-${p})`} fillOpacity={0} activeDot={false} isAnimationActive={false} />
                          ),
                        )}
                      </AreaChart>
                    </ChartContainer>
                  )}
                </div>
              </Card>

          <div className="grid gap-4 lg:grid-cols-5">
            <div className="space-y-4 lg:col-span-3">
              <Card
                ctx="recent"
                title="Pick up where you left off"
                action={
                  <Button variant="ghost" size="xs" className="text-muted-foreground" onClick={onSearch}>
                    Search all <ArrowUpRight className="size-3" />
                  </Button>
                }
              >
                <div className="space-y-0.5 p-2 pt-2">
                  {recent === null ? (
                    Array.from({ length: shown }).map((_, i) => <ChatRowSkeleton key={i} i={i} />)
                  ) : recent.length === 0 ? (
                    <div className="flex flex-col items-center gap-2 p-8 text-center">
                      <MessagesSquare className="size-8 text-muted-foreground" strokeWidth={1.25} />
                      <p className="text-sm font-medium">No Chats Found</p>
                      <p className="text-xs text-muted-foreground">Chats appear here once one of your AI tools has been used.</p>
                    </div>
                  ) : (
                    recent.slice(0, shown).map((c) => (
                      <ChatCard
                        key={`${c.provider}:${c.id}`}
                        chat={c}
                        title={niceTitle(titleOf(c))}
                        onOpen={() => onOpenChat(c)}
                        entries={rowEntries ? () => rowEntries(c) : undefined}
                        menu={
                          rowEntries && (
                            <Button
                              variant="ghost"
                              size="icon"
                              aria-label="Chat actions"
                              className="-my-1 size-7 shrink-0 text-muted-foreground opacity-0 transition-opacity group-hover:opacity-100 focus-visible:opacity-100"
                              onClick={(e) => {
                                e.stopPropagation();
                                // The three dots open the same menu as a right-click on the card, right under the button.
                                const r = e.currentTarget.getBoundingClientRect();
                                e.currentTarget.closest("[role=button]")?.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: r.left, clientY: r.bottom + 4 }));
                              }}
                            >
                              <MoreHorizontal className="size-4" />
                            </Button>
                          )
                        }
                      />
                    ))
                  )}
                </div>
              </Card>
            </div>

            <div className="space-y-4 lg:col-span-2">
              <Card ctx="storage" icon={<HardDrive className="size-4 text-muted-foreground" strokeWidth={1.5} />} title="Storage by tool" hint="Where the space on this PC goes">
                <div className="space-y-3 p-4 pt-3">
                  {stats === null ? (
                    <Skeleton className="h-16 w-full" />
                  ) : (
                    (() => {
                      const rows = shownTools.map((p) => ({ name: PROVIDER_SHORT[p], size: stats.sizes?.[p] ?? 0, color: COLOR[p] }));
                      return (
                        <>
                          <div className="flex h-2 gap-px overflow-hidden rounded-full bg-muted">
                            {/* Small tools get at least a few pixels so every tool with data shows in the bar. */}
                            {rows.filter((r) => r.size > 0).map((r) => (
                              <div key={r.name} className="shrink-0 first:rounded-l-full last:rounded-r-full" style={{ flexGrow: r.size, flexBasis: 0, minWidth: 6, background: r.color }} title={`${r.name} ${fmtSize(r.size)}`} />
                            ))}
                          </div>
                          <ul className="space-y-1.5">
                            {rows.map((r) => (
                              <li key={r.name} className="flex items-center justify-between text-sm">
                                <span className="flex items-center gap-2">
                                  <span className="size-2 rounded-full" style={{ background: r.color }} /> {r.name}
                                </span>
                                <span className="text-xs tabular-nums text-muted-foreground">{fmtSize(r.size)}</span>
                              </li>
                            ))}
                          </ul>
                        </>
                      );
                    })()
                  )}
                </div>
              </Card>
            </div>
          </div>

          <Card ctx="backup">
            <div className="flex flex-wrap items-center gap-x-6 gap-y-3 p-4">
              <div className="flex min-w-0 flex-1 basis-64 items-center gap-3">
                <IconBox>
                  <DatabaseBackup className="size-4 text-muted-foreground" strokeWidth={1.5} />
                </IconBox>
                <div className="min-w-0">
                  <p className="truncate text-sm font-medium">{backup?.lastSyncMs ? "Backup is up to date" : "Not backed up yet"}</p>
                  <p className="truncate text-xs text-muted-foreground" title={backup?.path}>
                    {backup?.path ?? "Copy your chats to Documents so they stay safe."}
                  </p>
                </div>
              </div>
              {backup && backup.files > 0 && (
                <div className="flex items-center gap-6 text-sm">
                  {[
                    { label: "Last synced", value: backup.lastSyncMs ? when(backup.lastSyncMs) : "Never" },
                    { label: "Files", value: backup.files.toLocaleString() },
                    { label: "Size", value: fmtSize(backup.bytes) },
                  ].map((x) => (
                    <div key={x.label}>
                      <p className="text-xs text-muted-foreground">{x.label}</p>
                      <p className="font-medium tabular-nums">{x.value}</p>
                    </div>
                  ))}
                </div>
              )}
              <Button size="sm" className="shrink-0" onClick={onSync}>
                Sync &amp; backup
              </Button>
            </div>
          </Card>

          <OverviewFooter />
        </div>
      </div>
    </FadedScroll>
    </ContextArea>
  );
}
