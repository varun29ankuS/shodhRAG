/**
 * Activity entries in plain words: an icon, a short title ("Settings changed")
 * and the details, with the new value marked to stand out ("Theme: Dark →
 * **Light**"). Every event type and action the app records has its own sentence
 * (app/tests/auditSentence.test.ts checks them against the Rust emitters); one
 * this version does not know reads as its words, never as raw JSON.
 */
import { formatCost, formatMs, formatTokens } from '../agent/format.ts';
import { agentModeInfo, parseAgentMode } from '../agent/agentModes.ts';
import { PROVIDER_LABELS } from '../modelPicker/modelTypes.ts';

/** Which icon an entry shows (mapped to a lucide icon in AuditTable). */
export type EventIcon =
  | 'question'
  | 'answer'
  | 'tool'
  | 'search'
  | 'document'
  | 'web'
  | 'approval'
  | 'source'
  | 'settings'
  | 'model'
  | 'key'
  | 'runtime'
  | 'retention'
  | 'memory'
  | 'forget'
  | 'recall'
  | 'code'
  | 'citation'
  | 'scope'
  | 'connector'
  | 'phone'
  | 'other';

/** A run of the details; `strong` marks a new value. */
export interface Segment {
  text: string;
  strong?: boolean;
}

export interface EventSentence {
  icon: EventIcon;
  /** What happened, in a few words: "Settings changed". */
  title: string;
  /** The details after the title; empty when the title says it all. */
  details: Segment[];
  /** False when the event type or action is one this version has no sentence for. */
  known: boolean;
}

/** The fields of an entry the sentence reads. */
export interface DescribedEvent {
  eventType: string;
  payload: Record<string, unknown> | null | undefined;
}

// ── Small readers ────────────────────────────────────────────────

function str(value: unknown): string | null {
  return typeof value === 'string' && value.trim() !== '' ? value : null;
}

function num(value: unknown): number | null {
  return typeof value === 'number' && Number.isFinite(value) ? value : null;
}

function obj(value: unknown): Record<string, unknown> | null {
  return typeof value === 'object' && value !== null && !Array.isArray(value) ? (value as Record<string, unknown>) : null;
}

function clip(text: string, max = 120): string {
  const single = text.replace(/\s+/g, ' ').trim();
  return single.length > max ? `${single.slice(0, max - 1)}…` : single;
}

/** Last path component, for compact display. */
function baseName(path: string): string {
  const parts = path.split(/[\\/]/).filter(Boolean);
  return parts.length > 0 ? parts[parts.length - 1] : path;
}

function plural(n: number, one: string, many: string): string {
  return `${n.toLocaleString()} ${n === 1 ? one : many}`;
}

/** An identifier in words: `tool_call` → "Tool call", `learnModel` → "Learn model". */
export function humanize(id: string): string {
  const words = id
    .replace(/([a-z0-9])([A-Z])/g, '$1 $2')
    .replace(/[_\-.]+/g, ' ')
    .replace(/\s+/g, ' ')
    .trim()
    .toLowerCase();
  return words ? words[0].toUpperCase() + words.slice(1) : id;
}

/** A tool id as people read it; an MCP tool (`mcp__server__tool`) keeps its server's name. */
function toolName(id: unknown): string {
  const tool = str(id);
  if (!tool) return 'A tool';
  const mcp = /^mcp__([^_].*?)__(.+)$/.exec(tool);
  if (mcp) return `${mcp[1]}: ${humanize(mcp[2]).toLowerCase()}`;
  return humanize(tool);
}

function providerName(id: unknown): string {
  const provider = str(id);
  if (!provider) return 'a provider';
  if (provider === 'local') return 'a model on this computer';
  return (PROVIDER_LABELS as Record<string, string>)[provider] ?? humanize(provider);
}

function text(value: string): Segment {
  return { text: value };
}

function strong(value: string): Segment {
  return { text: value, strong: true };
}

/** "Label: old → **new**" (or "Label: **new**" with no old value). */
function change(label: string, from: string | null, to: string): Segment[] {
  return from === null ? [text(`${label}: `), strong(to)] : [text(`${label}: ${from} → `), strong(to)];
}

/** Segments joined with a separator. */
function join(groups: Segment[][], separator = ' · '): Segment[] {
  const out: Segment[] = [];
  groups
    .filter(group => group.length > 0)
    .forEach((group, i) => {
      if (i > 0) out.push(text(separator));
      out.push(...group);
    });
  return out;
}

