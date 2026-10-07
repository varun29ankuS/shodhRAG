// Shodh Code mode guard, loaded into omp with `--hook` (see code_mode.rs).
//
// omp has no setting that keeps its file tools inside one folder, so every
// tool call passes through this `tool_call` handler first:
// - only the Code mode tools are allowed; any other tool is blocked;
// - every path a tool would touch must resolve (through symlinks and
//   junctions) inside the code folder; URLs and omp's internal resources
//   (`local://`, `ssh://`, `proc://`, web pages, ...) are blocked;
// - edits, writes and shell commands wait for the user's decision, asked
//   through omp's dialog channel and answered by Shodh's approval prompt.
// A handler error blocks the call (omp fails closed on `tool_call` errors).
//
// Shell commands cannot be confined by path: the shell keeps the user's
// access. They are only approved one command at a time.

import { existsSync, realpathSync } from "node:fs";
import * as path from "node:path";

export const ROOT_ENV = "SHODH_CODE_ROOT";
export const APPROVAL_TITLE = "shodh-approval";
export const GUARD_COMMAND = "shodh-guard";
export const ALLOW = "allow";

const SINGLE_PATH_TOOLS = new Set(["read", "write", "edit"]);
const PATH_LIST_TOOLS = new Set(["grep", "glob", "ast_grep"]);
const CHANGE_TOOLS = new Set(["edit", "write", "bash"]);

const isWindows = process.platform === "win32";

function comparable(p) {
  const normal = path.normalize(p);
  return isWindows ? normal.toLowerCase() : normal;
}

/** Whether `target` is `root` or inside it (both already resolved). */
export function isWithin(root, target) {
  const r = comparable(root);
  const t = comparable(target);
  if (t === r) return true;
  const prefix = r.endsWith(path.sep) ? r : r + path.sep;
  return t.startsWith(prefix);
}

/**
 * `p` with its longest existing ancestor resolved through symlinks and
 * junctions; the part that does not exist yet is appended unchanged.
 */
export function resolveExisting(p) {
  let existing = path.resolve(p);
  const missing = [];
  while (!existsSync(existing)) {
    const parent = path.dirname(existing);
    if (parent === existing) return null;
    missing.unshift(path.basename(existing));
    existing = parent;
  }
  return path.join(realpathSync.native(existing), ...missing);
}

/** The file part of a tool path: without a copied `[path#TAG]` wrapper or a `:selector` suffix. */
export function filePart(entry) {
  let text = entry;
  const wrapped = /^\[(.+)#[0-9A-Fa-f]{4}\]$/.exec(text);
  if (wrapped) text = wrapped[1];
  const drive = /^[A-Za-z]:/.test(text) ? 2 : 0;
  const colon = text.indexOf(":", drive);
  return colon >= 0 ? text.slice(0, colon) : text;
}

/** Why `raw` may not be used, or null when it stays inside `root`. */
export function pathProblem(root, cwd, raw) {
  if (typeof raw !== "string") return "A path must be text.";
  const entry = raw.trim();
  if (entry === "") return null;
  if (entry.includes("://") || /^[A-Za-z][A-Za-z0-9+.-]*:\/\//.test(entry)) {
    return `"${entry}" is a URL or internal resource; Code mode only works with files in the code folder.`;
  }
  if (entry.startsWith("~")) return `"${entry}" is outside the code folder.`;
  if (entry.startsWith("\\\\") || entry.startsWith("//")) return `"${entry}" is a network path outside the code folder.`;
  const resolved = resolveExisting(path.resolve(cwd, filePart(entry)));
  if (resolved === null || !isWithin(root, resolved)) return `"${entry}" is outside the code folder.`;
  return null;
}

function listProblem(root, cwd, value) {
  if (value === undefined || value === null) return null;
  if (typeof value !== "string") return "A path must be text.";
  for (const entry of value.split(";")) {
    const problem = pathProblem(root, cwd, entry);
    if (problem) return problem;
  }
  return null;
}

/** Why the call may not run, or null when it stays inside `root`. */
export function toolCallProblem(root, cwd, tool, input) {
  const args = input && typeof input === "object" ? input : {};
  if (SINGLE_PATH_TOOLS.has(tool)) {
    if (tool === "edit" && args.input !== undefined) {
      return "Only single-file replace edits are available in Code mode.";
    }
    if (typeof args.path !== "string" || args.path.trim() === "") return `${tool} needs a file path.`;
    const problem = pathProblem(root, cwd, args.path);
    if (problem) return problem;
    if (Array.isArray(args.paths)) {
      for (const p of args.paths) {
        const extra = pathProblem(root, cwd, p);
        if (extra) return extra;
      }
    }
    return null;
  }
  if (PATH_LIST_TOOLS.has(tool)) return listProblem(root, cwd, args.path);
  if (tool === "bash") {
    if (typeof args.command !== "string" || args.command.trim() === "") return "bash needs a command.";
    return args.cwd === undefined || args.cwd === null ? null : pathProblem(root, cwd, args.cwd);
  }
  return `The ${tool} tool is not available in Code mode.`;
}

export default function guard(pi) {
  pi.registerCommand(GUARD_COMMAND, {
    description: "Shodh Code mode guard (keeps tools inside the code folder)",
    handler: async () => {},
  });
  pi.on("tool_call", async (event, ctx) => {
    const configured = process.env[ROOT_ENV];
    if (!configured) return { block: true, reason: "Code mode has no code folder." };
    let root;
    try {
      root = realpathSync.native(configured);
    } catch {
      return { block: true, reason: "The code folder is not available." };
    }
    const problem = toolCallProblem(root, ctx.cwd, event.toolName, event.input);
    if (problem) return { block: true, reason: problem };
    if (!CHANGE_TOOLS.has(event.toolName)) return undefined;
    if (!ctx.hasUI) return { block: true, reason: "No approval channel is available." };
    const answer = await ctx.ui.input(
      APPROVAL_TITLE,
      JSON.stringify({ toolCallId: event.toolCallId, tool: event.toolName, input: event.input }),
    );
    if (answer === ALLOW) return undefined;
    return {
      block: true,
      reason: typeof answer === "string" && answer.trim() !== "" ? answer : "The user did not approve this.",
    };
  });
}
