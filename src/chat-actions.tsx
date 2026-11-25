import { toReaderHtml } from "@/reader-html";
import { launchCli } from "@/launch-cli";
import { invoke } from "@tauri-apps/api/core";
import { save } from "@tauri-apps/plugin-dialog";
import {
  Bot,
  Copy,
  ExternalLink,
  Save,
  FileCode,
  FileJson,
  FileText,
  FolderOpen,
  FolderTree,
  GitFork,
  Hash,
  Link2,
  MessageSquareText,
  Sparkles,
  Terminal,
  type LucideIcon,
} from "lucide-react";

export type Provider = "claude" | "codex" | "cursor" | "gemini" | "opencode" | "cline";
/** What the sidebar lists: one tool, or every tool together. */
export type Tool = Provider | "all";
const EVERY_PROVIDER: Provider[] = ["claude", "codex", "cursor", "gemini", "opencode", "cline"];
const EVERY_CORE: Provider[] = ["claude", "codex", "cursor"];
/** The tools ChatVault looks at. A tool switched off in Settings is left out, so every list and chart that reads this drops it. */
export const PROVIDERS: Provider[] = [...EVERY_PROVIDER];
/** The tools always shown on the overview. The others appear once ChatVault finds their chats. */
export const CORE_PROVIDERS: Provider[] = [...EVERY_CORE];
const switchedOff = new Set<Provider>();
/** Applies the list of tools switched off in Settings. The two arrays above are changed in place, so nothing else needs to know. */
export function setOffTools(list: string[]) {
  switchedOff.clear();
  list.forEach((p) => switchedOff.add(p as Provider));
  PROVIDERS.splice(0, PROVIDERS.length, ...EVERY_PROVIDER.filter((p) => !switchedOff.has(p)));
  CORE_PROVIDERS.splice(0, CORE_PROVIDERS.length, ...EVERY_CORE.filter((p) => !switchedOff.has(p)));
}
export const isToolOn = (p: Provider) => !switchedOff.has(p);
/** Tools whose chats cannot be resumed from here: they continue in Claude or Codex through a handoff file. */
export const isHandoffOnly = (p: Provider) => p !== "claude" && p !== "codex";
/**
 * How deleting a chat works. Most chats are a file or folder that goes to the Recycle Bin ("trash"). OpenCode keeps
 * its sessions in its own database, so OpenCode deletes the session itself ("opencode"). A chat kept inside another
 * app's database (Cursor's editor, Codex's database) cannot be removed safely from outside ("blocked"): Delete stays
 * visible but grayed, and says so when it is clicked.
 */
export type DeleteKind = "trash" | "opencode" | "blocked";
export const deleteKind = (c: { provider: Provider; path: string }): DeleteKind =>
  c.provider === "opencode" ? "opencode" : c.path.startsWith("cursor-ide:") || c.path.startsWith("codex-db:") ? "blocked" : "trash";
/** What a click on a grayed Delete tells the user. */
export const deleteBlockedMessage = (tool: string) =>
  `This chat is stored inside ${tool} itself, so ChatVault can't delete it. Delete it in ${tool}, or use Archive to hide it here.`;
export const PROVIDER_SHORT: Record<Provider, string> = { claude: "Claude", codex: "Codex", cursor: "Cursor", gemini: "Gemini", opencode: "OpenCode", cline: "Cline" };

/** One result of the app-wide search (the search_chats command): the chat, a short piece of text around the match, and whether the title matched. */
export type Found = { chat: Chat; snippet: string; inTitle: boolean };

export type Chat = {
  provider: Provider;
  id: string;
  localId: string | null;
  title: string;
  account: string;
  project: string;
  cwd: string | null;
  createdMs: number | null;
  lastMs: number;
  archived: boolean;
  size: number;
  path: string;
  model: string | null;
  hasMetadata: boolean;
  backup: boolean;
  userArchived?: boolean;
};
export type Msg = {
  role: "user" | "assistant";
  ts: string;
  text: string;
  kind: "text" | "tool" | "summary" | "image" | "result" | "reasoning";
  id?: string;
  name?: string;
  data?: Record<string, unknown> | string;
  error?: boolean;
  result?: Msg;
};
/** `partial` marks the quick first view of a chat's newest messages, before the full page and total are known. */
export type MsgPage = { messages: Msg[]; total: number; start: number; partial?: boolean };

export type AccountInfo = {
  id: string;
  orgs: number;
  desktopCurrent: boolean;
  cliCurrent: boolean;
  email: string | null;
  name: string | null;
  plan: string | null;
};

