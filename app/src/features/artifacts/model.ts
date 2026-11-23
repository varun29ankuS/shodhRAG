/**
 * The Artifacts tab's data and the rules it shows them by: Creator mode's creations
 * (`creation_commands.rs`), the progress of a build, the finish card's choices and what
 * the preview window says.
 *
 * Pure (no React, no Tauri) so it is unit-tested directly with Node
 * (`app/tests/artifacts.test.ts`).
 */

export type CreationStatus = 'building' | 'ready' | 'kept';

export interface VersionInfo {
  number: number;
  createdAt: string;
  runId: string | null;
  files: number;
  bytes: number;
  /** Files left out by the caps (count and size). */
  leftOut: number;
}

export interface Creation {
  id: string;
  conversationId: string;
  title: string;
  folder: string;
  createdAt: string;
  updatedAt: string;
  status: CreationStatus;
  summary: string | null;
  choices: string[];
  versions: VersionInfo[];
}

export interface FileEntry {
  path: string;
  size: number;
}

export interface FileContent {
  path: string;
  size: number;
  text: string | null;
  binary: boolean;
  truncated: boolean;
}

/** A version's changes: the app's one diff (`features/code/diffModel`). */
export type { DiffLine, FileChange } from '../code/diffModel';

/** Code the agent wrote in Code mode: a conversation's branch in its workspace folder. */
export interface CodeChange {
  conversationId: string;
  folder: string;
  branch: string;
  base: string;
  createdAt: string;
  autoApplied: number;
}

/** A preview shown in its own window (`creations/preview_window.rs`). */
export interface PreviewInfo {
  /** The preview window, for moving, reloading and closing it. */
  label: string;
  network: boolean;
  page: string;
  /** Its page already finished loading (a preview kept from before, or a fast first load). */
  loaded: boolean;
}

/** The Artifacts pane's place, as the embedded preview needs it (`preview_embed.rs`). */
export interface PreviewPlacement {
  /** The pane in this page's CSS pixels, or null while it is not on screen. */
  rect: { x: number; y: number; width: number; height: number } | null;
  /** Something of the app's own (a dialog, menu or toast) is above the pane. */
  covered: boolean;
  /**
   * The width (CSS pixels) of a pane docked at the window's right (the design check, the code
   * review): the floating chat sits left of it, over the page, never over the pane. None when
   * left out (a hidden page has no chat over it).
   */
  rightInset?: number;
}

/** The panes of a creation's page, in order. */
export const ARTIFACT_PANES = ['preview', 'files', 'versions'] as const;
export type ArtifactPane = typeof ARTIFACT_PANES[number];

export const PANE_LABELS: Record<ArtifactPane, string> = {
  preview: 'Preview',
  files: 'Files',
  versions: 'Versions',
};

export function isArtifactPane(value: unknown): value is ArtifactPane {
  return typeof value === 'string' && (ARTIFACT_PANES as readonly string[]).includes(value);
}


/**
 * What the preview says about itself, exactly as far as it goes.
 *
 * With the switch off, on Windows the preview window's own browser sends everything to a
 * closed address and lets WebRTC use no direct route, so nothing it sends leaves the
 * computer. On a Mac no engine setting closes WebRTC; a page script guard is all there is,
 * and guards can be broken, so the text says so plainly (`creations/preview.rs`).
 */
const ISOLATION = 'The page runs in a window of its own: it cannot use Shodh, read your files or reach your accounts.';

export const PREVIEW_NETWORK_OFF_TEXT_WINDOWS =
  `${ISOLATION} Its traffic goes to a closed address, so it cannot load or send anything over the internet, WebRTC included. Camera, microphone and other devices are refused.`;

export const PREVIEW_NETWORK_OFF_TEXT_MAC =
  `${ISOLATION} It cannot load pages or send requests over the internet. On a Mac a hostile page might still reach the internet through WebRTC, which Shodh can only partly block there, and that can reveal that it ran.`;

export const PREVIEW_NETWORK_ON_TEXT =
  `${ISOLATION} While this is on, it can load from and send to any secure (https) site, and use WebRTC.`;

