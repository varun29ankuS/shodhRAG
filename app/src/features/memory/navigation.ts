/**
 * Opening a Settings section from another view (the suggested-memories badge opens
 * Settings → Memory). The request is kept until Settings takes it, so it works whether
 * Settings is already mounted (event) or mounts after the navigation (initial state).
 */
import type { SettingsSection } from '../../components/shell/SettingsView';

export const OPEN_SETTINGS_SECTION = 'shodh:open-settings-section';

let requested: SettingsSection | null = null;

/** Ask Settings to show `section`. */
export function requestSettingsSection(section: SettingsSection): void {
  requested = section;
  window.dispatchEvent(new CustomEvent(OPEN_SETTINGS_SECTION));
}

/** The requested section, once. */
export function takeRequestedSection(): SettingsSection | null {
  const section = requested;
  requested = null;
  return section;
}
