/**
 * The screen a debug build opens at, for CI's screenshots (the Rust side
 * reads it only in debug builds: `start_screen` in mobile/src-tauri). With
 * no computer paired, each shows what it shows while not connected.
 */
import { initialNav, type Nav, type Section } from './nav';

const SECTIONS: readonly Section[] = ['ask', 'artifacts', 'library', 'workspaces', 'connectors', 'tasks', 'approvals', 'activity', 'settings'];

/** Where `screen` is: a section, "home", "conversations", "menu" (the menu open) or "pair". */
export function startNav(screen: string): Nav | null {
  if (screen === 'pair') return initialNav();
  if (screen === 'home') return initialNav('ask');
  if (screen === 'conversations') return { ...initialNav('ask'), ask: [{ kind: 'home' }, { kind: 'conversations' }] };
  if (screen === 'menu') return { ...initialNav('ask'), drawer: true };
  const section = SECTIONS.find(s => s === screen);
  return section ? initialNav(section) : null;
}
