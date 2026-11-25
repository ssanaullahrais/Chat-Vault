import { memo } from "react";
import { useStickyOpen } from "@/sticky-open";
import { Bot, Brain, FilePen, CircleCheck, CircleDashed, Circle, Globe, ListTodo, Terminal as TerminalIcon, Wrench, ArrowUpRight } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { CodeBlock, CodeBlockContent } from "@/components/ui/code-block";
import { Trace, TraceCollapsible, TraceCollapsibleContent, TraceCollapsibleTrigger, TraceDescription, TraceIcon, TraceLine, TraceTitle } from "@/components/ui/trace";
import { isFilePath, useFileView } from "@/file-viewer";
import { Highlight, textHas } from "@/highlight";
import { ColoredOutput, ShellLine, guessLanguage } from "@/shell-highlight";
import { hostname, uniqueUrls } from "@/ai-elements";
import { openUrl } from "@tauri-apps/plugin-opener";
import type { Msg } from "@/chat-actions";
import { cardOf, editInfo, failed, family, fileName, prettyTool, shellCommand, stepDescription, type PlanStep } from "@/step-summary";

export type Item = Msg & { result?: Msg };

/** Attaches every tool result to its call and drops the separate result messages. */
export function mergeResults(msgs: Msg[]): Item[] {
  const results = new Map<string, Msg>();
  for (const m of msgs) if (m.kind === "result" && m.id) results.set(m.id, m);
  return msgs.filter((m) => m.kind !== "result").map((m) => (m.kind === "tool" && m.id && results.has(m.id) ? { ...m, result: results.get(m.id) } : m));
}

export const isPlan = (m: Item) => family(m) === "plan";
export const isAgent = (m: Item) => family(m) === "agent";
const isShell = (m: Item) => family(m) === "shell";
const isWeb = (m: Item) => family(m) === "web";

const str = (v: unknown) => (typeof v === "string" ? v : Array.isArray(v) ? v.join(" ") : "");
const data = (m: Item) => (m.data ?? {}) as Record<string, unknown>;
const clipText = (s: string, n: number) => (s.length > n ? `${s.slice(0, n)}…` : s);

function splitText(text: string) {
  const k = text.indexOf(": ");
  return k > 0 ? [text.slice(0, k), text.slice(k + 2)] : [text, ""];
}

export const stepName = (m: Item) => m.name ?? (m.kind === "reasoning" ? "Thought" : splitText(m.text)[0]);

/** The task list or plan the AI kept, as it stood at the end of the turn, with how far it got. */
export function PlanBlock({ steps, memoKey }: { steps: PlanStep[]; memoKey?: string }) {
  const [open, setOpen] = useStickyOpen(memoKey, true);
  if (!steps.length) return null;
  const done = steps.filter((s) => s.status === "done").length;
  const running = steps.some((s) => s.status === "running");
  return (
    <Trace className="pl-11">
      <TraceCollapsible open={open} onOpenChange={setOpen}>
        <TraceCollapsibleTrigger>
          <TraceLine status={running ? "running" : undefined}>
            <TraceIcon>
              <ListTodo />
            </TraceIcon>
            <TraceTitle>Plan</TraceTitle>
            <TraceDescription>
              {done} of {steps.length} done
            </TraceDescription>
          </TraceLine>
        </TraceCollapsibleTrigger>
        <TraceCollapsibleContent>
          <Trace>
            {steps.map((s, i) => (
              <TraceLine key={i} status={s.status === "running" ? "running" : s.status === "pending" ? "pending" : undefined}>
                <TraceIcon>{s.status === "done" ? <CircleCheck /> : s.status === "running" ? <CircleDashed /> : <Circle />}</TraceIcon>
                <TraceTitle className={s.status === "done" ? "line-through" : undefined}>{s.text}</TraceTitle>
              </TraceLine>
            ))}
          </Trace>
        </TraceCollapsibleContent>
      </TraceCollapsible>
    </Trace>
  );
}

/**
 * A card the AI app drew for you: the end-of-turn "next step" box (recap, the question, its buttons) or a question
 * with options, with the answer when it is in the file. Buttons are shown as they appeared; they do nothing here.
 */
