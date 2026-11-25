import type { Msg } from "@/chat-actions";

/**
 * Plain-words summaries of an AI's steps, the same way for every tool (Claude Code, Codex, Cursor, Gemini CLI,
 * OpenCode, Cline). Steps are sorted into families by what they do, read from the names and inputs those tools
 * actually record. A tool or version ChatVault does not know is still shown under its own name, so a new tool name
 * never hides a step.
 */

type Step = Msg & { result?: Msg };

export type Family = "read" | "edit" | "search" | "shell" | "web" | "browser" | "plan" | "agent" | "think" | "other";

const READ = /^(read|read_file|read_many_files|view|view_file|open_file|cat|notebookread|readfile)$/i;
const EDIT = /^(edit|multiedit|write|create|create_file|edit_file|str_replace|str_replace_editor|str_replace_based_edit_tool|apply_patch|patch|replace|write_file|write_to_file|replace_in_file|notebookedit|search_replace|insert_edit_into_file)$/i;
const SEARCH = /^(grep|glob|ls|list|list_dir|list_files|list_directory|search|search_files|grep_search|search_file_content|codebase_search|codebase_investigator|file_search|find|find_files|toolsearch)$/i;
const SHELL = /^(bash|shell|powershell|exec|exec_command|local_shell|shell_command|run_shell_command|execute_command|run_terminal_cmd|run_command|terminal|command|commandexecution)$/i;
const WEB = /^(webfetch|web_fetch|websearch|web_search|google_web_search|fetch|fetch_url|browse_web)$/i;
const PLAN = /(^|__)(update_plan|todowrite|todo_write|write_todos|plan)$/i;
const AGENT = /^(task|agent|subagent|spawn_agent|delegate)$/i;

const data = (m: Step) => (m.data && typeof m.data === "object" ? (m.data as Record<string, unknown>) : {});
const str = (v: unknown) => (typeof v === "string" ? v : "");

/** The tool name without an MCP server prefix ("mcp__server__tool" → "tool"). */
export function baseName(name: string) {
  const parts = name.split("__");
  return parts[parts.length - 1] || name;
}

/** Inner tools a Codex `exec`/`js` script calls (`tools.exec_command(...)`, `tools.apply_patch(...)`). */
function innerTools(m: Step): string[] {
  const code = str(data(m).cmd) || str(data(m).code) || str(data(m).input);
  return Array.from(code.matchAll(/tools\.([A-Za-z_]+)\s*\(/g), (x) => x[1]);
}

export function family(m: Step): Family {
  if (m.kind === "reasoning") return "think";
  if (m.kind !== "tool") return "other";
  const name = m.name ?? "";
  const base = baseName(name);
  if (PLAN.test(name)) return "plan";
  if (AGENT.test(base)) return "agent";
  if (data(m)._edit) return "edit";
  if (/^(exec|js)$/i.test(base)) {
    const inner = innerTools(m);
    if (inner.some((t) => /apply_patch|write|edit/i.test(t))) return "edit";
    if (inner.length === 0 || inner.some((t) => /exec|shell|command/i.test(t))) return "shell";
  }
  if (EDIT.test(base)) return "edit";
  if (READ.test(base)) return "read";
  if (SEARCH.test(base)) return "search";
  if (SHELL.test(base)) return "shell";
  if (WEB.test(base)) return "web";
  if (/browser|chrome|computer|navigate|screenshot|page|tab|click|windows-mcp/i.test(name)) return "browser";
  return "other";
}

/** The command a shell step ran, whatever the tool calls the field (Codex scripts included). */
export function shellCommand(m: Step): string {
  const d = data(m);
  const direct = str(d.command) || str(d.cmd) || str(d.CommandLine);
  if (direct && !/tools\.[A-Za-z_]+\s*\(/.test(direct)) return direct;
  const inner = direct.match(/cmd\s*:\s*"((?:[^"\\]|\\.)*)"/);
  if (inner) {
    try {
      return JSON.parse(`"${inner[1]}"`);
    } catch {
      return inner[1];
    }
  }
  const text = m.text.indexOf(": ") > 0 ? m.text.slice(m.text.indexOf(": ") + 2) : "";
  return direct || text;
}