function sentence(icon: EventIcon, title: string, details: Segment[] = [], known = true): EventSentence {
  return { icon, title, details, known };
}

function onOff(value: unknown): string {
  return value === true ? 'On' : value === false ? 'Off' : valueText(value);
}

// ── Setting values and field names ───────────────────────────────

/** Words for the values settings take that are not obvious from the value itself. */
const VALUE_WORDS: Record<string, string> = {
  light: 'Light',
  dark: 'Dark',
  system: 'System',
  off: 'Off',
  ask: 'Ask',
  auto: 'Automatic',
};

function valueText(value: unknown): string {
  if (value === true) return 'On';
  if (value === false) return 'Off';
  if (value === null || value === undefined) return 'None';
  if (typeof value === 'number') return Number.isFinite(value) ? value.toLocaleString() : String(value);
  if (typeof value === 'string') return VALUE_WORDS[value] ?? (value.trim() === '' ? 'None' : value);
  if (Array.isArray(value)) return value.length === 0 ? 'None' : plural(value.length, 'item', 'items');
  return 'Changed';
}

/** Names of the fields of the preference groups (the Rust structs, camelCase). */
const FIELD_LABELS: Record<string, string> = {
  // Privacy (Policy)
  localOnly: 'Local-only mode',
  webAccess: 'Web access',
  // Memory (MemoryPrefs)
  injectMemories: 'Use memories in answers',
  learnMode: 'Learn from conversations',
  learnModel: 'Learning model',
  autoMinConfidence: 'Lowest confidence saved without asking',
  learnCaps: 'Daily learning limits',
  maxCallsPerDay: 'Learning model calls per day',
  maxInputCharsPerDay: 'Characters sent for learning per day',
  maxProposalsPerDay: 'Memory suggestions per day',
  // Answers (AnswerPrefs)
  autoRepair: 'Re-check flagged statements',
  thinkBeforeAnswering: 'Think before answering',
  // Notifications (NotificationPrefs)
  reminders: 'Task reminders',
  needsYou: 'Needs you',
  memorySuggestions: 'Memory suggestions',
  workFinished: 'Work finished',
  problems: 'Problems',
  // Tables (TablesPrefs)
  dailyCallCap: 'Model calls tables may make per day',
  diagnostics: 'Table diagnostics',
};

/** How a field's value reads (percentages, the learning model's default). */
function fieldValue(field: string, value: unknown): string {
  if (field === 'autoMinConfidence' && typeof value === 'number') return `${Math.round(value * 100)}%`;
  if (field === 'learnModel' && (value === null || value === undefined || value === '')) return 'The answering model';
  return valueText(value);
}

interface FieldChange {
  label: string;
  from: string;
  to: string;
}

/** The fields that differ between two preference objects, nested groups included. */
function fieldChanges(before: unknown, after: unknown): FieldChange[] {
  const a = obj(before) ?? {};
  const b = obj(after) ?? {};
  const keys = Array.from(new Set([...Object.keys(a), ...Object.keys(b)]));
  const changes: FieldChange[] = [];
  for (const key of keys) {
    const x = a[key];
    const y = b[key];
    if (obj(x) || obj(y)) {
      changes.push(...fieldChanges(x, y));
      continue;
    }
    if (JSON.stringify(x) === JSON.stringify(y)) continue;
    changes.push({ label: FIELD_LABELS[key] ?? humanize(key), from: fieldValue(key, x), to: fieldValue(key, y) });
  }
  return changes;
}

function changesSegments(before: unknown, after: unknown): Segment[] {
  const changes = fieldChanges(before, after);
  if (changes.length === 0) return [text('No visible change')];
  return join(changes.map(c => change(c.label, c.from, c.to)), '; ');
}

/** Settings the agent may change (`SettingKey`), by key. */
const PREFERENCE_LABELS: Record<string, string> = {
  theme: 'Theme',
  search_max_results: 'Passages per search',
  text_size: 'Text size',
};

function by(p: Record<string, unknown>): string {
  return p.via === 'agent' ? ' by Shodh' : '';
}

// ── Settings changes ─────────────────────────────────────────────