/** The line under the internet switch, for this computer. */
export function previewNetworkText(network: boolean, mac: boolean): string {
  if (network) return PREVIEW_NETWORK_ON_TEXT;
  return mac ? PREVIEW_NETWORK_OFF_TEXT_MAC : PREVIEW_NETWORK_OFF_TEXT_WINDOWS;
}

/** Where a preview shows: inside Artifacts over its pane, or in a window of its own. */
export type PreviewPlace = 'embedded' | 'window';

function placeOf(value: unknown): PreviewPlace {
  return value === 'embedded' ? 'embedded' : 'window';
}

/** The preview open now (`creation_preview_status`), if any. */
export interface PreviewShown {
  label: string;
  creationId: string;
  version: number | null;
  network: boolean;
  place: PreviewPlace;
}

export function parsePreviewShown(value: unknown): PreviewShown | null {
  if (typeof value !== 'object' || value === null) return null;
  const { label, creationId, version, network, place } = value as Record<string, unknown>;
  if (typeof label !== 'string' || typeof creationId !== 'string' || typeof network !== 'boolean') return null;
  const at = typeof version === 'number' && Number.isInteger(version) && version > 0 ? version : null;
  return { label, creationId, version: at, network, place: placeOf(place) };
}

/** The event Rust sends the main window about its preview window (`preview_window.rs`). */
export const PREVIEW_EVENT = 'creation-preview';

export type PreviewState = 'loaded' | 'blocked' | 'closed' | 'stopped';

export interface PreviewEvent {
  label: string;
  /**
   * Loaded: the first page finished. Blocked: it tried to leave its files, open a window or
   * download. Closed: the window went (the user, the page or Shodh closed it). Stopped: the
   * page's browser process crashed or hung.
   */
  state: PreviewState;
  /** In Artifacts or in its own window. */
  place: PreviewPlace;
}

const PREVIEW_STATES: readonly string[] = ['loaded', 'blocked', 'closed', 'stopped'];

export function parsePreviewEvent(payload: unknown): PreviewEvent | null {
  if (typeof payload !== 'object' || payload === null) return null;
  const { label, state, place } = payload as { label?: unknown; state?: unknown; place?: unknown };
  if (typeof label !== 'string' || typeof state !== 'string' || !PREVIEW_STATES.includes(state)) return null;
  return { label, state: state as PreviewState, place: placeOf(place) };
}

/** What the card says when the page was stopped from leaving its files. */
export const PREVIEW_BLOCKED_TEXT = 'The page tried to open another address, a window or a download. Shodh stopped it.';

/** Why the preview window closed, when the tab closed it or saw it go. */
export type PreviewEnd = 'closed' | 'stopped' | 'version' | 'network';

export const PREVIEW_END_TEXT: Record<PreviewEnd, string> = {
  closed: 'The preview closed.',
  stopped: 'The preview stopped working. Open it again to restart it.',
  version: 'The preview closed because you picked another version. Open it again to see that version.',
  network: 'The preview closed because you changed the internet switch. Open it again to use the new setting.',
};

/** The pane's state line for the preview: live here, or in its own window. */
export function previewShowingText(version: number | null, place: PreviewPlace = 'window'): string {
  const what = version === null ? 'Now' : `Version ${version}`;
  return place === 'embedded' ? `${what} · live` : `${what} · in its own window`;
}

/** How far the page itself got, as the pane shows it. */
export type PreviewLoad = 'opening' | 'loaded' | 'blocked' | 'stopped';

/** The page's state after Rust's report `state` (a close is handled by the pane itself). */
export function previewLoadAfter(prev: PreviewLoad, state: Exclude<PreviewState, 'closed'>): PreviewLoad {
  if (state === 'stopped') return 'stopped';
  if (state === 'blocked') return prev === 'stopped' ? prev : 'blocked';
  return prev === 'opening' ? 'loaded' : prev;
}

/**
 * The page's state when the pane learns its label: `loaded` is Rust's word that this page
 * loaded already (a page kept from an earlier visit, or one that finished first), and `early`
 * are the reports for that label that arrived before the label was known.
 */
export function previewLoadAtEmbed(loaded: boolean, early: readonly PreviewState[]): PreviewLoad {
  let load: PreviewLoad = loaded ? 'loaded' : 'opening';
  for (const state of early) {
    if (state !== 'closed') load = previewLoadAfter(load, state);
  }
  return load;
}

