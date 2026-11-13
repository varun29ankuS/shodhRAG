/**
 * Where the app is: the section, each section's own history, the menu, the
 * Reader and the approval sheet. Pure (a reducer), so the drawer and back
 * behaviour are tested without a screen.
 *
 * The sections are the desktop's own, in the desktop's order and with its
 * names (app/src/lib/viewTabs.ts, read here, never copied), plus Approvals,
 * which the phone shows as a section of its own (the desktop shows them in
 * its Inbox).
 */
import { VIEW_TABS, VIEW_TAB_LABELS, type ViewTab } from '../../../app/src/lib/viewTabs.ts';

export type Section = ViewTab | 'approvals';

/** The menu, in the desktop's order; Approvals before Activity. */
export const SECTIONS: readonly Section[] = (() => {
  const order: Section[] = [...VIEW_TABS];
  order.splice(order.indexOf('activity'), 0, 'approvals');
  return order;
})();

export const SECTION_LABELS: Record<Section, string> = { ...VIEW_TAB_LABELS, approvals: 'Approvals' };

/** The floating tab bar: the three most used. */
export const TABS: readonly Section[] = ['ask', 'library', 'approvals'];

/** A screen of the Ask section. */
export type AskScreen =
  | { kind: 'home' }
  | { kind: 'conversations' }
  /** A conversation: saved, and (with `question`) asking a new question in it. */
  | { kind: 'answer'; conversationId: string; question: string | null; workspaceId: string | null };

/** A folder of the Library: its path on the computer and its name. */
export interface Folder {
  path: string;
  name: string;
}

/** Where a Reader was opened from, to zoom out of. */
export interface Origin {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface ReaderTarget {
  path: string;
  name: string;
  /** 1-based page to open at, when known. */
  page: number | null;
  /** The cited passage, shown highlighted. */
  passage: string | null;
  /** What Back returns to, in words ("Answer", the folder's name). */
  back: string;
  origin: Origin | null;
}

/** An approval answered in the bottom sheet. */
export interface SheetTarget {
  sessionId: string;
  stepId: string;
}

export interface Nav {
  section: Section;
  /** The Ask section's screens, the home first. */
  ask: AskScreen[];
  /** The Library's open folders, outermost first; empty at the linked folders. */
  library: Folder[];
  drawer: boolean;
  reader: ReaderTarget | null;
  sheet: SheetTarget | null;
}

export type NavAction =
  | { type: 'go'; section: Section }
  | { type: 'drawer'; open: boolean }
  | { type: 'push'; screen: AskScreen }
  | { type: 'folder'; folder: Folder }
  /** Show the Library at a breadcrumb: `depth` folders deep (0: the linked folders). */
  | { type: 'crumb'; depth: number }
  | { type: 'read'; target: ReaderTarget }
  | { type: 'sheet'; target: SheetTarget | null }
  | { type: 'back' };

export const HOME: AskScreen = { kind: 'home' };

export function initialNav(section: Section = 'ask'): Nav {
  return { section, ask: [HOME], library: [], drawer: false, reader: null, sheet: null };
}

/** Whether Back has anything to close or leave (otherwise the phone leaves the app). */
export function canGoBack(nav: Nav): boolean {
  return (
    nav.sheet !== null ||
    nav.reader !== null ||
    nav.drawer ||
    (nav.section === 'ask' && nav.ask.length > 1) ||
    (nav.section === 'library' && nav.library.length > 0) ||
    nav.section !== 'ask'
  );
}

export function navigate(nav: Nav, action: NavAction): Nav {
  switch (action.type) {
    case 'go':
      // Choosing a section shows its first screen; choosing the one shown again goes to its top.
      if (action.section === nav.section) {
        return { ...nav, drawer: false, ask: action.section === 'ask' ? [HOME] : nav.ask, library: action.section === 'library' ? [] : nav.library };
      }
      return { ...nav, section: action.section, drawer: false };
    case 'drawer':
      return { ...nav, drawer: action.open };
    case 'push':
      return { ...nav, section: 'ask', drawer: false, ask: [...nav.ask, action.screen] };
    case 'folder':
      return { ...nav, section: 'library', drawer: false, library: [...nav.library, action.folder] };
    case 'crumb':
      return { ...nav, library: nav.library.slice(0, Math.max(0, action.depth)) };
    case 'read':
      return { ...nav, drawer: false, reader: action.target };
    case 'sheet':
      return { ...nav, sheet: action.target };
    case 'back':
      // What is on top closes first: the menu, the sheet, the Reader.
      if (nav.drawer) return { ...nav, drawer: false };
      if (nav.sheet) return { ...nav, sheet: null };
      if (nav.reader) return { ...nav, reader: null };
      if (nav.section === 'ask' && nav.ask.length > 1) return { ...nav, ask: nav.ask.slice(0, -1) };
      if (nav.section === 'library' && nav.library.length > 0) return { ...nav, library: nav.library.slice(0, -1) };
      if (nav.section !== 'ask') return { ...nav, section: 'ask' };
      return nav;
  }
}

/** The Ask screen shown. */
export function askScreen(nav: Nav): AskScreen {
  return nav.ask[nav.ask.length - 1] ?? HOME;
}