export const PROVIDER_NAME: Record<Provider, string> = { claude: "Claude Code", codex: "Codex", cursor: "Cursor", gemini: "Gemini CLI", opencode: "OpenCode", cline: "Cline" };

export const fmtDate = (ms: number) =>
  new Date(ms).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" });
/** Folder names read better as titles: "seven-jets_main" -> "Seven Jets Main". Names that already mix cases ("ChatVault") are left alone. */
export const prettyProject = (name: string) => {
  if (/[a-z][A-Z]/.test(name) || /\s/.test(name)) return name;
  return name
    .replace(/[-_.]+/g, " ")
    .trim()
    .split(" ")
    .map((w) => (w ? w[0].toUpperCase() + w.slice(1) : w))
    .join(" ");
};
/** Chat titles often start in lower case ("fix the login"): capitalize the first letter for display. */
export const prettyTitle = (t: string) => t.trimStart().replace(/^\p{Ll}/u, (c) => c.toUpperCase());
export const fmtSize = (b: number) =>
  b > 1073741824
    ? `${(b / 1073741824).toFixed(1)} GB`
    : b > 1048576
      ? `${(b / 1048576).toFixed(1)} MB`
      : `${Math.max(1, Math.round(b / 1024))} KB`;

let accountEmails: Record<string, string> = {};
let desktopAccounts: Set<string> | null = null;
export const setAccountNames = (list: AccountInfo[]) => {
  accountEmails = Object.fromEntries(list.filter((a) => a.email).map((a) => [a.id, a.email as string]));
  // The Claude app can only open chats that belong to the account it is signed in to right now.
  desktopAccounts = new Set(list.filter((a) => a.desktopCurrent).map((a) => a.id));
};
let customAccountNames: Record<string, string> = {};
/** The names you gave to accounts (see Rename account). */
export const setCustomAccountNames = (names: Record<string, string>) => {
  customAccountNames = names;
};
/** The email when it is known (Gemini and Cursor store it as the account itself), otherwise the first 8 characters of the account id. */
export const originalAccountLabel = (a: string) =>
  a === "unknown" ? "Unknown account" : a === "local" ? "This computer" : a.includes("@") ? a : (accountEmails[a] ?? `Account ${a.slice(0, 8)}`);
/** The name you gave the account, or else its email or id. */
export const accountLabel = (a: string) => customAccountNames[a] ?? originalAccountLabel(a);

/** "2026-10-18_05-30": the moment of the export, safe in a file name. */
const fileStamp = () => {
  const d = new Date();
  const p = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}_${p(d.getHours())}-${p(d.getMinutes())}`;
};

/** Asks where to save with the system Save dialog, then writes the file there. Returns false if the user cancels. */
async function saveWithDialog(name: string, content: string, toast: (m: string) => void, format: { ext: string; label: string } = { ext: "md", label: "Markdown" }) {
  const safe = name.replace(/[\/:*?"<>|]+/g, "_").replace(/\s+/g, " ").trim().slice(0, 80) || "chat";
  const path = await save({ defaultPath: `${safe}-${fileStamp()}.${format.ext}`, filters: [{ name: format.label, extensions: [format.ext] }] });
  if (!path) return false;
  await invoke("write_text_file", { path, content });
  toast("Saved");
  return true;
}

const clip = (s: string, n: number) => (s.length > n ? `${s.slice(0, n)}…` : s);

export function toMarkdown(chat: Chat, msgs: Msg[]) {
  const who = PROVIDER_SHORT[chat.provider];
  const lines = [
    `# ${chat.title}`,
    "",
    `- Tool: ${PROVIDER_NAME[chat.provider]}`,
    `- Account: ${accountLabel(chat.account)}`,
    `- Project: ${chat.project}`,
    `- Last activity: ${fmtDate(chat.lastMs)}`,
    "",
  ];
  for (const m of msgs) {
    if (m.kind !== "text" && m.kind !== "summary") continue;
    lines.push(`## ${m.role === "user" ? "You" : who} · ${m.ts ? fmtDate(Date.parse(m.ts)) : ""}`, "", m.text, "");
  }
  return lines.join("\n");
}

const textOf = (msgs: Msg[]) => msgs.filter((m) => m.kind === "text" || m.kind === "summary");

export function toPlainText(chat: Chat, msgs: Msg[]) {
  const who = PROVIDER_SHORT[chat.provider];
  const head = [chat.title, "=".repeat(Math.min(chat.title.length, 80)), `${PROVIDER_NAME[chat.provider]} · ${chat.project} · ${fmtDate(chat.lastMs)}`, ""];
  const body = textOf(msgs).flatMap((m) => [`${m.role === "user" ? "You" : who}${m.ts ? ` (${fmtDate(Date.parse(m.ts))})` : ""}:`, m.text, ""]);
  return [...head, ...body].join("\n");
}

