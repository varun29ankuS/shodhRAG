/**
 * The Artifacts tab's data and the rules it shows them by: Creator mode's creations
 * (`creation_commands.rs`), the progress of a build, the finish card's choices and where
 * the preview window goes.
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

export interface DiffLine {
  kind: 'same' | 'added' | 'removed';
  text: string;
  old: number | null;
  new: number | null;
}

export interface FileChange {
  path: string;
  status: 'added' | 'removed' | 'changed';
  binary: boolean;
  tooLarge: boolean;
  hunks: { lines: DiffLine[] }[];
}

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
 * What the preview says about itself, exactly as far as it goes. With the switch off the
 * page cannot load or send anything through the web, WebRTC is gone and DNS look-ahead is
 * off; a browser may still warm up a connection to a link the page shows, so it is not
 * called a sealed network.
 */
export const PREVIEW_NETWORK_OFF_TEXT =
  'The page runs in a window of its own: it cannot use Shodh, read your files or reach your accounts. It cannot load or send anything over the internet, and WebRTC and DNS look-ahead are off. A hostile page might still make the browser connect ahead to a link it shows, which can reveal that it ran.';

export const PREVIEW_NETWORK_ON_TEXT =
  'The page runs in a window of its own: it cannot use Shodh, read your files or reach your accounts. While this is on, it can load from and send to any secure (https) site.';

/**
 * The preview area, in CSS pixels of the app's page, and the page's size: Rust turns it
 * into screen pixels by the page's size on screen (`preview_window.rs` `Area`).
 */
export interface PreviewArea {
  x: number;
  y: number;
  width: number;
  height: number;
  viewportWidth: number;
  viewportHeight: number;
}

/** A box on the page, in CSS pixels. */
export interface Box {
  left: number;
  top: number;
  right: number;
  bottom: number;
}

/**
 * Something drawn over the page: a dialog, menu, toast or the conversation dock. The
 * preview is a window above the page, so it would hide these; it makes room or closes.
 */
export interface FloatingLayer {
  box: Box;
  /** A modal dialog: the preview closes while it is open. */
  modal: boolean;
}

/** Below this the preview closes rather than show a sliver (CSS pixels). */
export const MIN_PREVIEW_HEIGHT = 160;
export const MIN_PREVIEW_WIDTH = 200;

/** Space kept between the preview and a layer it makes room for. */
const LAYER_GAP = 8;

/** The elements that float over the page (portaled dialogs, menus, toasts, the dock). */
export const FLOATING_LAYER_SELECTOR =
  '[role="dialog"], [role="alertdialog"], [role="menu"], [role="listbox"], [role="tooltip"], [data-sonner-toast], [data-floating-layer]';

function overlaps(a: Box, b: Box): boolean {
  return a.left < b.right && b.left < a.right && a.top < b.bottom && b.top < a.bottom;
}

function fits(box: Box): boolean {
  return box.right - box.left >= MIN_PREVIEW_WIDTH && box.bottom - box.top >= MIN_PREVIEW_HEIGHT;
}

function areaOf(box: Box): number {
  return (box.right - box.left) * (box.bottom - box.top);
}

/**
 * `box` cut clear of `layer`: ending above it, or beside it (on the side away from it),
 * whichever leaves more; null when neither leaves enough.
 */
function clearOf(box: Box, layer: Box): Box | null {
  const above: Box = { ...box, bottom: Math.min(box.bottom, layer.top - LAYER_GAP) };
  const onRight = (layer.left + layer.right) / 2 >= (box.left + box.right) / 2;
  const beside: Box = onRight
    ? { ...box, right: Math.min(box.right, layer.left - LAYER_GAP) }
    : { ...box, left: Math.max(box.left, layer.right + LAYER_GAP) };
  const options = [above, beside].filter(fits).sort((a, b) => areaOf(b) - areaOf(a));
  return options[0] ?? null;
}

/**
 * Where the preview window goes: the preview pane's box, cut to what the page shows of it
 * (`visible`, the scrolling area) and to the window, then clear of each layer over it
 * (above it or beside it, whichever leaves more). Null (the preview closes) while a modal
 * dialog is open, or when too little is left.
 */
export function previewPlacement(
  pane: Box,
  visible: Box,
  viewport: { width: number; height: number },
  layers: readonly FloatingLayer[],
): PreviewArea | null {
  if (layers.some(layer => layer.modal)) return null;
  let box: Box = {
    left: Math.max(pane.left, visible.left, 0),
    top: Math.max(pane.top, visible.top, 0),
    right: Math.min(pane.right, visible.right, viewport.width),
    bottom: Math.min(pane.bottom, visible.bottom, viewport.height),
  };
  // Each cut only shrinks the box, so a layer it was already clear of stays clear; a later
  // cut can bring the box clear of an earlier one too, so the layers are checked until none
  // overlaps.
  for (let pass = 0; pass <= layers.length; pass++) {
    const covering = layers.find(layer => overlaps(box, layer.box));
    if (!covering) break;
    const cut = clearOf(box, covering.box);
    if (!cut) return null;
    box = cut;
  }
  if (layers.some(layer => overlaps(box, layer.box))) return null;
  const x = Math.max(0, Math.round(box.left));
  const y = Math.max(0, Math.round(box.top));
  const width = Math.min(Math.round(box.right), viewport.width) - x;
  const height = Math.min(Math.round(box.bottom), viewport.height) - y;
  if (width < MIN_PREVIEW_WIDTH || height < MIN_PREVIEW_HEIGHT) return null;
  return { x, y, width, height, viewportWidth: viewport.width, viewportHeight: viewport.height };
}

export function samePlacement(a: PreviewArea | null, b: PreviewArea | null): boolean {
  if (a === null || b === null) return a === b;
  return a.x === b.x && a.y === b.y && a.width === b.width && a.height === b.height
    && a.viewportWidth === b.viewportWidth && a.viewportHeight === b.viewportHeight;
}

/** The event Rust sends the main window about its preview window (`preview_window.rs`). */
export const PREVIEW_EVENT = 'creation-preview';

export interface PreviewEvent {
  label: string;
  /** Loaded: the first page finished. Blocked: it tried to leave its files, open a window or download. */
  state: 'loaded' | 'blocked';
}

export function parsePreviewEvent(payload: unknown): PreviewEvent | null {
  if (typeof payload !== 'object' || payload === null) return null;
  const { label, state } = payload as { label?: unknown; state?: unknown };
  if (typeof label !== 'string' || (state !== 'loaded' && state !== 'blocked')) return null;
  return { label, state };
}

/** What the preview area says under the window, in plain words. */
export const PREVIEW_BLOCKED_TEXT = 'The page tried to open another address, a window or a download. Shodh stopped it.';

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
