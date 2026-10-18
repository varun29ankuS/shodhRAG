// Shodh Code mode guard, loaded into omp with `--hook` (see code_mode.rs).
//
// omp has no setting that keeps its file tools inside one folder, so every
// tool call passes through this `tool_call` handler first:
// - only the Code mode tools are allowed, plus the host tools Shodh registered
//   for the session (named in SHODH_HOST_TOOLS; Shodh asks for and audits
//   those itself); any other tool is blocked;
// - every path a tool would touch must resolve (through symlinks and
//   junctions) inside the code folder; URLs and omp's internal resources
//   (`local://`, `ssh://`, `proc://`, web pages, ...) are blocked;
// - edits and writes never touch the repository's `.git` folder;
// - edits, writes and shell commands wait for the user's decision, asked
//   through omp's dialog channel and answered by Shodh's approval prompt,
//   which is told the file an edit or write lands in (`target`).
// A handler error blocks the call (omp fails closed on `tool_call` errors).
//
// The guard checks the path omp will open, so it follows omp 18.4.10's own
// reading of a path argument (packages/coding-agent/src/tools/path-utils.ts:
// `expandPath`, `resolveToCwd`, `normalizeWindowsDriveAliasPath`,
// `normalizePathLikeInput`, `splitDelimitedPathEntry`; the `:selector` split
// of packages/tui/src/tools/read.ts `splitPathAndSel`). Where omp may read one
// argument in several ways (a literal file or a list of paths, with or
// without a selector), every reading must stay inside the folder. Forms whose
// reading depends on omp's guesses are refused outright: a leading `@`, `~`
// or `:`, quotes, colons other than a drive or a line selector, backslashes
// outside Windows, and names ending in a dot or space on Windows. Reads and
// edits need an existing file: omp searches the folder for a missing one.
// Update this file together with the pinned omp (sidecar.rs `OMP_VERSION`).
//
// Shell commands cannot be confined by path: the shell keeps the user's
// access. They are only approved one command at a time.

import { existsSync, realpathSync } from "node:fs";
import * as path from "node:path";

export const ROOT_ENV = "SHODH_CODE_ROOT";
export const HOST_TOOLS_ENV = "SHODH_HOST_TOOLS";
export const APPROVAL_TITLE = "shodh-approval";
export const GUARD_COMMAND = "shodh-guard";
export const ALLOW = "allow";

const SINGLE_PATH_TOOLS = new Set(["read", "write", "edit"]);
const PATH_LIST_TOOLS = new Set(["grep", "glob", "ast_grep"]);
const CHANGE_TOOLS = new Set(["edit", "write", "bash"]);
const CODE_TOOLS = new Set([...SINGLE_PATH_TOOLS, ...PATH_LIST_TOOLS, "bash"]);
/** Tools whose file must exist: omp looks for a missing one elsewhere in the folder. */
const EXISTING_FILE_TOOLS = new Set(["read", "edit"]);

/** The session's host tools: names from `value` (comma-separated), never a Code mode tool. */
export function hostTools(value) {
  return new Set(
    (typeof value === "string" ? value : "")
      .split(",")
      .map((name) => name.trim())
      .filter((name) => name !== "" && !CODE_TOOLS.has(name)),
  );
}

const isWindows = process.platform === "win32";

/** omp's `UNICODE_SPACES`, which it reads as plain spaces. */
const UNICODE_SPACES = /[\u00A0\u2000-\u200A\u202F\u205F\u3000]/g;

/**
 * One `:selector` chunk omp peels off a path (`10-20`, `L5`, `5+3`, `1..`,
 * `-40`, `raw`, `conflicts`, `img`): the shapes of omp's `FILE_LINE_RANGE_RE`,
 * slightly widened. A chunk never holds a separator, so the path with the
 * chunk and the path without it name entries of the same folder.
 */
const SELECTOR_CHUNK = /^(?:-?L?\d[\dL.,+-]*|raw|conflicts|img)$/i;