/** The short description the AI gave the step ("Type-check the frontend"), when it gave one. */
export const stepDescription = (m: Step) => str(data(m)._desc) || str(data(m).description) || str(data(m).title);

export type EditInfo = { files: string[]; added: number; removed: number };
export function editInfo(m: Step): EditInfo | null {
  const e = data(m)._edit as EditInfo | undefined;
  if (e && Array.isArray(e.files)) return e;
  const p = str(data(m).file_path) || str(data(m).filePath) || str(data(m).path);
  return family(m) === "edit" && p ? { files: [p], added: 0, removed: 0 } : null;
}

/** The file a step read or wrote, for "Read legal.tsx". */
function fileOf(m: Step): string {
  const d = data(m);
  const p = str(d.file_path) || str(d.filePath) || str(d.path) || str(d.absolute_path) || str(d.target_file) || editInfo(m)?.files[0] || "";
  if (p) return p;
  const detail = m.text.indexOf(": ") > 0 ? m.text.slice(m.text.indexOf(": ") + 2) : "";
  return /[\\/]|\.\w{1,6}$/.test(detail) ? detail : "";
}

export const fileName = (p: string) => p.split(/[\\/]/).filter(Boolean).pop() ?? p;

export const failed = (m: Step) => m.result?.error === true || m.error === true;

/** "Edited 3 files", "ran 2 commands", ... in the order they first happened, plus totals for the header. */
export function summarize(steps: Step[]) {
  const order: Family[] = [];
  const by = new Map<Family, Step[]>();
  for (const s of steps) {
    const f = family(s);
    if (!by.has(f)) {
      by.set(f, []);
      order.push(f);
    }
    by.get(f)!.push(s);
  }
  const files = (list: Step[]) => Array.from(new Set(list.map(fileOf).filter(Boolean)));
  const count = (n: number, one: string, many: string) => (n === 1 ? one : many.replace("#", String(n)));
  const parts: string[] = [];
  for (const f of order) {
    const list = by.get(f)!;
    const fs = files(list);
    switch (f) {
      case "read":
        parts.push(fs.length === 1 ? `read ${fileName(fs[0])}` : count(fs.length || list.length, "read a file", "read # files"));
        break;
      case "edit": {
        const edited = Array.from(new Set(list.flatMap((s) => editInfo(s)?.files ?? []).concat(fs)));
        parts.push(edited.length === 1 ? `edited ${fileName(edited[0])}` : count(edited.length || list.length, "edited a file", "edited # files"));
        break;
      }
      case "search":
        parts.push("searched code");
        break;
      case "shell":
        parts.push(count(list.length, "ran a command", "ran # commands"));
        break;
      case "web":
        parts.push(list.some((s) => /search/i.test(s.name ?? "")) ? "searched the web" : count(list.length, "fetched a page", "fetched # pages"));
        break;
      case "browser":
        parts.push("used the browser");
        break;
      case "agent":
        parts.push(count(list.length, "ran an agent", "ran # agents"));
        break;
      case "plan":
        parts.push("updated the plan");
        break;
      case "other": {
        const names = Array.from(new Set(list.map((s) => prettyTool(s.name ?? s.text.split(":")[0]))));
        parts.push(names.length <= 2 ? `used ${names.join(" and ")}` : `used ${names.length} tools`);
        break;
      }
      case "think":
        break;
    }
  }
  if (!parts.length) parts.push(by.has("think") ? "thought" : count(steps.length, "1 step", "# steps"));
  const text = parts.join(", ");
  let added = 0;
  let removed = 0;
  for (const s of steps) {
    const e = editInfo(s);
    if (e) {
      added += e.added;
      removed += e.removed;
    }
  }
  return { text: text.charAt(0).toUpperCase() + text.slice(1), added, removed, failed: steps.filter(failed).length };
}