function settingsChange(p: Record<string, unknown>): EventSentence {
  const action = str(p.action);
  const failed = p.ok === false ? [text(' · '), strong('failed')] : [];
  switch (action) {
    case 'preference_change': {
      const key = str(p.key) ?? '';
      const label = PREFERENCE_LABELS[key] ?? humanize(key || 'setting');
      return sentence('settings', `Settings changed${by(p)}`, change(label, valueText(p.old), valueText(p.new)));
    }
    case 'policy_change':
      return sentence('settings', 'Privacy changed', changesSegments(p.old, p.new));
    case 'memory_preferences_change':
      return sentence('memory', 'Memory settings changed', changesSegments(p.old, p.new));
    case 'answer_preferences_change':
      return sentence('settings', 'Answer settings changed', changesSegments(p.old, p.new));
    case 'notification_preferences_change':
      return sentence('settings', 'Notifications changed', changesSegments(p.old, p.new));
    case 'tables_preferences_change':
      return sentence('settings', 'Table settings changed', changesSegments(p.old, p.new));
    case 'code_mode_change':
      return sentence('code', 'Settings changed', change('Code mode', onOff(p.old), onOff(p.new)));
    case 'default_mode_change': {
      const mode = (value: unknown) => {
        const parsed = parseAgentMode(value);
        return parsed ? agentModeInfo(parsed).label : valueText(value);
      };
      return sentence('settings', 'Settings changed', change('Mode for new conversations', mode(p.old), mode(p.new)));
    }
    case 'close_to_tray':
      return sentence('settings', 'Settings changed', change('Keep running when the window is closed', onOff(p.old), onOff(p.new)));
    case 'agent_idle_minutes': {
      const minutes = (value: unknown) => {
        const n = num(value);
        return n === null ? valueText(value) : n === 0 ? 'Until Shodh closes' : plural(n, 'minute', 'minutes');
      };
      return sentence('settings', 'Settings changed', change('Keep an idle assistant for', minutes(p.old), minutes(p.new)));
    }
    case 'start_with_windows':
      return sentence('settings', 'Settings changed', change('Start with Windows', onOff(p.old), onOff(p.new)));
    case 'check_for_updates':
      return sentence('settings', 'Settings changed', change('Check for updates', onOff(p.old), onOff(p.new)));
    case 'app_update_install':
      return sentence('runtime', 'Update installed', change('Shodh', valueText(p.old), valueText(p.new)));
    case 'memory_learning_stop': {
      const discarded = num(p.discarded) ?? 0;
      return sentence('memory', 'Learning stopped', [
        ...change('Learn from conversations', valueText(p.old), 'Off'),
        text(` · ${plural(discarded, 'waiting suggestion', 'waiting suggestions')} discarded`),
      ]);
    }
    case 'code_settings_change': {
      const allowlist = obj(p.new)?.allowlist;
      const allowed = Array.isArray(allowlist) ? allowlist.length : 0;
      return sentence('code', 'Code settings changed', [
        strong(plural(allowed, 'command runs without asking', 'commands run without asking')),
      ]);
    }
    case 'code_build_trust':
      return sentence('code', p.trusted === true ? 'Builds trusted' : 'Build trust removed', [
        text(p.trusted === true ? 'Auto mode may run this workspace’s builds and tests' : 'Builds and tests ask first again'),
      ]);
    case 'provider_connection': {
      const provider = providerName(p.provider);
      return p.change === 'sign_out'
        ? sentence('key', 'Signed out', [strong(provider)])
        : sentence('key', 'Signed in', [strong(provider)]);
    }
    case 'api_key_set':
      return sentence('key', 'API key saved', [text('For '), strong(providerName(p.provider))]);
    case 'api_key_deleted':
      return sentence('key', 'API key removed', [text('For '), strong(providerName(p.provider))]);
    case 'model_switch': {
      const target = [str(p.provider) ? providerName(p.provider) : null, str(p.model)].filter(Boolean).join(' · ');
      if (p.mode === 'disabled') return sentence('model', 'Model turned off', failed.length > 0 ? [strong('failed')] : []);
      return sentence('model', 'Model changed', [text('To '), strong(target || 'a new model'), ...failed]);
    }
    case 'indexing_paused':
      return sentence('source', `Indexing paused${by(p)}`, [text('It stays paused until it is resumed')]);
    case 'indexing_resumed':
      return sentence('source', `Indexing resumed${by(p)}`);
    // GitHub
    case 'github_sign_in':
      return sentence('connector', 'GitHub connected', [text('As '), strong(str(p.login) ?? 'your account')]);
    case 'github_sign_out':
      return sentence('connector', 'GitHub disconnected', [text(str(p.login) ?? 'Your account')]);
    case 'github_add_repo':
      return sentence('connector', 'GitHub repository added', [strong(str(p.repo) ?? 'A repository'), text(` · ${repoAccess(p)}`)]);
    case 'github_set_repo': {
      const before = obj(p.before);
      return sentence('connector', 'GitHub access changed', [
        text(`${str(p.repo) ?? 'A repository'}: ${before ? `${repoAccess(before)} → ` : ''}`),
        strong(repoAccess(p)),
      ]);
    }
    case 'github_remove_repo':
      return sentence('connector', 'GitHub repository removed', [text(str(p.repo) ?? 'A repository')]);
    case 'github_grant':
      return sentence('connector', 'GitHub access widened', [
        text(`${str(p.repo) ?? 'A repository'}: `),
        strong(repoAccess(p)),
        text(' (you approved it)'),
      ]);
    // Slack
    case 'slack_connect':
      return sentence('connector', 'Slack connected', [strong(str(p.slackWorkspace) ?? 'A Slack workspace'), ...(str(p.bot) ? [text(` · as ${str(p.bot)}`)] : [])]);
    case 'slack_check_permissions': {
      const scopes = Array.isArray(p.scopes) ? p.scopes.length : 0;
      return sentence('connector', 'Slack permissions checked', [text(`${str(p.slackWorkspace) ?? 'Slack'} · ${plural(scopes, 'permission', 'permissions')}`)]);
    }
    case 'slack_set_enabled':
      return sentence('connector', 'Slack answers changed', change('Answer in Slack', null, onOff(p.enabled)));
    case 'slack_set_answer_workspace':
      return sentence('connector', 'Slack answers changed', change('Answers come from', null, str(p.workspace) ?? 'No workspace'));
    case 'slack_set_answer_channels': {
      const channels = Array.isArray(p.channels) ? p.channels.length : 0;
      return sentence('connector', 'Slack answers changed', [
        ...change('Channels', null, channels === 0 ? 'None' : plural(channels, 'channel', 'channels')),
        ...(p.sharedWithOtherOrganizations === true ? [text(' · includes channels shared with other organisations')] : []),
      ]);
    }
    case 'slack_set_daily_cap':
      return sentence('connector', 'Slack answers changed', change('Answers per day', valueText(p.old), valueText(p.new)));
    case 'slack_disconnect':
      return sentence('connector', 'Slack disconnected', [
        text(str(p.slackWorkspace) ?? 'The Slack workspace'),
        ...(p.forgot === true ? [text(' · its threads and memories forgotten')] : []),
      ]);
    case 'slack_index_channel':
      return sentence('connector', 'Slack channel added', [
        strong(`#${str(p.channel) ?? 'channel'}`),
        ...(num(p.windowDays) !== null ? [text(` · last ${plural(num(p.windowDays) ?? 0, 'day', 'days')}`)] : []),
      ]);
    case 'slack_forget_channel':
      return sentence('forget', 'Slack channel forgotten', [
        text(`#${str(p.channel) ?? 'channel'} · ${plural(num(p.threads) ?? 0, 'thread', 'threads')}, ${plural(num(p.memories) ?? 0, 'memory', 'memories')}`),
      ]);
    case 'slack_set_index_window':
      return sentence('connector', 'Slack settings changed', change('Channel history kept', null, plural(num(p.days) ?? 0, 'day', 'days')));
    case 'slack_set_owner':
      return sentence('connector', 'Slack settings changed', change('You in Slack', null, str(p.owner) ?? 'Nobody'));
    case 'slack_promote_memory':
      return sentence('memory', 'Slack memory kept', [text(`From #${str(p.channel) ?? 'channel'}${str(p.author) ? `, by ${str(p.author)}` : ''}`)]);
    case 'slack_delete_memory':
      return sentence('forget', 'Slack memory deleted', [text(`From #${str(p.channel) ?? 'channel'}${str(p.author) ? `, by ${str(p.author)}` : ''}`)]);
    // Telegram
    case 'telegram_connect':
      return sentence('connector', 'Telegram connected', [text('Bot '), strong(`@${str(p.bot) ?? 'bot'}`)]);
    case 'telegram_link_code':
      return sentence('connector', 'Telegram link created', [text(p.kind === 'invite' ? 'To invite someone' : 'To link your own account')]);
    case 'telegram_owner_linked':
      return sentence('connector', 'Telegram account linked', [strong(str(p.name) ?? 'Your account')]);
    case 'telegram_person_joined':
      return sentence('connector', 'Someone joined on Telegram', [strong(str(p.name) ?? 'A person')]);
    case 'telegram_unlink_owner':
      return sentence('connector', 'Telegram account unlinked');
    case 'telegram_set_enabled':
      return sentence('connector', 'Telegram answers changed', change('Answer in Telegram', null, onOff(p.enabled)));
    case 'telegram_set_answer_workspace':
      return sentence('connector', 'Telegram answers changed', change('Answers come from', null, str(p.workspace) ?? 'No workspace'));
    case 'telegram_allow_group':
      return sentence('connector', 'Telegram group allowed', [strong(str(p.group) ?? 'A group')]);
    case 'telegram_remove_group':
      return sentence('connector', 'Telegram group removed', [text(str(p.group) ?? 'A group')]);
    case 'telegram_remove_person':
      return sentence('connector', 'Telegram person removed', [text(str(p.name) ?? 'A person')]);
    case 'telegram_set_daily_cap':
      return sentence('connector', 'Telegram answers changed', change('Answers per day', valueText(p.old), valueText(p.new)));
    case 'telegram_disconnect':
      return sentence('connector', 'Telegram disconnected', [
        text(str(p.bot) ? `@${str(p.bot)}` : 'The bot'),
        ...(p.forgot === true ? [text(' · its conversations forgotten')] : []),
      ]);
    default:
      break;
  }
  // The audit log's own retention setting has no action, only the setting it changed.
  if (p.setting === 'audit_retention_days') {
    const days = (value: unknown) => {
      const n = num(value);
      return n === null ? '?' : plural(n, 'day', 'days');
    };
    return sentence('retention', 'Settings changed', change('Keep Activity for', days(p.old), days(p.new)));
  }
  return sentence('settings', 'Settings changed', action ? [text(humanize(action))] : [], false);
}