/** What the pane's status line reads from. */
export interface PreviewLineInput {
  error: string | null;
  end: PreviewEnd | null;
  load: PreviewLoad;
  /** What of the app's own is over the page, hiding it (an embedded page only). */
  hiddenBy: string | null;
  version: number | null;
  poppedOut: boolean;
}

/**
 * The pane's status line: why the page is not there (it could not open, it closed, it
 * stopped, something hides it), or that it is opening, or where it shows.
 */
export function previewStateLine(input: PreviewLineInput): string {
  if (input.error) return 'Could not open';
  if (input.end) return PREVIEW_END_TEXT[input.end];
  if (input.load === 'stopped') return 'Stopped';
  if (!input.poppedOut && input.hiddenBy) return `Hidden while ${input.hiddenBy} is open`;
  if (input.load === 'opening') return 'Opening';
  return previewShowingText(input.version, input.poppedOut ? 'window' : 'embedded');
}

/** The page a creation's preview opens: `index.html` at the top of its folder. */
export const ENTRY_PAGE = 'index.html';

/** Whether a creation's files have the page its preview shows. */
export function hasEntryPage(files: readonly FileEntry[] | null): boolean {
  return (files ?? []).some(f => f.path === ENTRY_PAGE);
}

/** What Artifacts opens on for a request (the assistant's, or a Creator build starting). */
export interface ArtifactSelection {
  id: string;
  version: number | null;
  pane: ArtifactPane;
  /**
   * Show the page itself, not only the pane: the request opened the preview pane (a build
   * starting, or the assistant's open_artifact on the preview or on no pane in particular).
   * The page shows inside Artifacts and takes no focus.
   */
  showPreview: boolean;
  /** Increases with each request, so repeating one re-applies it. */
  seq: number;
}

/** The selection an `artifact` navigation target opens. */
export function selectionFor(
  target: { artifactId: string; version: number | null; pane: string | null },
  prev: { seq: number } | null,
): ArtifactSelection {
  const pane: ArtifactPane = isArtifactPane(target.pane) ? target.pane : 'preview';
  return {
    id: target.artifactId,
    version: target.version,
    pane,
    showPreview: pane === 'preview',
    seq: (prev?.seq ?? 0) + 1,
  };
}

/**
 * Whether the open preview no longer matches what the page asks for, and why: another
 * version or a changed switch closes it (with a note), anything else keeps it.
 */
export function previewEndFor(shown: PreviewShown, creationId: string, version: number | null, network: boolean): PreviewEnd | null {
  if (shown.creationId !== creationId) return null;
  if (shown.version !== version) return 'version';
  if (shown.network !== network) return 'network';
  return null;
}

/** Tools whose start means the agent began building: the tab opens. */
const BUILDING_TOOLS: readonly string[] = ['write', 'edit', 'run_command'];

export function startsBuilding(tool: string): boolean {
  return BUILDING_TOOLS.includes(tool);
}

/** The parts of a transcript step this reads. */
export interface StepLike {
  tool: string;
  status: string;
  label: string;
  args: unknown;
  detail?: unknown;
}

function pathOf(args: unknown): string | null {
  if (typeof args !== 'object' || args === null) return null;
  const path = (args as { path?: unknown }).path;
  return typeof path === 'string' && path.trim() ? path.trim() : null;
}

/** What a build has done so far, for the progress line. */
export interface BuildProgress {
  /** Distinct files written or edited. */
  files: number;
  /** Commands that ran. */
  commands: number;
  /** What it is doing now, in plain words; null when nothing runs. */
  now: string | null;
  /** Something waits for the user's approval. */
  waiting: boolean;
}

export function buildProgress(steps: readonly StepLike[]): BuildProgress {
  const files = new Set<string>();
  let commands = 0;
  let now: string | null = null;
  let waiting = false;
  for (const step of steps) {
    if ((step.tool === 'write' || step.tool === 'edit') && step.status === 'done') {
      const path = pathOf(step.args);
      if (path) files.add(path);
    }
    if (step.tool === 'run_command' && step.status === 'done') commands += 1;
    if (step.status === 'awaiting_approval') waiting = true;
    if (step.status === 'running' || step.status === 'awaiting_approval') now = step.label;
  }
  return { files: files.size, commands, now, waiting };
}