/** `[path#TAG]`, the header omp's hashline output wraps a path in. */
const HASHLINE_HEADER = /^\[(.+)#[0-9A-Fa-f]{4}\]$/;

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

/**
 * The file part of a tool path: without a copied `[path#TAG]` wrapper or a
 * trailing `:selector` (at most two chunks, as omp peels them). Null when a
 * colon is left that is neither a Windows drive nor a selector, or when
 * nothing is left.
 */
export function filePart(entry) {
  let text = entry;
  const wrapped = HASHLINE_HEADER.exec(text);
  if (wrapped) text = wrapped[1];
  const drive = isWindows && /^[A-Za-z]:[\\/]/.test(text) ? 2 : 0;
  for (let peeled = 0; peeled < 2; peeled++) {
    const colon = text.lastIndexOf(":");
    if (colon < drive || !SELECTOR_CHUNK.test(text.slice(colon + 1))) break;
    text = text.slice(0, colon);
  }
  if (text.indexOf(":", drive) >= 0) return null;
  return text === "" ? null : text;
}

/** omp's `windowsDriveAliasPath`: on Windows `/c/x` and `/mnt/c/x` are `C:\x`. */
function driveAlias(p) {
  if (!isWindows || !p.startsWith("/")) return null;
  const parts = p.split("/");
  let drive;
  let tail = 2;
  if (parts.length >= 2 && /^[A-Za-z]$/.test(parts[1])) {
    drive = parts[1].toUpperCase();
  } else if (parts.length >= 3 && parts[1].toLowerCase() === "mnt" && /^[A-Za-z]$/.test(parts[2])) {
    drive = parts[2].toUpperCase();
    tail = 3;
  }
  if (!drive) return null;
  return `${drive}:\\${parts.slice(tail).filter(Boolean).join("\\")}`;
}

/** Why the text of one path cannot be checked as it is, or null. */
function formProblem(entry) {
  if (entry.includes("\0")) return `"${entry}" is not a valid path.`;
  if (entry.includes("://") || /^[A-Za-z][A-Za-z0-9+.-]*:\/\//.test(entry)) {
    return `"${entry}" is a URL or internal resource; Code mode only works with files in the code folder.`;
  }
  if (entry.startsWith("~")) return `"${entry}" is outside the code folder.`;
  if (entry.startsWith("\\\\") || entry.startsWith("//")) return `"${entry}" is a network path outside the code folder.`;
  if (entry.startsWith("@") || entry.startsWith(":")) {
    return `"${entry}" starts with "${entry[0]}"; give the path in the code folder without it.`;
  }
  if (entry.includes('"')) return `"${entry}" contains quotes; give the path without them.`;
  if (!isWindows && entry.includes("\\")) {
    return `"${entry}" contains a backslash; use / between folders and no escapes.`;
  }
  if (isWindows && /^[A-Za-z]:(?![\\/])/.test(entry)) {
    return `"${entry}" is relative to a drive's current folder; give a path relative to the code folder.`;
  }
  if (isWindows && entry.split(/[\\/]/).some((part) => part !== "." && part !== ".." && /[ .]$/.test(part))) {
    return `"${entry}" has a name ending in a dot or space, which Windows reads differently.`;
  }
  return null;
}

/**
 * Check one path argument: `{ problem }` when it may not be used, else
 * `{ resolved, readings }`: the file it names (the first reading) and every
 * resolved reading of it. Both are empty for a blank entry. With
 * `mustExist`, the file (with or without its selector) must exist.
 */
export function checkPath(root, cwd, raw, mustExist = false) {
  if (typeof raw !== "string") return { problem: "A path must be text." };
  const shown = raw.trim();
  if (shown === "") return { resolved: null, readings: [] };
  const targets = [];
  for (const form of new Set([shown, raw])) {
    const entry = form.replace(UNICODE_SPACES, " ");
    const problem = formProblem(entry);
    if (problem) return { problem };
    const file = filePart(entry);
    if (file === null) {
      return { problem: `"${shown}" has a ":" that is not a line selector; Code mode cannot check where it points.` };
    }
    for (const candidate of new Set([file, entry])) {
      targets.push(/^\/+$/.test(candidate) ? cwd : path.resolve(cwd, driveAlias(candidate) ?? candidate));
    }
  }
  const readings = [];
  let exists = false;
  for (const target of targets) {
    const real = resolveExisting(target);
    if (real === null || !isWithin(root, real)) return { problem: `"${shown}" is outside the code folder.` };
    if (existsSync(real)) exists = true;
    readings.push(real);
  }
  if (mustExist && !exists) {
    return { problem: `"${shown}" does not exist in the code folder; give the exact path of one existing file.` };
  }
  return { resolved: readings[0], readings };
}

/** Why `raw` may not be used, or null when it stays inside `root`. */
export function pathProblem(root, cwd, raw) {
  return checkPath(root, cwd, raw).problem ?? null;
}

/**
 * Every part omp may split a path argument into (at `;`, at `,`, at
 * whitespace, or at all of them; braces ignored, so more splits than omp
 * makes), each trimmed. Empty for an argument without delimiters.
 */
export function pathParts(value) {
  const parts = new Set();
  for (const separator of [/;/, /,/, /\s/, /[;,\s]/]) {
    const split = value.split(separator);
    if (split.length < 2) continue;
    for (const part of split) {
      const trimmed = part.trim();
      if (trimmed !== "") parts.add(trimmed);
    }
  }
  return [...parts];
}

/**
 * Why a path argument, whole or split into a list, may not be used, or null.
 * omp keeps a delimited argument whole only when that names an existing
 * entry, so such a whole must then be inside the folder; every part must be.
 */
function listProblem(root, cwd, value) {
  if (value === undefined || value === null) return null;
  if (typeof value !== "string") return "A path must be text.";
  const parts = pathParts(value.trim());
  if (parts.length === 0) return pathProblem(root, cwd, value);
  const whole = resolveExisting(path.resolve(cwd, value.trim()));
  if (whole !== null && existsSync(whole) && !isWithin(root, whole)) return `"${value.trim()}" is outside the code folder.`;
  for (const entry of parts) {
    const problem = pathProblem(root, cwd, entry);
    if (problem) return problem;
  }
  return null;
}

/** Whether `target` (inside `root`) is a `.git` folder or file of the code folder, or inside one. */
export function inGitFolder(root, target) {
  return path
    .relative(root, target)
    .split(/[\\/]/)
    .some((part) => part.toLowerCase() === ".git");
}

/**
 * Why the call may not run, or null when it stays inside `root`. For an edit
 * or a write, `onTarget` receives the resolved file it changes.
 */
export function toolCallProblem(root, cwd, tool, input, onTarget) {
  const args = input && typeof input === "object" ? input : {};
  if (SINGLE_PATH_TOOLS.has(tool)) {
    if (tool === "edit" && (args.input !== undefined || args._input !== undefined)) {
      return "Only single-file replace edits are available in Code mode.";
    }
    if (tool === "edit" && args._path !== undefined && args._path !== args.path) {
      return "An edit names its one file in path.";
    }
    if (typeof args.path !== "string" || args.path.trim() === "") return `${tool} needs a file path.`;
    const listed = listProblem(root, cwd, args.path);
    if (listed) return listed;
    const checked = checkPath(root, cwd, args.path, EXISTING_FILE_TOOLS.has(tool));
    if (checked.problem) return checked.problem;
    if (Array.isArray(args.paths)) {
      for (const p of args.paths) {
        const extra = listProblem(root, cwd, p);
        if (extra) return extra;
      }
    } else if (args.paths !== undefined && args.paths !== null) {
      return "paths must be a list.";
    }
    if (tool !== "read") {
      if (checked.readings.some((reading) => inGitFolder(root, reading))) {
        return "Code mode does not change files in the repository's .git folder.";
      }
      if (onTarget) onTarget(checked.resolved);
    }
    return null;
  }
  if (PATH_LIST_TOOLS.has(tool)) return listProblem(root, cwd, args.path);
  if (tool === "bash") {
    if (typeof args.command !== "string" || args.command.trim() === "") return "bash needs a command.";
    return args.cwd === undefined || args.cwd === null ? null : listProblem(root, cwd, args.cwd);
  }
  return `The ${tool} tool is not available in Code mode.`;
}

export default function guard(pi) {
  pi.registerCommand(GUARD_COMMAND, {
    description: "Shodh Code mode guard (keeps tools inside the code folder)",
    handler: async () => {},
  });
  pi.on("tool_call", async (event, ctx) => {
    // Shodh runs these itself, behind its own approval and audit.
    if (hostTools(process.env[HOST_TOOLS_ENV]).has(event.toolName)) return undefined;
    const configured = process.env[ROOT_ENV];
    if (!configured) return { block: true, reason: "Code mode has no code folder." };
    let root;
    try {
      root = realpathSync.native(configured);
    } catch {
      return { block: true, reason: "The code folder is not available." };
    }
    let target;
    const problem = toolCallProblem(root, ctx.cwd, event.toolName, event.input, (resolved) => {
      target = resolved;
    });
    if (problem) return { block: true, reason: problem };
    if (!CHANGE_TOOLS.has(event.toolName)) return undefined;
    if (!ctx.hasUI) return { block: true, reason: "No approval channel is available." };
    const request = { toolCallId: event.toolCallId, tool: event.toolName, input: event.input };
    if (target !== undefined) request.target = target;
    const answer = await ctx.ui.input(APPROVAL_TITLE, JSON.stringify(request));
    if (answer === ALLOW) return undefined;
    return {
      block: true,
      reason: typeof answer === "string" && answer.trim() !== "" ? answer : "The user did not approve this.",
    };
  });
}
