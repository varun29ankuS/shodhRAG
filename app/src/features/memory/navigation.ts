/**
 * Opening a Settings section from another view (the suggested-memories badge opens
 * Settings → Memory). The request is kept until Settings takes it, so it works whether
 * Settings is already mounted (event) or mounts after the navigation (initial state).
 */
import type { SettingsSection } from '../../components/shell/SettingsView';
import { OneShot } from '../../lib/oneShot.ts';

export const OPEN_SETTINGS_SECTION = 'shodh:open-settings-section';

/** Expires if Settings never shows (lib/oneShot). */
const requested = new OneShot<SettingsSection>();

/** Ask Settings to show `section`. */
export function requestSettingsSection(section: SettingsSection): void {
  requested.set(section);
  window.dispatchEvent(new CustomEvent(OPEN_SETTINGS_SECTION));
}

/**
 * The requested section, without taking it: for a view's first render, which
 * may be rendered again before it shows (a deferred view switch). The view
 * takes it once mounted.
 */
export function peekRequestedSection(): SettingsSection | null {
  return requested.peek();
}

/** The requested section, once. */
export function takeRequestedSection(): SettingsSection | null {
  return requested.take();
}

/** Dispatched when a memory's editor is requested ([`requestMemoryEdit`]). */
export const OPEN_MEMORY_EDIT = 'shodh:open-memory-edit';

const requestedEdit = new OneShot<string>();

/**
 * Open Settings → Memory with memory `id`'s editor open ("Outdated" under an answer): the
 * edit saves a newer version, as any edit there does. Kept until Settings → Memory takes it.
 */
export function requestMemoryEdit(id: string): void {
  requestedEdit.set(id);
  requestSettingsSection('memory');
  window.dispatchEvent(new CustomEvent(OPEN_MEMORY_EDIT));
}

/** The memory whose editor was requested, without taking it (see `peekRequestedSection`). */
export function peekRequestedMemoryEdit(): string | null {
  return requestedEdit.peek();
}

/** The memory whose editor was requested, once. */
export function takeRequestedMemoryEdit(): string | null {
  return requestedEdit.take();
}