/** The progress line under the preview, in plain words. */
export function progressLine(progress: BuildProgress, running: boolean): string {
  const parts: string[] = [];
  if (progress.waiting) parts.push('Waiting for your approval');
  else if (progress.now) parts.push(progress.now);
  else if (running) parts.push('Working');
  parts.push(`${progress.files} ${progress.files === 1 ? 'file' : 'files'} written`);
  if (progress.commands > 0) parts.push(`${progress.commands} ${progress.commands === 1 ? 'command' : 'commands'} run`);
  return parts.join(' · ');
}

/** What the agent recorded with `finish_creation`, from its step's detail. */
export interface FinishCard {
  summary: string;
  choices: string[];
}

/** Longest choice shown, and most shown (the tool caps them too). */
const MAX_CHOICE_CHARS = 160;
const MAX_CHOICES = 12;

/**
 * The finish card of an answer: its last finished `finish_creation` step. Only plain
 * strings are taken, cut to the tool's caps; anything else in the detail is ignored.
 */
export function finishCard(steps: readonly StepLike[]): FinishCard | null {
  for (let i = steps.length - 1; i >= 0; i--) {
    const step = steps[i];
    if (step.tool !== 'finish_creation' || step.status !== 'done') continue;
    const detail = step.detail;
    if (typeof detail !== 'object' || detail === null) continue;
    const { summary, choices } = detail as { summary?: unknown; choices?: unknown };
    if (typeof summary !== 'string' || !summary.trim()) continue;
    const list = Array.isArray(choices)
      ? choices
          .filter((c): c is string => typeof c === 'string' && c.trim().length > 0)
          .slice(0, MAX_CHOICES)
          .map(c => c.trim().slice(0, MAX_CHOICE_CHARS))
      : [];
    return { summary: summary.trim().slice(0, 200), choices: list };
  }
  return null;
}

/** What Change something puts in the composer. */
export const CHANGE_DRAFT = 'Change: ';

/** The status as the list shows it. */
export const STATUS_LABELS: Record<CreationStatus, string> = {
  building: 'Building',
  ready: 'Ready',
  kept: 'Kept',
};

/** A size in plain words ("12 KB"). */
export function sizeLabel(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${Math.round(bytes / 1024)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

/** The creations whose title, summary or choices contain `text` (any case). */
export function filterCreations(creations: readonly Creation[], text: string): Creation[] {
  const needle = text.trim().toLowerCase();
  if (!needle) return [...creations];
  return creations.filter(c =>
    c.title.toLowerCase().includes(needle)
    || (c.summary ?? '').toLowerCase().includes(needle)
    || c.choices.some(choice => choice.toLowerCase().includes(needle)),
  );
}

/** The version a diff compares `number` with: the one before it (0 is nothing). */
export function previousVersion(versions: readonly VersionInfo[], number: number): number {
  const earlier = versions.filter(v => v.number < number).map(v => v.number);
  return earlier.length > 0 ? Math.max(...earlier) : 0;
}

/** Files as a tree's rows: each folder once, before its files, indented by depth. */
export interface FileRow {
  /** The full path of a file, or a folder's path with a trailing `/`. */
  key: string;
  name: string;
  depth: number;
  folder: boolean;
  size: number;
}

export function fileRows(files: readonly FileEntry[]): FileRow[] {
  const rows: FileRow[] = [];
  const seen = new Set<string>();
  const sorted = [...files].sort((a, b) => a.path.localeCompare(b.path));
  for (const file of sorted) {
    const parts = file.path.split('/');
    for (let depth = 0; depth < parts.length - 1; depth++) {
      const key = `${parts.slice(0, depth + 1).join('/')}/`;
      if (seen.has(key)) continue;
      seen.add(key);
      rows.push({ key, name: parts[depth], depth, folder: true, size: 0 });
    }
    rows.push({ key: file.path, name: parts[parts.length - 1], depth: parts.length - 1, folder: false, size: file.size });
  }
  return rows;
}