export function toJson(chat: Chat, msgs: Msg[]) {
  const { path: _path, ...meta } = chat;
  return JSON.stringify({ chat: meta, messages: textOf(msgs).map((m) => ({ role: m.role, time: m.ts, text: m.text })) }, null, 2);
}

/** A self-contained reading page: opens in any browser, copies the Markdown back, and prints to PDF. */
export function toHtml(chat: Chat, msgs: Msg[]) {
  return toReaderHtml(chat.title, [PROVIDER_NAME[chat.provider], prettyProject(chat.project), fmtDate(chat.lastMs), `${textOf(msgs).length} messages`], toMarkdown(chat, msgs));
}

/** Extra file formats for "Save this chat", next to Markdown. */
function saveFormatActions(chat: Chat, toast: Toast, allMsgs: () => Promise<Msg[]>): ChatAction[] {
  const formats: { id: string; label: string; hint: string; icon: LucideIcon; ext: string; name: string; build: (c: Chat, m: Msg[]) => string }[] = [
    { id: "save-txt", label: "Save as .txt", hint: "Plain text, readable anywhere.", icon: FileText, ext: "txt", name: "Plain text", build: toPlainText },
    { id: "save-html", label: "Save as .html", hint: "A web page you can open in a browser or print to PDF.", icon: FileCode, ext: "html", name: "Web page", build: toHtml },
    { id: "save-json", label: "Save as .json", hint: "Structured data: role, time and text of every message.", icon: FileJson, ext: "json", name: "JSON", build: toJson },
  ];
  return formats.map((f) => ({
    id: f.id,
    label: f.label,
    hint: f.hint,
    icon: f.icon,
    group: "Save" as const,
    run: async () => {
      await saveWithDialog(chat.title, f.build(chat, await allMsgs()), toast, { ext: f.ext, label: f.name });
    },
  }));
}

/** Past this many characters a handoff is condensed: it is read in full on every start, so smaller is faster for any AI. */
const HANDOFF_LIMIT = 60_000;

/** A single file an AI agent can read to pick the work up where the chat stopped. */
export const handoffIsCondensed = (msgs: Msg[]) => msgs.filter((m) => m.kind === "text" || m.kind === "summary").reduce((n, m) => n + m.text.length, 0) > HANDOFF_LIMIT;