export function CardBlock({ item }: { item: Item }) {
  const card = cardOf(item);
  if (!card) return null;
  if (card.kind === "next") {
    return (
      <div className="pl-11">
        <div className="max-w-2xl rounded-xl border bg-card p-3.5 text-sm">
          {card.recap && <p className="text-muted-foreground">{card.recap}</p>}
          {card.ask && <p className={card.recap ? "mt-3 font-medium" : "font-medium"}>{card.ask}</p>}
          {card.actions.length > 0 && (
            <div className="mt-3 flex flex-wrap gap-2">
              {card.actions.map((a, i) => (
                <span key={a + i} className={i === 0 ? "rounded-md bg-primary px-2.5 py-1 text-xs font-medium text-primary-foreground" : "rounded-md bg-muted px-2.5 py-1 text-xs text-muted-foreground"}>
                  {a}
                </span>
              ))}
            </div>
          )}
        </div>
      </div>
    );
  }
  return (
    <div className="pl-11">
      <div className="max-w-2xl space-y-3 rounded-xl border bg-card p-3.5 text-sm">
        {card.questions.map((q, qi) => (
          <div key={qi}>
            {q.header && <p className="mb-0.5 text-[11px] font-medium uppercase tracking-wide text-muted-foreground">{q.header}</p>}
            <p className="font-medium">{q.question}</p>
            {q.options.length > 0 && (
              <ul className="mt-2 space-y-1">
                {q.options.map((o, oi) => {
                  const picked = !!card.answer && card.answer.includes(o.label);
                  return (
                    <li key={oi} className={picked ? "rounded-md border border-foreground/30 bg-muted/60 px-2.5 py-1.5" : "rounded-md border px-2.5 py-1.5"}>
                      <span className="flex items-center gap-1.5">
                        {picked && <CircleCheck className="size-3.5 shrink-0" />}
                        <span className={picked ? "font-medium" : undefined}>{o.label}</span>
                      </span>
                      {o.description && <span className="block text-xs text-muted-foreground">{o.description}</span>}
                    </li>
                  );
                })}
              </ul>
            )}
          </div>
        ))}
      </div>
    </div>
  );
}

/** A sub-agent run: what it was asked and what it reported back. */
export function SubagentBlock({ item, memoKey }: { item: Item; memoKey?: string }) {
  const [open, setOpen] = useStickyOpen(memoKey);
  const d = data(item);
  const title = str(d.description) || "Sub-agent";
  const kind = str(d.subagent_type);
  return (
    <Trace className="pl-11">
      <TraceCollapsible open={open} onOpenChange={setOpen}>
        <TraceCollapsibleTrigger>
          <TraceLine status={item.result?.error ? "error" : undefined}>
            <TraceIcon>
              <Bot />
            </TraceIcon>
            <TraceTitle className="truncate">Ran agent: {title}</TraceTitle>
            {kind && <Badge variant="outline">{kind}</Badge>}
          </TraceLine>
        </TraceCollapsibleTrigger>
        <TraceCollapsibleContent className="flex flex-col gap-3">
          {str(d.prompt) && (
            <CodeBlock size="sm">
              <CodeBlockContent data-lenis-prevent wrap className="max-h-48">
                {str(d.prompt)}
              </CodeBlockContent>
            </CodeBlock>
          )}
          {item.result?.text && (
            <CodeBlock size="sm">
              <CodeBlockContent data-lenis-prevent wrap className="max-h-64">
                {item.result.text}
              </CodeBlockContent>
            </CodeBlock>
          )}
        </TraceCollapsibleContent>
      </TraceCollapsible>
    </Trace>
  );
}

