/**
 * Styles and small helpers shared by the Tables tab's components. The buttons, inputs and
 * focus ring are the workspace page's own, so a table looks like the rest of its workspace.
 */
import { requestSettingsSection } from '../memory/navigation';
import type { CellMark } from './model';
import type { StatusTone } from './format';

export { FOCUS_RING, INPUT, LABEL, OUTLINE_BUTTON, PRIMARY_BUTTON, QUIET_BUTTON, SECTION_TITLE, TEXTAREA } from '../workspaces/ui';

/** The frame of a Tables dialog (as the workspace dialogs). */
export const DIALOG_OVERLAY = 'ask-fade-in fixed inset-0 z-[55] bg-black/40';
export const DIALOG_CONTENT =
  'ask-fade-in fixed z-[55] inset-0 m-auto flex flex-col w-[min(640px,calc(100vw-48px))] max-h-[min(760px,calc(100vh-48px))] h-fit rounded-[18px] border border-shodh-border-strong bg-shodh-surface text-shodh-text shadow-[0_24px_80px_rgba(0,0,0,0.45)] focus:outline-none';
export const DIALOG_HEADER = 'shrink-0 flex items-start gap-3 px-5 py-4 border-b border-shodh-border-subtle';
export const DIALOG_BODY = 'flex-1 min-h-0 overflow-y-auto scrollbar-thin px-5 py-4 flex flex-col gap-4';
export const CLOSE_BUTTON = 'w-8 h-8 inline-flex items-center justify-center rounded-lg text-shodh-text-muted hover:bg-shodh-raised hover:text-shodh-text';

/** Show a Settings section (Model, to fix or change the model; Tables, for the cap and diagnostics). */
export function openSettings(section: 'models' | 'tables'): void {
  requestSettingsSection(section);
  window.dispatchEvent(new CustomEvent('switchTab', { detail: 'settings' }));
}

/** What a mark says, for its badge and its screen-reader label. */
export const MARK_LABELS: Record<CellMark, string> = {
  unverified: 'Unverified',
  not_found: 'Not found',
  edited: 'Edited',
  failed: 'Failed',
};

/** The badge colours of a mark (theme tokens only). */
export const MARK_CLASSES: Record<CellMark, string> = {
  unverified: 'bg-shodh-warning/15 text-shodh-warning',
  not_found: 'bg-shodh-raised-2 text-shodh-text-muted',
  edited: 'bg-shodh-info/15 text-shodh-info',
  failed: 'bg-shodh-error/15 text-shodh-error',
};

/** The text colour of a status line by its tone. */
export const TONE_CLASSES: Record<StatusTone, string> = {
  active: 'text-shodh-text-secondary',
  waiting: 'text-shodh-text-secondary',
  needs_you: 'text-shodh-warning',
  done: 'text-shodh-success',
  failed: 'text-shodh-error',
  cancelled: 'text-shodh-text-muted',
};
