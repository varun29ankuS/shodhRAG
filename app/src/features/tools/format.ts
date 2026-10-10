/**
 * Text of the composer's tool chip and the form that adds an MCP server.
 * Pure functions (tested in tests/toolsFormat.test.ts).
 */

interface ChatToolsCounts {
  servers: readonly { error: string | null }[];
  skills: readonly unknown[];
  total: number;
}

function plural(n: number, word: string): string {
  return `${n} ${word}${n === 1 ? '' : 's'}`;
}

/** "2 MCP · 1 skill · 31 tools": the servers that work, the skills, every tool. */
export function chipLabel(view: ChatToolsCounts): string {
  const parts: string[] = [];
  const servers = view.servers.filter(s => s.error === null).length;
  if (servers > 0) parts.push(`${servers} MCP`);
  if (view.skills.length > 0) parts.push(plural(view.skills.length, 'skill'));
  parts.push(plural(view.total, 'tool'));
  return parts.join(' · ');
}

export interface ServerForm {
  name: string;
  kind: 'stdio' | 'http';
  /** stdio: the command line ("npx -y @scope/server"); http: the URL. */
  target: string;
  /** `KEY=value` lines: environment (stdio) or headers (http, `Name: value` also accepted). */
  secrets: string;
}

/** Split a command line on spaces, keeping "quoted parts" together. */
export function splitCommand(line: string): string[] {
  const parts: string[] = [];
  const pattern = /"([^"]*)"|'([^']*)'|(\S+)/g;
  for (const match of line.matchAll(pattern)) {
    parts.push(match[1] ?? match[2] ?? match[3] ?? '');
  }
  return parts;
}

/** The form as `mcp.json` text with one server, or why it is incomplete. */
export function formToConfig(form: ServerForm): { config: string } | { error: string } {
  const name = form.name.trim();
  if (!/^[A-Za-z0-9._-]{1,48}$/.test(name)) {
    return { error: 'Name the server with letters, digits, ".", "-" or "_".' };
  }
  const pairs: Record<string, string> = {};
  for (const raw of form.secrets.split(/\r?\n/)) {
    const line = raw.trim();
    if (!line) continue;
    const at = form.kind === 'http' && !line.includes('=') ? line.indexOf(':') : line.indexOf('=');
    if (at <= 0) return { error: `"${line}" is not KEY=value.` };
    pairs[line.slice(0, at).trim()] = line.slice(at + 1).trim();
  }
  if (form.kind === 'http') {
    const url = form.target.trim();
    if (!/^https?:\/\/\S+$/.test(url)) return { error: 'Enter the server URL (http:// or https://).' };
    return { config: JSON.stringify({ mcpServers: { [name]: { url, headers: pairs } } }) };
  }
  const [command, ...args] = splitCommand(form.target.trim());
  if (!command) return { error: 'Enter the command that starts the server.' };
  return { config: JSON.stringify({ mcpServers: { [name]: { command, args, env: pairs } } }) };
}