/** One tool step inside an expanded group. */
export const StepItem = memo(function StepItem({ item, q, memoKey }: { item: Item; q: string; memoKey?: string }) {
  const [open, setOpen] = useStickyOpen(memoKey);
  const files = useFileView();
  const [name, detail] = splitText(item.text);
  const d0 = data(item);
  const filePath = [d0.file_path, d0.filePath, d0.path, d0.absolute_path, d0.target_file].map(str).find((x) => x && isFilePath(x)) ?? (isFilePath(detail) ? detail : "");

  if (item.kind === "reasoning") {
    return (
      <Trace>
        <TraceCollapsible open={open || (!!q && textHas(item.text, q))} onOpenChange={setOpen}>
          <TraceCollapsibleTrigger>
            <TraceLine>
              <TraceIcon>
                <Brain />
              </TraceIcon>
              <TraceTitle>Thought</TraceTitle>
            </TraceLine>
          </TraceCollapsibleTrigger>
          <TraceCollapsibleContent className="whitespace-pre-wrap text-sm">
            <Highlight text={item.text.trim()} q={q} activeMark={false} />
          </TraceCollapsibleContent>
        </TraceCollapsible>
      </Trace>
    );
  }

  const desc = stepDescription(item);
  const bad = failed(item);
  const failedTag = bad ? <span className="shrink-0 text-xs text-red-600 dark:text-red-400">Failed</span> : null;

  if (isShell(item)) {
    const cmd = shellCommand(item) || detail;
    const out = item.result?.text ?? "";
    return (
      <Trace>
        <TraceCollapsible open={open} onOpenChange={setOpen}>
          <TraceCollapsibleTrigger>
            <TraceLine status={bad ? "error" : undefined}>
              <TraceIcon>
                <TerminalIcon />
              </TraceIcon>
              {/* The AI's own description of the command when it gave one ("Type-check the frontend"), else the command. */}
              {desc ? (
                <TraceTitle className="min-w-0 truncate">
                  <Highlight text={desc} q={q} activeMark={false} />
                </TraceTitle>
              ) : (
                <TraceTitle className="truncate font-mono text-xs">
                  Ran <ShellLine text={clipText(cmd.split("\n")[0], 90)} q={q} />
                </TraceTitle>
              )}
              {failedTag}
            </TraceLine>
          </TraceCollapsibleTrigger>
          <TraceCollapsibleContent>
            <CodeBlock size="sm">
              <CodeBlockContent data-lenis-prevent className="max-h-64">
                <span>
                  <span className="text-muted-foreground">$ </span>
                  <ShellLine text={cmd} />
                  {out ? "\n" : null}
                  {out ? <ColoredOutput code={out} language={guessLanguage(cmd)} /> : null}
                </span>
              </CodeBlockContent>
            </CodeBlock>
          </TraceCollapsibleContent>
        </TraceCollapsible>
      </Trace>
    );
  }

  if (isWeb(item)) {
    const d = data(item);
    const target = str(d.url) || str(d.query) || detail;
    const urls = Array.from(new Set([...(str(d.url) ? [str(d.url)] : []), ...uniqueUrls(item.result?.text ?? "")]));
    return (
      <Trace>
        <TraceCollapsible open={open} onOpenChange={setOpen}>
          <TraceCollapsibleTrigger>
            <TraceLine>
              <TraceIcon>
                <Globe />
              </TraceIcon>
              <TraceTitle className="truncate">
                {item.name === "WebSearch" ? "Searched" : "Fetched"} {clipText(item.name === "WebSearch" ? target : hostname(target), 70)}
              </TraceTitle>
              {urls.length > 0 && <TraceDescription>{urls.length} sources</TraceDescription>}
            </TraceLine>
          </TraceCollapsibleTrigger>
          <TraceCollapsibleContent>
            <Trace>
              {urls.slice(0, 12).map((u) => (
                <a
                  key={u}
                  href={u}
                  onClick={(e) => {
                    e.preventDefault();
                    openUrl(u).catch(() => {});
                  }}
                >
                  <TraceLine>
                    <TraceIcon>
                      <Globe />
                    </TraceIcon>
                    <TraceTitle>{hostname(u)}</TraceTitle>
                    <TraceIcon>
                      <ArrowUpRight />
                    </TraceIcon>
                  </TraceLine>
                </a>
              ))}
            </Trace>
          </TraceCollapsibleContent>
        </TraceCollapsible>
      </Trace>
    );
  }

  const edit = family(item) === "edit" ? editInfo(item) : null;
  if (edit) {
    const target = edit.files[0] ?? filePath;
    return (
      <Trace>
        <TraceLine status={bad ? "error" : undefined}>
          <TraceIcon>
            <FilePen />
          </TraceIcon>
          <TraceTitle className="shrink-0">{edit.files.length > 1 ? `Edited ${edit.files.length} files` : "Edited"}</TraceTitle>
          {target && edit.files.length <= 1 && (
            <button type="button" onClick={() => files.open(target)} className="min-w-0 cursor-pointer truncate text-left font-mono text-xs text-muted-foreground underline decoration-dotted underline-offset-2 hover:text-foreground" title={target}>
              <Highlight text={fileName(target)} q={q} activeMark={false} />
            </button>
          )}
          {(edit.added > 0 || edit.removed > 0) && (
            <span className="shrink-0 font-mono text-xs tabular-nums">
              <span className="text-emerald-600 dark:text-emerald-400">+{edit.added}</span> <span className="text-red-600 dark:text-red-400">−{edit.removed}</span>
            </span>
          )}
          {failedTag}
        </TraceLine>
      </Trace>
    );
  }

  return (
    <Trace>
      <TraceLine status={bad ? "error" : undefined}>
        <TraceIcon>
          <Wrench />
        </TraceIcon>
        <TraceTitle className="font-medium text-foreground/80">
          <Highlight text={desc || prettyTool(item.name ?? name)} q={q} activeMark={false} />
        </TraceTitle>
        {filePath ? (
          <button type="button" onClick={() => files.open(filePath)} className="min-w-0 cursor-pointer truncate text-left font-mono text-xs text-muted-foreground underline decoration-dotted underline-offset-2 hover:text-foreground">
            <Highlight text={filePath} q={q} activeMark={false} />
          </button>
        ) : (
          detail && (
            <TraceDescription className="truncate font-mono text-xs">
              <Highlight text={detail} q={q} activeMark={false} />
            </TraceDescription>
          )
        )}
      </TraceLine>
    </Trace>
  );
});