/** A tool name people can read: no MCP prefix, spaces instead of underscores. */
export function prettyTool(name: string) {
  return baseName(name).replace(/[_-]+/g, " ").trim() || "a tool";
}

// ---------------------------------------------------------------- cards

/** Steps the AI app draws as a card for you rather than as a tool step: the end-of-turn summary and questions. */
export const isCard = (m: Step) => m.kind === "tool" && /(^|__)wrap_up$|^(AskUserQuestion|request_user_input(_async)?|ask_user|ask_followup_question)$/i.test(m.name ?? "");

export type CardQuestion = { question: string; header?: string; options: { label: string; description?: string }[]; multi?: boolean };
export type Card = { kind: "next"; recap: string; ask: string; actions: string[]; blocked: boolean } | { kind: "questions"; questions: CardQuestion[]; answer: string };

export function cardOf(m: Step): Card | null {
  const d = data(m);
  if (/wrap_up$/i.test(m.name ?? "")) {
    const actions = (Array.isArray(d.actions) ? d.actions : []) as { label?: string; action?: string }[];
    const labels = actions.map((a) => a.label || (a.action ? a.action.replace(/_/g, " ").replace(/^./, (c) => c.toUpperCase()) : "")).filter(Boolean);
    const recap = str(d.recap);
    if (!recap && !str(d.ask)) return null;
    return { kind: "next", recap, ask: str(d.ask), actions: labels, blocked: d.blocked === true };
  }
  const raw = (Array.isArray(d.questions) ? d.questions : str(d.question) ? [{ question: str(d.question), options: d.options }] : []) as Record<string, unknown>[];
  const questions: CardQuestion[] = raw
    .map((q) => ({
      question: str(q.question) || str(q.text),
      header: str(q.header) || undefined,
      multi: q.multiSelect === true,
      options: (Array.isArray(q.options) ? q.options : []).map((o: unknown) =>
        typeof o === "string" ? { label: o } : { label: str((o as Record<string, unknown>).label), description: str((o as Record<string, unknown>).description) || undefined },
      ),
    }))
    .filter((q) => q.question);
  if (!questions.length) return null;
  return { kind: "questions", questions, answer: m.result?.text ?? "" };
}

// ---------------------------------------------------------------- plans

export type PlanStep = { text: string; status: "done" | "running" | "pending" };

const norm = (s: unknown): PlanStep["status"] => (s === "completed" || s === "done" ? "done" : s === "in_progress" || s === "running" || s === "active" ? "running" : "pending");

/**
 * The plan after this step, from the plan before it. Full lists replace it (Claude's `items`, Codex's `plan`, the
 * `todos` of TodoWrite and OpenCode); Claude's short updates (`advance`, `done`, `start`) move it forward.
 */
export function nextPlan(m: Step, prev: PlanStep[]): PlanStep[] {
  const d = data(m);
  const list = (d.items ?? d.plan ?? d.todos) as { content?: string; step?: string; text?: string; status?: string }[] | undefined;
  if (Array.isArray(list)) return list.map((t) => ({ text: t.content ?? t.step ?? t.text ?? "", status: norm(t.status) })).filter((t) => t.text);
  const next = prev.map((s) => ({ ...s }));
  if (d.advance === true) {
    const at = next.findIndex((s) => s.status === "running");
    if (at >= 0) {
      next[at].status = "done";
      const after = next.findIndex((s, i) => i > at && s.status === "pending");
      if (after >= 0) next[after].status = "running";
    }
  }
  if (Array.isArray(d.done)) for (const n of d.done as number[]) if (next[n - 1]) next[n - 1].status = "done";
  if (typeof d.start === "number" && next[d.start - 1]) {
    next.forEach((s) => s.status === "running" && (s.status = "pending"));
    next[d.start - 1].status = "running";
  }
  return next;
}