function repoAccess(p: Record<string, unknown>): string {
  if (p.write !== true) return 'read only';
  return p.merge === true ? 'read, write and merge' : 'read and write';
}

// ── Sources ──────────────────────────────────────────────────────

const SOURCE_TITLES: Record<string, string> = {
  index_folder: 'Folder indexed',
  add: 'Folder added',
  add_file: 'File added',
  reindex: 'Indexed again',
  remove: 'Source removed',
  clear_all: 'All documents removed',
  reset: 'Index reset',
  create_folder: 'Folder created',
  download: 'File downloaded',
  indexing_cancelled: 'Indexing stopped',
};

function sourceChange(p: Record<string, unknown>): EventSentence {
  const action = str(p.action) ?? '';
  const title = SOURCE_TITLES[action];
  if (action === 'indexing_cancelled') {
    const runs = num(p.runs) ?? 0;
    return sentence('source', `${title}${by(p)}`, [text(`${plural(runs, 'indexing run', 'indexing runs')} stopped`)]);
  }
  const target = str(p.path) ?? str(p.source_id);
  const files = num(p.files);
  // Files already indexed that the folder took over instead of indexing again.
  const unchanged = num(p.files_unchanged) ?? 0;
  const counts: Segment[][] = [];
  if (files !== null && (files > 0 || unchanged === 0)) counts.push([text(plural(files, 'file', 'files'))]);
  if (unchanged > 0) counts.push([text(counts.length > 0 ? `${unchanged.toLocaleString()} already indexed` : `${plural(unchanged, 'file', 'files')} already indexed`)]);
  const details = join([
    target ? [strong(action === 'add_file' || action === 'download' ? baseName(target) : clip(target, 90))] : [],
    ...counts,
    p.ok === false ? [strong('failed')] : [],
  ]);
  if (!title) return sentence('source', `Sources changed${by(p)}`, join([[text(humanize(action || 'change'))], details]), false);
  return sentence('source', `${title}${by(p)}`, details);
}