/** The user's standing instructions (always..., never..., don't..., make sure...), picked out of their own messages, newest last. */
export function extractRules(msgs: Msg[], max = 25) {
  const cue = /\b(always|never|don'?t|do not|dont|make sure|must|should(?:n'?t)?|prefer|remember|instead of|rather than)\b/i;
  const seen = new Set<string>();
  const found: string[] = [];
  for (const m of msgs) {
    if (m.role !== "user" || m.kind !== "text") continue;
    for (const raw of m.text.split(/(?<=[.!?])\s+|\n+/)) {
      const t = raw.replace(/\s+/g, " ").replace(/^[-*>\s"]+/, "").trim();
      if (t.length < 12 || t.length > 220 || t.endsWith("?") || !cue.test(t)) continue;
      const key = t.toLowerCase();
      if (seen.has(key)) continue;
      seen.add(key);
      found.push(t);
    }
  }
  return found.slice(-max);
}

/** `fullPath`: the complete conversation as its own file, for a condensed handoff to point at. */
export function buildHandoff(chat: Chat, msgs: Msg[], fullPath?: string) {
  const text = msgs.filter((m) => m.kind === "text" || m.kind === "summary");
  const line = (m: Msg, max: number) =>
    `### ${m.role === "user" ? "User" : "Assistant"}${m.kind === "summary" ? " (auto summary of earlier work)" : ""} · ${m.ts ? fmtDate(Date.parse(m.ts)) : ""}

${clip(m.text, max)}
`;

  const total = text.reduce((n, m) => n + m.text.length, 0);
  let body: string[];
  let note = "This is the full conversation.";
  if (total <= HANDOFF_LIMIT) {
    body = text.map((m) => line(m, 8000));
  } else {
    const head = text.slice(0, 2);
    const tail = text.slice(-24);
    const middle = text.slice(2, -24).filter((m) => m.role === "user").slice(-120);
    note = "This chat was long, so it is condensed: the first 2 messages and the last 24 are in full, and the middle lists only what the user asked, one line each.";
    body = [
      ...head.map((m) => line(m, 2500)),
      "### Middle of the conversation (user requests only)\n",
      ...middle.map((m) => `- ${clip(m.text.replace(/\s+/g, " "), 160)}`),
      "",
      ...tail.map((m) => line(m, 3500)),
    ];
  }

  // What an AI needs first, so it can act without reading anything else.
  const lastUser = [...text].reverse().find((m) => m.role === "user");
  const lastAi = [...text].reverse().find((m) => m.role === "assistant");
  const stopped = [
    "## Where we stopped",
    "",
    "Last request from the user:",
    "",
    lastUser ? clip(lastUser.text, 2000) : "(none)",
    "",
    "Last reply from the assistant:",
    "",
    lastAi ? clip(lastAi.text, 2000) : "(none)",
    "",
  ];

  const rules = extractRules(text);
  const rulesSection = ["## Rules and preferences (the user's own words)", "", ...(rules.length ? rules.map((r) => `- ${r}`) : ["- None stated."]), ""];

  return [
    `# Handoff: ${chat.title}`,
    "",
    `Work started in a ${PROVIDER_NAME[chat.provider]} chat, possibly under another account. Carry on from "Where we stopped", follow the rules below, and do not redo finished work. Check git status and recent commits before changing code.`,
    "",
    `- Project folder: ${chat.cwd ?? "unknown"}`,
    ...originalFileLines(chat),
    `- Last activity: ${fmtDate(chat.lastMs)}`,
    `- ${note}`,
    ...(fullPath ? [`- Full conversation, every message, searchable: ${fullPath} (search it for an earlier detail instead of guessing; do not read it whole)`] : []),
    "",
    ...stopped,
    ...rulesSection,
    "## Conversation",
    "",
    ...body,
    "## Your task now",
    "",
    "Continue from Where we stopped, following the rules above. First check git status and recent commits, then say in one line where we stopped, then carry on.",
    "",
  ].join("\n");
}

/** The chat's own transcript file, only when the original is there. A backup copy, a database-only chat and a compressed file are not worth pointing an AI at: it would hunt for a file it can't use. */
/** Cursor, Gemini and the other tools keep chats in databases: their "path" is a label like "cursor:...", not a file. */
const isFilePath = (path: string) => !/^[a-z][a-z0-9-]+:/i.test(path);
export const originalPath = (chat: Chat) => (!chat.backup && isFilePath(chat.path) && !/\.(zst|gz|zip)$/i.test(chat.path) ? chat.path : null);
const originalFileLines = (chat: Chat) => {
  const o = originalPath(chat);
  return o ? [`- Original transcript file: ${o} (complete log, large: open it only if the user asks for an exact detail this file lacks)`] : [];
};
/** Appended to every prompt that points an AI at a handoff file. */
const sourceNote = (chat: Chat) => {
  const o = originalPath(chat);
  return o ? ` The original log (${fmtSize(chat.size)}, open it only if I ask for an exact detail): ${o}.` : "";
};
/**
 * The one prompt every tool is given, whether ChatVault starts it ("Continue in …"), you copy a start command, or you
 * copy the prompt itself. It points at the handoff file and, when there is one, the original transcript.
 */
export const buildPrompt = (chat: Chat, path: string) => {
  const title = chat.title.replace(/\s+/g, " ").replace(/["\u201c\u201d]/g, "").trim();
  const where = chat.cwd ? ` Project folder: ${chat.cwd}.` : "";
  return (
    `Read ${path} in full: the handoff of an earlier ${PROVIDER_NAME[chat.provider]} chat (${title}).${where} ` +
    "Continue from its Where we stopped section, follow its Rules and preferences, and do not redo finished work. " +
    "Check git status and recent commits first, so you build on what is already done. " +
    `If it names a full conversation file, search that file for any earlier detail rather than guessing.${sourceNote(chat)} ` +
    "Begin with one line saying where we stopped."
  );
};
/** Inside a single-quoted PowerShell string a quote is doubled. */
const psq = (t: string) => t.replace(/'/g, "''");
/** Moves PowerShell into the chat's folder first. A literal path, so spaces, `$` and quotes in folder names are safe. */
const cdTo = (chat: Chat) => (chat.cwd ? `Set-Location -LiteralPath '${psq(chat.cwd)}'; ` : "");

export const loadAll = (chat: Chat) => invoke<Msg[]>("read_chat_all", { provider: chat.provider, path: chat.path });

/**
 * Saves what a new AI reads. A short chat goes in whole. A long one gets a compact briefing plus the complete conversation
 * as a second, searchable file, so the AI starts fast and can still look up anything that was said.
 */
async function prepareHandoff(chat: Chat) {
  const msgs = await loadAll(chat);
  const fullPath = handoffIsCondensed(msgs) ? await invoke<string>("save_handoff", { title: `full ${chat.title}`, content: toMarkdown(chat, msgs) }) : undefined;
  return invoke<string>("save_handoff", { title: chat.title, content: buildHandoff(chat, msgs, fullPath) });
}

/** The arguments that start a tool on a fresh handoff of this chat. */
const handoffArgs = async (chat: Chat) => {
  const handoff = await prepareHandoff(chat);
  return { handoff, prompt: buildPrompt(chat, handoff) };
};

const codexHandoffCommand = (path: string, chat: Chat) => `codex '${psq(buildPrompt(chat, path))}'`;
const claudeHandoffCommand = (path: string, chat: Chat) => {
  const folder = path.replace(/[\\/][^\\/]*$/, "");
  // The prompt goes first: --add-dir takes any number of values, so a prompt after it would be read as one more folder and dropped.
  return `claude '${psq(buildPrompt(chat, path))}' --add-dir '${psq(folder)}'`;
};

/** "Copy" actions that put chat content on the clipboard, as opposed to the "start this tool" commands. */
export const COPY_CONTENT_IDS = ["copy-prompt", "copy-md", "copy-path", "copy-id"];

/**
 * What you reach for when you want another AI to pick a chat up with all its memory: a prompt that points it at the full
 * transcript, and the transcript's file path.
 */
function contextActions(chat: Chat, toast: Toast): ChatAction[] {
  // The clipboard can refuse a write that comes after a long wait; a hidden text box is the fallback.
  const copy = async (text: string, what: string) => {
    try {
      await navigator.clipboard.writeText(text);
    } catch {
      const box = document.createElement("textarea");
      box.value = text;
      box.style.position = "fixed";
      box.style.opacity = "0";
      document.body.appendChild(box);
      box.select();
      const ok = document.execCommand("copy");
      box.remove();
      if (!ok) throw new Error("Could not copy to the clipboard");
    }
    toast(`${what} copied`);
  };
  const out: ChatAction[] = [
    {
      id: "copy-prompt",
      label: "Copy prompt",
      hint: "Saves the whole chat as a handoff file and copies a prompt you can paste into any AI. It tells the AI to read the file completely and carry on with the same memory.",
      icon: MessageSquareText,
      group: "Copy",
      run: async () => {
        await copy(buildPrompt(chat, await prepareHandoff(chat)), "Prompt");
      },
    },
    ...(isFilePath(chat.path)
      ? [
          {
            id: "copy-path",
            label: "Copy file path",
            hint: "Copies the path of this chat's transcript file.",
            icon: Link2,
            group: "Copy" as const,
            run: () => copy(chat.path, "File path"),
          },
        ]
      : []),
  ];
  return out;
}

/** Every tool this chat can be continued in, its own included. One list for the sidebar menu, the right-click menu and the chat header. */
export const continueTargets = (actions: ChatAction[]) =>
  actions.filter((a) => a.id.startsWith("continue-")).map((a) => ({ ...a, logo: a.id.slice("continue-".length) as Provider }));

export type ChatAction = {
  id: string;
  label: string;
  hint: string;
  icon: LucideIcon;
  /** Show this tool's official logo instead of the icon. */
  logo?: Provider;
  group: "Chat" | "Continue" | "Open" | "Copy" | "Save";
  /** 1 and 2 are the two buttons shown in the chat header. */
  slot?: 1 | 2;
  danger?: boolean;
  /** Shown grayed: the action is listed but not available for this chat; clicking it explains why. */
  muted?: boolean;
  run: () => Promise<void>;
};

type Toast = (m: string) => void;

/** Where a chat can be continued from. Every tool can take over any chat through a handoff file with the whole conversation. */
const TOOLS: { id: Provider; name: string; command: (path: string, chat: Chat) => string; icon: LucideIcon }[] = [
  { id: "claude", name: "Claude", command: (path, chat) => claudeHandoffCommand(path, chat), icon: Sparkles },
  { id: "codex", name: "Codex", command: (path, chat) => codexHandoffCommand(path, chat), icon: Bot },
  { id: "gemini", name: "Gemini CLI", command: (path, chat) => `gemini --prompt-interactive '${psq(buildPrompt(chat, path))}' --include-directories '${psq(path.replace(/[\\/][^\\/]*$/, ""))}'`, icon: Bot },
  { id: "cursor", name: "Cursor Agent", command: (path, chat) => `agent '${psq(buildPrompt(chat, path))}'`, icon: Bot },
  { id: "opencode", name: "OpenCode", command: (path, chat) => `opencode --prompt '${psq(buildPrompt(chat, path))}'`, icon: Bot },
  { id: "cline", name: "Cline", command: (path, chat) => `cline --tui --auto-approve false '${psq(buildPrompt(chat, path))}'`, icon: Bot },
];

/** "Continue in <tool>" and "Copy <tool> command" for every tool the chat is not already from. */
function handoffActions(chat: Chat, toast: Toast, skip: Provider[]): ChatAction[] {
  const cd = cdTo(chat);
  const out: ChatAction[] = [];
  TOOLS.filter((t) => t.id !== chat.provider && !skip.includes(t.id) && isToolOn(t.id)).forEach((t) => {
    out.push({
      id: `continue-${t.id}`,
      label: `Continue in ${t.name}`,
      hint: `Saves a handoff file with this chat and starts ${t.name} with it, so it keeps the context.`,
      icon: t.icon,
      group: "Continue",
      run: async () => {
        const handoff = await prepareHandoff(chat);
        await launchCli({ agent: `${t.id}-handoff`, id: chat.id, cwd: chat.cwd, handoff, prompt: buildPrompt(chat, handoff) });
        toast(`Starting ${t.name} in a terminal`);
      },
    });
    out.push({
      id: `copy-${t.id}`,
      label: `Copy ${t.name} command`,
      hint: `Saves the handoff file and copies the command that starts ${t.name} with it.`,
      icon: Terminal,
      logo: t.id,
      group: "Copy",
      run: async () => {
        const handoff = await prepareHandoff(chat);
        await navigator.clipboard.writeText(`${cd}${t.command(handoff, chat)}`);
        toast("Command copied");
      },
    });
  });
  return out;
}


export function chatActions(chat: Chat, toast: Toast, msgs?: Msg[] | null): ChatAction[] {
  if (isHandoffOnly(chat.provider)) return cursorActions(chat, toast, msgs);
  const isClaude = chat.provider === "claude";
  const cd = cdTo(chat);
  // Always the whole chat from its file: the messages loaded on screen are only the newest page of it.
  const allMsgs = () => loadAll(chat);
  const copy = async (text: string, what: string) => {
    await navigator.clipboard.writeText(text);
    toast(`${what} copied`);
  };
  const launch = (agent: string, extra: Record<string, unknown> = {}) =>
    launchCli({ agent, id: chat.id, cwd: chat.cwd, ...extra });

  const continueClaude: ChatAction = isClaude
    ? {
        id: "continue-claude",
        label: "Continue in Claude",
        hint: "Starts a new Claude Code session with the whole chat as a handoff file, under the account signed in now. It never reopens the old session, so a used-up limit can't follow it.",
        icon: Sparkles,
        group: "Continue",
        slot: 1,
        run: async () => {
          await launch("claude-handoff", await handoffArgs(chat));
          toast("Starting Claude Code in a terminal");
        },
      }
    : {
        id: "continue-claude",
        label: "Continue in Claude",
        hint: "Saves a handoff file with this Codex chat and starts Claude Code with it, so Claude keeps the context.",
        icon: Sparkles,
        group: "Continue",
        slot: 2,
        run: async () => {
          const handoff = await prepareHandoff(chat);
          await launch("claude-handoff", { handoff, prompt: buildPrompt(chat, handoff) });
          toast("Starting Claude Code in a terminal");
        },
      };
  const continueCodex: ChatAction = isClaude
    ? {
        id: "continue-codex",
        label: "Continue in Codex",
        hint: "Saves a handoff file with this Claude chat and starts Codex with it, so Codex keeps the context.",
        icon: Bot,
        group: "Continue",
        slot: 2,
        run: async () => {
          const handoff = await prepareHandoff(chat);
          await launch("codex-handoff", { handoff, prompt: buildPrompt(chat, handoff) });
          toast("Starting Codex in a terminal");
        },
      }
    : {
        id: "continue-codex",
        label: "Continue in Codex",
        hint: "Starts a new Codex session with the whole chat as a handoff file, under the account signed in now. It never reopens the old session.",
        icon: Bot,
        group: "Continue",
        slot: 1,
        run: async () => {
          await launch("codex-handoff", await handoffArgs(chat));
          toast("Starting Codex in a terminal");
        },
      };

  const list: ChatAction[] = [
    continueClaude,
    continueCodex,
    {
      id: "fork",
      label: "Fork chat",
      hint: chat.path.startsWith("codex-db:") ? "Opens Codex's fork command for this chat in a terminal." : "Makes a copy of this chat that appears in the list right away. The original is left untouched.",
      icon: GitFork,
      group: "Continue",
      run: async () => {
        if (chat.path.startsWith("codex-db:")) {
          await launch("codex-fork");
          toast("Starting Codex fork in a terminal");
          return;
        }
        const f = await invoke<{ id: string; path: string }>("fork_chat", { provider: chat.provider, path: chat.path });
        window.dispatchEvent(new CustomEvent("chatvault:forked", { detail: { from: chat, id: f.id, path: f.path } }));
        toast("Forked. The copy is in your list.");
      },
    },
  ];
  if (isClaude && chat.localId && (!desktopAccounts || desktopAccounts.size === 0 || desktopAccounts.has(chat.account))) {
    list.push({
      id: "open",
      label: "Open in Claude app",
      hint: "Works only when the account that owns this chat is signed in to the Claude app.",
      icon: ExternalLink,
      group: "Open",
      run: () => invoke("open_in_claude", { localId: chat.localId }),
    });
  }
  list.push({
    id: "reveal",
    label: "Show transcript file",
    hint: "Shows the transcript file in File Explorer.",
    icon: FolderOpen,
    group: "Open",
    run: () => invoke("reveal_in_folder", { path: chat.path }),
  });
  if (chat.cwd) {
    list.push({
      id: "folder",
      label: "Open project folder",
      hint: "Opens the folder this chat worked in.",
      icon: FolderTree,
      group: "Open",
      run: () => invoke<void>("open_folder", { path: chat.cwd }),
    });
  }
  list.push(
    {
      id: "copy-id",
      label: "Copy chat ID",
      hint: "Copies the session id, useful for scripts and support.",
      icon: Hash,
      group: "Copy",
      run: () => copy(chat.id, "Chat ID"),
    },
    {
      id: "copy-native",
      label: isClaude ? "Copy Claude command" : "Copy Codex command",
      hint: "Saves the handoff file and copies the command that starts a new session of this tool with it.",
      icon: Terminal,
      logo: isClaude ? "claude" : "codex",
      group: "Copy",
      run: async () => {
        const handoff = await prepareHandoff(chat);
        await copy(`${cd}${isClaude ? claudeHandoffCommand(handoff, chat) : codexHandoffCommand(handoff, chat)}`, "Command");
      },
    },
    {
      id: "copy-other",
      label: isClaude ? "Copy Codex command" : "Copy Claude command",
      hint: "Saves the handoff file and copies the command that starts the other tool with it.",
      icon: Terminal,
      logo: isClaude ? "codex" : "claude",
      group: "Copy",
      run: async () => {
        const handoff = await prepareHandoff(chat);
        await copy(`${cd}${isClaude ? codexHandoffCommand(handoff, chat) : claudeHandoffCommand(handoff, chat)}`, "Command");
      },
    },
    {
      id: "copy-md",
      label: "Copy as Markdown",
      hint: "Copies the readable conversation.",
      icon: Copy,
      group: "Copy",
      run: async () => copy(toMarkdown(chat, await allMsgs()), "Chat"),
    },
    {
      id: "save-md",
      label: "Save as .md",
      hint: "Choose where to save the conversation.",
      icon: Save,
      group: "Save",
      run: async () => {
        await saveWithDialog(chat.title, toMarkdown(chat, await allMsgs()), toast);
      },
    },
    ...saveFormatActions(chat, toast, allMsgs),
    {
      id: "save-handoff",
      label: "Save handoff file",
      hint: "Choose where to save a file another AI can read to continue this chat.",
      icon: FileText,
      group: "Save",
      run: async () => {
        await saveWithDialog(`${chat.title} - handoff`, buildHandoff(chat, await allMsgs()), toast);
      },
    },
  );
  // Continue the same chat in the other tools too: Gemini CLI, Cursor Agent, OpenCode and Cline.
  const extra = handoffActions(chat, toast, ["claude", "codex"]);
  const at = list.findIndex((x) => x.id === "fork") + 1;
  list.splice(at, 0, ...extra.filter((x) => x.group === "Continue"));
  const copyAt = list.findIndex((x) => x.id === "copy-other") + 1;
  list.splice(copyAt > 0 ? copyAt : list.length, 0, ...extra.filter((x) => x.group === "Copy"));
  list.splice(list.findIndex((x) => x.id === "copy-id"), 0, ...contextActions(chat, toast));
  return list;
}

/** Cursor chats have no command-line resume, so they continue in Claude Code or Codex through a handoff file. */
function cursorActions(chat: Chat, toast: Toast, _msgs?: Msg[] | null): ChatAction[] {
  const cd = cdTo(chat);
  // Always the whole chat from its file: the messages loaded on screen are only the newest page of it.
  const allMsgs = () => loadAll(chat);
  const copy = async (text: string, what: string) => {
    await navigator.clipboard.writeText(text);
    toast(`${what} copied`);
  };
  const handoffRun = (agent: "claude-handoff" | "codex-handoff", name: string) => async () => {
    const handoff = await prepareHandoff(chat);
    await launchCli({ agent, id: chat.id, cwd: chat.cwd, handoff, prompt: buildPrompt(chat, handoff) });
    toast(`Starting ${name} in a terminal`);
  };
  const list: ChatAction[] = [
    {
      id: "continue-claude",
      label: "Continue in Claude",
      hint: "Saves a handoff file with this Cursor chat and starts Claude Code with it.",
      icon: Sparkles,
      group: "Continue",
      slot: 1,
      run: handoffRun("claude-handoff", "Claude Code"),
    },
    {
      id: "continue-codex",
      label: "Continue in Codex",
      hint: "Saves a handoff file with this Cursor chat and starts Codex with it.",
      icon: Bot,
      group: "Continue",
      slot: 2,
      run: handoffRun("codex-handoff", "Codex"),
    },
  ];
  if (chat.cwd) {
    list.push(
      {
        id: "open-cursor",
        label: "Open project in Cursor",
        hint: "Opens this chat's project folder in Cursor.",
        icon: ExternalLink,
        group: "Open",
        run: () => invoke<void>("open_in_cursor", { path: chat.cwd }),
      },
      {
        id: "folder",
        label: "Open project folder",
        hint: "Opens the folder this chat worked in.",
        icon: FolderTree,
        group: "Open",
        run: () => invoke<void>("open_folder", { path: chat.cwd }),
      },
    );
  }
  list.push(
    {
      id: "reveal",
      label: "Show Cursor data file",
      hint: "Shows the file Cursor keeps this chat in.",
      icon: FolderOpen,
      group: "Open",
      run: () => invoke("reveal_in_folder", { path: chat.path }),
    },
    { id: "copy-id", label: "Copy chat ID", hint: "Copies the chat id.", icon: Hash, group: "Copy", run: () => copy(chat.id, "Chat ID") },
    {
      id: "copy-claude",
      label: "Copy Claude command",
      hint: "Saves the handoff file and copies the command that starts Claude Code with it.",
      icon: Terminal,
      logo: "claude",
      group: "Copy",
      run: async () => copy(`${cd}${claudeHandoffCommand(await prepareHandoff(chat), chat)}`, "Command"),
    },
    {
      id: "copy-codex",
      label: "Copy Codex command",
      hint: "Saves the handoff file and copies the command that starts Codex with it.",
      icon: Terminal,
      logo: "codex",
      group: "Copy",
      run: async () => copy(`${cd}${codexHandoffCommand(await prepareHandoff(chat), chat)}`, "Command"),
    },
    { id: "copy-md", label: "Copy as Markdown", hint: "Copies the readable conversation.", icon: Copy, group: "Copy", run: async () => copy(toMarkdown(chat, await allMsgs()), "Chat") },
    {
      id: "save-md",
      label: "Save as .md",
      hint: "Choose where to save the conversation.",
      icon: Save,
      group: "Save",
      run: async () => {
        await saveWithDialog(chat.title, toMarkdown(chat, await allMsgs()), toast);
      },
    },
    ...saveFormatActions(chat, toast, allMsgs),
    {
      id: "save-handoff",
      label: "Save handoff file",
      hint: "Choose where to save a file another AI can read to continue this chat.",
      icon: FileText,
      group: "Save",
      run: async () => {
        await saveWithDialog(`${chat.title} - handoff`, buildHandoff(chat, await allMsgs()), toast);
      },
    },
  );
  const others = handoffActions(chat, toast, ["claude", "codex"]);
  const fork = list.findIndex((x) => x.id === "continue-codex") + 1;
  list.splice(fork, 0, ...others.filter((x) => x.group === "Continue"));
  const cc = list.findIndex((x) => x.id === "copy-codex") + 1;
  list.splice(cc > 0 ? cc : list.length, 0, ...others.filter((x) => x.group === "Copy"));
  list.splice(list.findIndex((x) => x.id === "copy-id"), 0, ...contextActions(chat, toast));
  return list;
}
