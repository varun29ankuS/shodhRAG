import { describe, expect, it } from 'vitest';
import { VIEW_TABS, VIEW_TAB_LABELS } from '../../app/src/lib/viewTabs.ts';
import { canGoBack, initialNav, navigate, SECTION_LABELS, SECTIONS, TABS, type Nav, type NavAction } from '../src/lib/nav';
import { startNav } from '../src/lib/start';
import { Subscriptions } from '../src/lib/subscriptions';

const run = (actions: NavAction[], from: Nav = initialNav()) => actions.reduce(navigate, from);

describe('the menu', () => {
  it("lists every desktop section in the desktop's order and names, with Approvals", () => {
    expect(SECTIONS.filter(s => s !== 'approvals')).toEqual([...VIEW_TABS]);
    for (const tab of VIEW_TABS) expect(SECTION_LABELS[tab]).toBe(VIEW_TAB_LABELS[tab]);
    expect(SECTIONS).toContain('approvals');
    expect(SECTIONS.indexOf('approvals')).toBe(SECTIONS.indexOf('activity') - 1);
    expect(TABS.map(t => SECTION_LABELS[t])).toEqual(['Ask', 'Library', 'Approvals']);
  });
});

describe('going back', () => {
  it('closes what is on top first, then goes back within the section, then to Ask', () => {
    const deep = run([
      { type: 'go', section: 'library' },
      { type: 'folder', folder: { path: '/p', name: 'Papers' } },
      { type: 'folder', folder: { path: '/p/r', name: 'Retrieval' } },
      { type: 'read', target: { path: '/p/r/a.pdf', name: 'a.pdf', page: null, passage: null, back: 'Retrieval', origin: null } },
      { type: 'drawer', open: true },
    ]);
    // Opening the Reader closed nothing; the menu opened over it.
    expect(deep.reader?.path).toBe('/p/r/a.pdf');
    const steps: string[] = [];
    let nav = deep;
    while (canGoBack(nav)) {
      nav = navigate(nav, { type: 'back' });
      steps.push(`${nav.section}:${nav.library.length}:${nav.drawer}:${nav.reader !== null}`);
    }
    expect(steps).toEqual(['library:2:false:true', 'library:2:false:false', 'library:1:false:false', 'library:0:false:false', 'ask:0:false:false']);
    expect(navigate(nav, { type: 'back' })).toBe(nav);
  });

  it('a breadcrumb goes straight to its folder; choosing the section again goes to its top', () => {
    const inFolder = run([
      { type: 'folder', folder: { path: '/p', name: 'Papers' } },
      { type: 'folder', folder: { path: '/p/r', name: 'Retrieval' } },
    ]);
    expect(navigate(inFolder, { type: 'crumb', depth: 1 }).library.map(f => f.name)).toEqual(['Papers']);
    expect(navigate(inFolder, { type: 'go', section: 'library' }).library).toEqual([]);
    const asked = run([{ type: 'push', screen: { kind: 'conversations' } }]);
    expect(navigate(asked, { type: 'go', section: 'ask' }).ask).toEqual([{ kind: 'home' }]);
  });

  it("opens a debug build's requested screen, and nothing it does not know", () => {
    expect(startNav('library')?.section).toBe('library');
    expect(startNav('menu')?.drawer).toBe(true);
    expect(startNav('conversations')?.ask.map(s => s.kind)).toEqual(['home', 'conversations']);
    expect(startNav('tasks')?.section).toBe('tasks');
    expect(startNav('nowhere')).toBeNull();
  });
});

describe("the sessions the phone hears", () => {
  it('sends the union of every part of the app, again after a new connection', async () => {
    const sent: string[][] = [];
    const subscriptions = new Subscriptions(async ids => {
      sent.push(ids);
    });
    await subscriptions.set('approvals', ['s7', 's7']);
    await subscriptions.set('ask', ['s1']);
    await subscriptions.set('ask', ['s1']);
    await subscriptions.set('approvals', []);
    await subscriptions.reconnected();
    expect(sent).toEqual([['s7'], ['s1', 's7'], ['s1'], ['s1']]);
  });

  it('tries again with the next change when sending failed', async () => {
    let fail = true;
    const sent: string[][] = [];
    const subscriptions = new Subscriptions(async ids => {
      if (fail) throw new Error('offline');
      sent.push(ids);
    });
    await expect(subscriptions.set('ask', ['s1'])).rejects.toThrow('offline');
    fail = false;
    await subscriptions.set('ask', ['s1']);
    expect(sent).toEqual([['s1']]);
  });
});