// ── Memories ─────────────────────────────────────────────────────

const MEMORY_TITLES: Record<string, string> = {
  remember: 'Memory saved',
  update: 'Memory edited',
  pin: 'Memory pinned',
  unpin: 'Memory unpinned',
  not_relevant: 'Memory marked not relevant',
  archive: 'Memory archived',
  unarchive: 'Memory restored',
  resolve_conflict: 'Conflicting memory retired',
  undo_resolve_conflict: 'Conflict decision undone',
  undo_learned: 'Learned memory undone',
  link: 'Memories linked',
  undo_link: 'Memory link undone',
};

function memoryWrite(p: Record<string, unknown>): EventSentence {
  const action = str(p.action) ?? 'remember';
  const title = MEMORY_TITLES[action];
  const outcome = str(p.outcome);
  // Older entries carry the memory's words; newer ones only its kind and length.
  const words = str(p.text);
  const kind = str(p.class);
  const via = p.via === 'agent' ? [text('By Shodh, approved by you')] : [];
  const what = words ? [text(`“${clip(words, 90)}”`)] : kind ? [text(humanize(kind))] : [];
  if (action === 'not_relevant') return sentence('memory', title, join([what, [text('It will weigh less next time')]]));
  if (action === 'link' || action === 'undo_link') {
    const ids = Array.isArray(p.ids) ? p.ids.length : 2;
    return sentence('memory', title, [text(plural(ids, 'memory', 'memories'))]);
  }
  if (outcome === 'unchanged') return sentence('memory', 'Already remembered', join([what, via]));
  if (!title) return sentence('memory', 'Memory changed', join([[text(humanize(action))], what]), false);
  return sentence('memory', title, join([what, via]));
}

// ── Models ───────────────────────────────────────────────────────

const MODEL_REASONS: Record<string, string> = {
  user_choice: 'you chose it',
  session_override: 'for this conversation',
  fallback_once: 'backup for one answer',
  fallback_automatic: 'automatic backup',
  startup: 'at startup',
};

function modelLabel(ref: unknown, described: unknown): string | null {
  const model = obj(ref);
  const name = model ? str(model.model) : null;
  if (model && name) return `${name} (${providerName(model.provider)})`;
  const text = str(described);
  return text && text !== 'none' ? text : null;
}

/**
 * What a paired phone did, in words (`phone_link.rs` records the command name, never its
 * arguments). A command phones may not call is never shown by its id.
 */
const PHONE_COMMANDS: Record<string, string> = {
  pair: 'Paired with this computer',
  connect: 'Connected',
  remove: 'Removed',
  agent_start: 'Started a conversation',
  agent_send: 'Asked a question',
  agent_abort: 'Stopped an answer',
  agent_approve: 'Answered an approval',
  workspaces_list: 'Listed the workspaces',
  conversations_list: 'Listed the conversations',
  conversation_load: 'Opened a conversation',
  library_folders: 'Browsed the library',
  get_source_file_info: 'Opened a document',
  get_pdf_info: 'Opened a document',
  read_source_page: 'Read a page',
  read_source_text: 'Read a document',
  read_source_table: 'Read a table',
};

function phoneLink(p: Record<string, unknown>): EventSentence {
  const device = str(p.device) ?? 'A phone';
  const command = str(p.command);
  const did =
    command && Object.prototype.hasOwnProperty.call(PHONE_COMMANDS, command)
      ? PHONE_COMMANDS[command]
      : 'Tried something phones cannot do';
  const outcome = str(p.outcome);
  const reason = str(p.reason);
  const details: Segment[][] = [[strong(did)]];
  if (outcome === 'refused') details.push([text(reason ? `Refused: ${clip(reason)}` : 'Refused')]);
  if (outcome === 'failed') details.push([text('Did not work')]);
  if (command === 'remove') return sentence('phone', 'You removed a phone', [strong(device)]);
  return sentence('phone', `From ${device}`, join(details));
}

function modelChange(p: Record<string, unknown>): EventSentence {
  const from = modelLabel(p.from_model, p.from);
  const to = modelLabel(p.to_model, p.to) ?? 'a new model';
  const reason = str(p.reason);
  const failure = str(p.failure);
  return sentence(
    'model',
    reason === 'fallback_once' || reason === 'fallback_automatic' ? 'Switched to a backup model' : 'Model changed',
    join([
      from ? [text(`${from} → `), strong(to)] : [strong(to)],
      reason ? [text(MODEL_REASONS[reason] ?? humanize(reason))] : [],
      failure ? [text(`after: ${humanize(failure).toLowerCase()}`)] : [],
    ]),
    reason === null || reason in MODEL_REASONS,
  );
}

// ── Retrievals ───────────────────────────────────────────────────

function retrieval(p: Record<string, unknown>): EventSentence {
  const tool = str(p.tool);
  const query = str(p.query);
  const quoted = query ? [text(`“${clip(query, 80)}”`)] : [];
  switch (tool) {
    case 'open_document': {
      const path = str(p.path);
      const location = str(p.location);
      return sentence('document', 'Document opened', join([[strong(path ? baseName(path) : 'A document')], location ? [text(location)] : []]));
    }
    case 'web_search':
    case 'search_papers': {
      const sources = Array.isArray(p.sources) ? p.sources.length : 0;
      return sentence(
        'web',
        tool === 'web_search' ? 'Searched the web' : 'Searched for papers',
        join([quoted, [text(plural(sources, 'result', 'results'))]]),
      );
    }
    case 'fetch_url': {
      const url = str(p.final_url) ?? str(p.url);
      let host = url;
      if (url) {
        try {
          host = new URL(url).host || url;
        } catch {
          host = url;
        }
      }
      return sentence('web', 'Web page read', [strong(host ? clip(host, 80) : 'A web page')]);
    }
    default: {
      const passages = Array.isArray(p.passages) ? p.passages.length : 0;
      return sentence(
        'search',
        'Documents searched',
        join([[strong(plural(passages, 'passage', 'passages'))], query ? [text(`for “${clip(query, 80)}”`)] : []]),
        // A document search (the only other retrieval) carries its passages.
        tool === null || Array.isArray(p.passages),
      );
    }
  }
}

// ── The sentence of any entry ────────────────────────────────────

const APPROVAL_TITLES: Record<string, string> = {
  approved: 'You approved',
  denied: 'You declined',
  timed_out: 'Approval timed out',
  cancelled: 'Approval cancelled',
  refused: 'Refused',
  auto_approved: 'Ran without asking',
};

/** The answer's run status (`audit/tap.rs`). */
const ANSWER_TITLES: Record<string, string> = {
  completed: 'Answered',
  aborted: 'Answer stopped',
  error: 'Answer failed',
};

export function describeEvent(row: DescribedEvent): EventSentence {
  const p = row.payload ?? {};
  switch (row.eventType) {
    case 'question': {
      const words = str(p.text) ?? '';
      return sentence('question', p.steer === true ? 'Redirected the answer' : 'Asked', words ? [text(`“${clip(words)}”`)] : []);
    }
    case 'answer': {
      const status = str(p.status) ?? 'completed';
      const parts: Segment[][] = [];
      const tin = num(p.tokens_in);
      const tout = num(p.tokens_out);
      if (tin !== null || tout !== null) parts.push([text(`${formatTokens(tin ?? 0)} in / ${formatTokens(tout ?? 0)} out`)]);
      const cost = formatCost(num(p.cost_usd) ?? 0);
      if (cost) parts.push([strong(cost)]);
      const citations = Array.isArray(p.citations) ? p.citations.length : 0;
      parts.push([text(plural(citations, 'citation', 'citations'))]);
      const duration = num(p.duration_ms);
      if (duration !== null) parts.push([text(formatMs(duration))]);
      return sentence('answer', ANSWER_TITLES[status] ?? `Answer ${humanize(status).toLowerCase()}`, join(parts), status in ANSWER_TITLES);
    }
    case 'tool_call': {
      const summary = str(p.summary);
      const duration = num(p.duration_ms);
      return sentence(
        'tool',
        p.ok === false ? 'Tool failed' : 'Tool used',
        join([[strong(toolName(p.tool))], summary ? [text(clip(summary, 80))] : [], duration !== null ? [text(formatMs(duration))] : []]),
      );
    }
    case 'retrieval':
      return retrieval(p);
    case 'approval': {
      const decision = str(p.decision) ?? '';
      const label = str(p.label) ?? toolName(p.tool);
      // A call the mode let run says which mode it was ("Ran without asking · Auto mode").
      const mode = decision === 'auto_approved' ? parseAgentMode(p.mode) : null;
      return sentence(
        'approval',
        APPROVAL_TITLES[decision] ?? 'Approval decided',
        join([[strong(clip(label, 100))], mode ? [text(`${agentModeInfo(mode).label} mode`)] : []]),
        decision in APPROVAL_TITLES,
      );
    }
    case 'source_change':
      return sourceChange(p);
    case 'settings_change':
      return settingsChange(p);
    case 'runtime_install': {
      // The checksum stays in the entry's details; the line says what the check meant.
      if (p.component === 'search_models') {
        return p.ok === false
          ? sentence('runtime', 'Search models download failed', [text(clip(str(p.error) ?? '', 80))])
          : sentence('runtime', 'Search models downloaded', [text('Checked: they match the official copies')]);
      }
      const version = str(p.version);
      const name = version ? [strong(`Answer engine (omp ${version})`)] : [strong('Answer engine')];
      return p.ok === false
        ? sentence('runtime', 'Install failed', join([name, [text(clip(str(p.error) ?? '', 80))]]))
        : sentence('runtime', 'Installed', join([name, [text('checked: it matches the official copy')]]));
    }
    case 'retention_checkpoint': {
      const deleted = num(p.deleted) ?? 0;
      return sentence('retention', 'Old entries removed', [
        text(`${plural(deleted, 'entry', 'entries')} past the retention period (through #${num(p.last_deleted_id) ?? '?'})`),
      ]);
    }
    case 'memory_write':
      return memoryWrite(p);
    case 'memory_forget': {
      const versions = Array.isArray(p.ids) ? p.ids.length : num(p.versions) ?? 1;
      const words = str(p.text);
      const kind = str(p.class);
      return sentence(
        'forget',
        'Memory forgotten',
        join([words ? [text(`“${clip(words, 90)}”`)] : kind ? [text(humanize(kind))] : [], [text(plural(versions, 'version', 'versions'))]]),
      );
    }
    case 'memory_use': {
      const count = Array.isArray(p.ids) ? p.ids.length : 0;
      const memories = plural(count, 'memory', 'memories');
      // At the end of an answer: the memories it drew on, of those it was given.
      if (p.action === 'used_in_answer') {
        const considered = num(p.considered);
        return sentence('recall', 'Memories used in an answer', [strong(memories), ...(considered !== null ? [text(` of ${considered.toLocaleString()} given`)] : [])]);
      }
      const query = str(p.query);
      return sentence('recall', 'Memories recalled', join([[strong(memories)], query ? [text(`for “${clip(query, 80)}”`)] : []]));
    }
    case 'model_change':
      return modelChange(p);
    case 'code_change': {
      const branch = str(p.branch) ?? 'a branch';
      const base = str(p.base) ?? 'the original branch';
      return p.action === 'discarded'
        ? sentence('code', 'Code changes set aside', [text(`Back on ${base}; the changes stay on `), strong(branch)])
        : sentence('code', 'Code branch started', [text('Changes go to '), strong(branch), text(` (from ${base})`)]);
    }
    case 'citation_corrected': {
      const from = Array.isArray(p.from) ? p.from.filter(n => typeof n === 'number').map(n => `[${n}]`).join('') : '';
      const score = num(p.score);
      return sentence('citation', 'You changed a citation', [
        text(`${from || '?'} → `),
        strong(`[${num(p.to) ?? '?'}]`),
        ...(score !== null ? [text(` · support ${Math.round(score * 100)}%`)] : []),
      ]);
    }
    case 'scope_change': {
      const moved = obj(p.moved) ?? {};
      const kinds: [string, string, string][] = [
        ['memories', 'memory', 'memories'],
        ['snippets', 'snippet', 'snippets'],
        ['results', 'result', 'results'],
        ['others', 'other statement', 'other statements'],
      ];
      const parts = kinds
        .map(([key, one, many]) => {
          const n = num(moved[key]) ?? 0;
          return n > 0 ? plural(n, one, many) : null;
        })
        .filter((part): part is string => part !== null);
      const reason = str(p.reason);
      return sentence('scope', 'Made available everywhere', join([[strong(parts.join(', ') || 'Nothing')], reason ? [text(humanize(reason).toLowerCase())] : []]));
    }
    case 'phone_link':
      return phoneLink(p);
    default:
      return sentence('other', humanize(row.eventType || 'event'), [], false);
  }
}

/** The sentence as one line of text ("Settings changed · Theme: Dark → Light"). */
export function sentenceText(s: EventSentence): string {
  const details = s.details.map(d => d.text).join('');
  return details ? `${s.title} · ${details}` : s.title;
}
